mod collab;
use crate::convert as cv;
use crate::*;
use reprise_doc::{Document, PersistenceMode};
use reprise_layout::incremental::{LayoutCache, LayoutContinuation, LayoutSession};
use reprise_layout::{Engine, LayoutSnapshot};
use std::collections::BTreeMap;
use std::rc::{Rc, Weak};

// InsertBlock's empty named style is the session base. Preserve an existing
// definition, including custom authored defaults in a package.
fn ensure_base_style(doc: &Document) -> Result<()> {
    if doc.style("").is_none() {
        doc.define_style(
            "",
            &reprise_doc::Style {
                family: Some("serif".into()),
                ..reprise_doc::default_style()
            },
        )?;
        doc.commit();
    }
    Ok(())
}

/// Workspace factory. Each document owns isolated font/plugin configuration.
#[derive(Default)]
pub struct Workspace;
impl Workspace {
    pub fn new() -> Self {
        Self
    }
    pub fn create(&self, request: &Payload<Create>) -> Result<DocumentSession> {
        let r = validate(request)?;
        let id = cv::document_id(&r.document_id)?;
        let peer = cv::peer(&r.peer_id)?;
        let doc = Document::new(peer)?;
        ensure_base_style(&doc)?;
        let package = reprise_format::Package::new(&doc, id, PersistenceMode::History)?;
        Ok(DocumentSession::new(
            doc,
            package,
            r.document_id.clone(),
            r.peer_id.clone(),
            Engine::new(reprise_font::FontStore::default()),
            Vec::new(),
        ))
    }
    pub fn open(&self, request: &Payload<Open>, bytes: &[u8]) -> Result<DocumentSession> {
        let r = validate(request)?;
        let peer = cv::peer(&r.peer_id)?;
        if bytes.len() > 128 * 1024 * 1024 {
            return Err(Error::Limit("package bytes".into()));
        }
        let mut opened = reprise_format::Package::open(
            bytes,
            peer,
            reprise_format::Limits::default(),
            &reprise_format::MigrationRegistry::builtin(),
        )?;
        // The core has no DocumentAt layout entry point; do not expose editable newer state.
        if opened.is_read_only() {
            return Err(Error::ReadOnly);
        }
        let mut engine = Engine::new(reprise_font::FontStore::default());
        opened.restore_fonts(&mut engine.fonts);
        opened
            .notes
            .extend(opened.assets.restore_images(&mut engine.assets));
        let package = opened.package().clone();
        let id = package
            .document_id()
            .0
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let notes = opened.notes.clone();
        let doc = opened.into_document()?;
        ensure_base_style(&doc)?;
        Ok(DocumentSession::new(
            doc,
            package,
            id,
            r.peer_id.clone(),
            engine,
            notes,
        ))
    }
}
/// A single editable replica. Neither core nor CRDT handles cross the facade.
pub struct DocumentSession {
    editor: Box<reprise_edit::Editor>,
    engine: Engine,
    package: reprise_format::Package,
    document_id: String,
    peer_id: String,
    cache: LayoutCache,
    snapshot: Option<LayoutSnapshot>,
    options: Option<LayoutOptions>,
    generation: u64,
    identity: Rc<()>,
    notes: Vec<reprise_diag::Note>,
    /// The host's selection, anchored (see `set_selection`).
    selection: Option<reprise_edit::StableSelection>,
    workers: std::sync::Arc<dyn reprise_layout::workers::Workers>,
    /// Set when a contained call panicked; see [`DocumentSession::contain`].
    poisoned: Option<String>,
}
impl DocumentSession {
    /// Runs `f` on this session and turns a panic inside it into
    /// [`Error::Poisoned`] (`bindings.poisoned`) instead of unwinding into
    /// the host. A panic can leave the store half-updated or its locks
    /// poisoned, so the session refuses every later contained call with the
    /// same error: the host should drop it, reopen the last saved package
    /// and resynchronise. Hosts route every session call through this.
    ///
    /// Only an unwinding build can contain a panic. On `wasm32` with
    /// `panic=abort` the instance traps instead; see `docs/bindings.md`.
    pub fn contain<T>(&mut self, f: impl FnOnce(&mut DocumentSession) -> Result<T>) -> Result<T> {
        if let Some(message) = &self.poisoned {
            return Err(Error::Poisoned(message.clone()));
        }
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self))) {
            Ok(result) => result,
            Err(payload) => {
                let message = crate::panic_message(payload.as_ref());
                self.poisoned = Some(message.clone());
                Err(Error::Poisoned(message))
            }
        }
    }
    /// Whether a contained call panicked; the session must be reopened.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.is_some()
    }
    fn new(
        doc: Document,
        package: reprise_format::Package,
        document_id: String,
        peer_id: String,
        engine: Engine,
        notes: Vec<reprise_diag::Note>,
    ) -> Self {
        Self {
            editor: Box::new(reprise_edit::Editor::new(doc, engine.schemas.clone())),
            engine,
            package,
            document_id,
            peer_id,
            cache: LayoutCache::default(),
            snapshot: None,
            options: None,
            generation: 0,
            identity: Rc::new(()),
            notes,
            selection: None,
            workers: std::sync::Arc::new(reprise_layout::workers::Serial),
            poisoned: None,
        }
    }
    fn doc(&self) -> &Document {
        self.editor.document()
    }
    fn bump(&mut self) -> Result<()> {
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| Error::Limit("session generations".into()))?;
        self.snapshot = None;
        Ok(())
    }
    fn reconfigure(&mut self) -> Result<()> {
        self.bump()?;
        self.cache = LayoutCache::default();
        Ok(())
    }
    pub fn state(&self) -> Result<Payload<State>> {
        let mut todo: Vec<_> = self
            .doc()
            .blocks()
            .into_iter()
            .rev()
            .map(|n| (n, 0u32))
            .collect();
        let mut blocks = Vec::new();
        // A peer can write a node whose envelope doesn't read (collaboration
        // invariant I2): leave it out, keep its children, and say so.
        let mut malformed = Vec::new();
        while let Some((node, depth)) = todo.pop() {
            if blocks.len() >= 100_000 || depth > 1024 {
                return Err(Error::Limit("document tree".into()));
            }
            let block = match self.doc().block(node) {
                Ok(block) => Some(block),
                Err(reprise_doc::DocError::Malformed(..)) => None,
                Err(e) => return Err(e.into()),
            };
            todo.extend(
                self.doc()
                    .children(Some(node))
                    .into_iter()
                    .rev()
                    .map(|n| (n, depth.saturating_add(1))),
            );
            let Some(block) = block else {
                malformed.push(node);
                continue;
            };
            blocks.push(Block {
                id: node.to_string(),
                parent: self.doc().parent_of(node).flatten().map(|n| n.to_string()),
                kind: match block.kind {
                    reprise_doc::BlockKind::Paragraph => BlockKind::Paragraph,
                    reprise_doc::BlockKind::Annotation => BlockKind::Annotation,
                    reprise_doc::BlockKind::Image => BlockKind::Image,
                },
                text: block.text.to_string(),
                // Unreadable formatting is reported by layout diagnostics.
                formatting: self.doc().text_formats(node).ok().and_then(cv::text_runs),
            });
        }
        let page = self.engine.page_setup(self.doc());
        let d = page.dimensions;
        let page_setup = PageSetup {
            width: d.width.0,
            height: d.height.0,
            margins: PageMargins {
                top: d.top.0,
                right: d.right.0,
                bottom: d.bottom.0,
                left: d.left.0,
            },
            orientation: match d.width.cmp(&d.height) {
                std::cmp::Ordering::Less => PageOrientation::Portrait,
                std::cmp::Ordering::Greater => PageOrientation::Landscape,
                std::cmp::Ordering::Equal => PageOrientation::Square,
            },
            template: page.template,
            source: match page.source {
                reprise_layout::TemplateSource::Builtin => PageTemplateSource::Builtin,
                reprise_layout::TemplateSource::Document => PageTemplateSource::Document,
            },
            patched: page.patched,
        };
        let mut diagnostics = self.diagnostics().data;
        for note in page.diagnostics {
            let note = cv::layout_diagnostic(&note);
            if !diagnostics.contains(&note) {
                diagnostics.push(note);
            }
        }
        diagnostics.extend(malformed.into_iter().map(|node| Diagnostic {
            code: reprise_doc::invariants::MALFORMED_NODE.as_str().into(),
            severity: Severity::Error,
            message: format!("node {node} has an unreadable envelope and is left out"),
            subject: Some(node.to_string()),
            start: None,
            end: None,
        }));
        Ok(Payload::new(State {
            document_id: self.document_id.clone(),
            peer_id: self.peer_id.clone(),
            revision: cv::revision(&self.doc().revision()),
            can_undo: self.editor.can_undo(),
            can_redo: self.editor.can_redo(),
            blocks,
            page_setup,
            diagnostics,
        }))
    }
    /// Select scoped native layout workers. WASM and builds without
    /// `native-workers` stay serial. Workers never change output or budgets.
    pub fn set_layout_threads(&mut self, threads: usize) -> usize {
        #[cfg(all(feature = "native-workers", not(target_arch = "wasm32")))]
        {
            let threads = threads.clamp(1, 256);
            self.workers = std::sync::Arc::new(reprise_layout::workers::Threads(threads));
            threads
        }
        #[cfg(any(not(feature = "native-workers"), target_arch = "wasm32"))]
        {
            let _ = threads;
            1
        }
    }
    pub fn set_page_setup(
        &mut self,
        request: &Payload<PageSetupPatch>,
    ) -> Result<Payload<Applied>> {
        let setup = *validate(request)?;
        self.apply(&Payload::new(Transaction {
            commands: vec![Command::SetPageSetup { setup }],
        }))
    }
    pub fn apply(&mut self, request: &Payload<Transaction>) -> Result<Payload<Applied>> {
        let r = validate(request)?;
        if let [Command::InsertImage { image }] = r.commands.as_slice() {
            return self.insert_image(&Payload::new(image.clone()));
        }
        if r.commands.len() > reprise_edit::MAX_COMMANDS {
            return Err(Error::Limit("transaction commands".into()));
        }
        let mut bytes = 0usize;
        for command in &r.commands {
            if let Command::InsertText { text, .. } | Command::InsertBlock { text, .. } = command {
                bytes = bytes.saturating_add(text.len());
            }
        }
        if bytes > reprise_edit::MAX_TRANSACTION_BYTES {
            return Err(Error::Limit("transaction text".into()));
        }
        let commands = r
            .commands
            .iter()
            .map(|c| cv::command(c, self.engine.medium))
            .collect::<Result<Vec<_>>>()?;
        let applied = self.editor.apply(&commands.into())?;
        self.snapshot = None;
        Ok(Payload::new(cv::applied(applied, Vec::new())))
    }
    /// Insert one image through the editing kernel's atomic fragment transaction.
    pub fn insert_image(&mut self, request: &Payload<ImageInsert>) -> Result<Payload<Applied>> {
        let r = validate(request)?;
        if r.asset.len() != 64
            || !r
                .asset
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::InvalidId(r.asset.clone()));
        }
        if r.alt.len() > reprise_edit::MAX_TRANSACTION_BYTES {
            return Err(Error::Limit("image alt text".into()));
        }
        let style = cv::style(&r.style)?;
        let doc = Document::new(1)?;
        ensure_base_style(&doc)?;
        let image = reprise_doc::image::ImageData {
            asset: r.asset.clone(),
            width: r.width.map(|x| reprise_doc::LengthExpr::Pt(cv::length(x))),
            height: r.height.map(|x| reprise_doc::LengthExpr::Pt(cv::length(x))),
        };
        let node = doc.append_image("", &image, &r.alt)?;
        doc.set_overrides(node, &style)?;
        doc.commit();
        let fragment = reprise_clipboard::copy_all(
            &doc,
            "bindings-image-insert",
            &self.engine.schemas,
            None,
            None,
        )?;
        self.paste_fragment(fragment, r.at.as_ref())
    }
    pub fn undo(&mut self) -> Result<Payload<bool>> {
        let changed = self.editor.undo()?;
        if changed {
            self.snapshot = None;
        }
        Ok(Payload::new(changed))
    }
    pub fn redo(&mut self) -> Result<Payload<bool>> {
        let changed = self.editor.redo()?;
        if changed {
            self.snapshot = None;
        }
        Ok(Payload::new(changed))
    }
    pub fn declare_font(
        &mut self,
        request: &Payload<FontDeclaration>,
        bytes: &[u8],
    ) -> Result<Payload<Face>> {
        let r = validate(request)?;
        if bytes.len() > 32 * 1024 * 1024 {
            return Err(Error::Limit("font bytes".into()));
        }
        let declaration = reprise_font::FontDeclaration {
            family: r.family.clone(),
            descriptors: reprise_font::Descriptors {
                weight: r.weight,
                stretch: r.stretch,
                style: match r.style {
                    FontStyle::Normal => reprise_font::FontStyle::Normal,
                    FontStyle::Italic => reprise_font::FontStyle::Italic,
                    FontStyle::Oblique => reprise_font::FontStyle::Oblique,
                },
            },
            face_index: r.face_index,
        };
        let id = self
            .engine
            .fonts
            .register(bytes.to_vec(), declaration)
            .map_err(|e| Error::note(e.note()))?;
        self.reconfigure()?;
        Ok(Payload::new(Face {
            family: id.family,
            hash: id.hash,
        }))
    }
    /// Bundle an immutable resource and install image bytes in the engine store.
    pub fn register_asset(
        &mut self,
        request: &Payload<AssetDeclaration>,
        bytes: &[u8],
    ) -> Result<Payload<String>> {
        let r = validate(request)?;
        if bytes.len() > MAX_ASSET_BYTES {
            return Err(Error::Limit("asset bytes".into()));
        }
        if r.id.is_empty() || r.id.len() > 1024 {
            return Err(Error::InvalidId(r.id.clone()));
        }
        use reprise_format::{Asset, AssetSource, ids};
        let raw = self
            .package
            .container()
            .sections
            .get(&ids::ASSETS)
            .ok_or_else(|| Error::Invalid("missing asset manifest".into()))?;
        let mut assets: Vec<Asset> =
            serde_json::from_slice(&raw.bytes).map_err(|e| Error::Invalid(e.to_string()))?;
        if assets.iter().any(|a| a.id == r.id) {
            return Err(Error::InvalidId(r.id.clone()));
        }
        let mut bundles: BTreeMap<_, _> = self
            .package
            .container()
            .sections
            .iter()
            .filter(|(id, _)| (ids::FIRST_ASSET..=ids::LAST_ASSET).contains(id))
            .map(|(id, s)| (*id, s.bytes.clone()))
            .collect();
        let section = (ids::FIRST_ASSET..=ids::LAST_ASSET)
            .find(|id| !bundles.contains_key(id))
            .ok_or_else(|| Error::Limit("asset sections".into()))?;
        let hash = reprise_format::content_hash(bytes);
        assets.push(Asset {
            id: r.id.clone(),
            kind: match r.kind {
                AssetKind::Image => reprise_format::AssetKind::Image,
                AssetKind::Other => reprise_format::AssetKind::Other,
            },
            hash: hash.clone(),
            source: AssetSource::Bundled { section },
            extra: BTreeMap::new(),
        });
        bundles.insert(section, bytes.to_vec());
        // Preflight all fallible resource work before publishing either store.
        let mut store = self.engine.assets.clone();
        if r.kind == AssetKind::Image {
            store.insert(bytes).map_err(|e| Error::Limit(e.into()))?;
        }
        self.package.set_assets(assets, bundles)?;
        self.engine.assets = store;
        self.reconfigure()?;
        Ok(Payload::new(hash))
    }
    pub fn start_layout(&mut self, request: &Payload<LayoutOptions>) -> Result<LayoutJob> {
        let r = validate(request)?;
        if r.max_pages > 10_000 || r.viewport_start > r.viewport_end || r.viewport_end > 10_000 {
            return Err(Error::Limit("layout pages/viewport".into()));
        }
        if self.options.as_ref().is_none_or(|old| {
            old.width != r.width || old.height != r.height || old.max_pages != r.max_pages
        }) {
            self.reconfigure()?;
            self.engine.medium =
                reprise_doc::Medium::new(cv::length(r.width), cv::length(r.height));
            self.engine.flow.max_pages = r.max_pages;
        }
        self.options = Some(r.clone());
        self.bump()?;
        let cache = std::mem::take(&mut self.cache);
        let mut session = LayoutSession::from_cache(&self.engine, cache);
        session.set_workers(self.workers.clone());
        let job = session.start(
            self.doc(),
            reprise_layout::incremental::Viewport::Pages(
                r.viewport_start as usize..r.viewport_end as usize,
            ),
        );
        let continuation = job.suspend();
        self.cache = session.into_cache();
        Ok(LayoutJob {
            continuation: Some(continuation),
            owner: Rc::downgrade(&self.identity),
            token: self.token(),
            snapshot: None,
            progress: None,
            cancelled: false,
        })
    }
    fn token(&self) -> LayoutToken {
        LayoutToken {
            document_id: self.document_id.clone(),
            revision: cv::revision(&self.doc().revision()),
            generation: self.generation.to_string(),
        }
    }
    fn current(&self) -> Result<&LayoutSnapshot> {
        let snapshot = self.snapshot.as_ref().ok_or(Error::NoLayout)?;
        if snapshot.revision != self.doc().revision() {
            return Err(Error::Stale);
        }
        Ok(snapshot)
    }
    fn navigator(&self) -> Result<reprise_edit::Navigator<'_>> {
        Ok(reprise_edit::Navigator::semantic(
            self.current()?,
            self.doc(),
        ))
    }
    fn checked_caret(&self, c: &Caret) -> Result<reprise_edit::Caret> {
        let c = cv::caret(c)?;
        let block = self
            .doc()
            .block(c.node)
            .map_err(|_| Error::InvalidId(c.node.to_string()))?;
        let text = block.text.to_string();
        if c.offset > text.len() || !text.is_char_boundary(c.offset) {
            return Err(Error::Invalid(
                "caret is not at a UTF-8 boundary in its block".into(),
            ));
        }
        if self
            .current()?
            .block(c.node)
            .is_none_or(|b| b.lines.is_empty())
        {
            return Err(Error::NoLayout);
        }
        self.navigator()?
            .normalize(c)
            .filter(|normalized| normalized.offset == c.offset)
            .ok_or_else(|| Error::Invalid("caret is not at a laid-out cluster boundary".into()))
    }
    pub fn move_cursor(&self, request: &Payload<Move>) -> Result<Payload<Cursor>> {
        let r = validate(request)?;
        let caret = self.checked_caret(&r.cursor.caret)?;
        let cursor = self
            .navigator()?
            .move_cursor(
                &reprise_edit::Cursor {
                    caret,
                    goal_x: r.cursor.goal_x.map(cv::length),
                },
                cv::movement(r.movement),
            )
            .ok_or_else(|| Error::InvalidId(r.cursor.caret.node.clone()))?;
        Ok(Payload::new(Cursor {
            caret: cv::caret_out(cursor.caret),
            goal_x: cursor.goal_x.map(|x| x.0),
        }))
    }
    pub fn caret_rect(&self, request: &Payload<Caret>) -> Result<Payload<PageRect>> {
        let c = self.checked_caret(validate(request)?)?;
        let r = self.navigator()?.caret_rect(c).ok_or(Error::NoLayout)?;
        Ok(Payload::new(PageRect {
            page: r.page as u32,
            rect: cv::rect(r.rect),
        }))
    }
    pub fn hit_test(&self, request: &Payload<HitTest>) -> Result<Payload<Hit>> {
        let r = validate(request)?;
        let hit = self
            .navigator()?
            .hit(
                r.page as usize,
                reprise_geom::Point::new(cv::length(r.x), cv::length(r.y)),
            )
            .ok_or_else(|| Error::Invalid("hit page has no caret".into()))?;
        Ok(Payload::new(Hit {
            caret: cv::caret_out(hit.caret),
            page: hit.page as u32,
            inside: hit.inside,
        }))
    }
    fn selection(&self, r: &Selection) -> Result<reprise_edit::Selection> {
        Ok(reprise_edit::Selection {
            anchor: self.checked_caret(&r.anchor)?,
            focus: self.checked_caret(&r.focus)?,
        })
    }
    pub fn selection_rects(&self, request: &Payload<Selection>) -> Result<Payload<Vec<PageRect>>> {
        let selection = self.selection(validate(request)?)?;
        Ok(Payload::new(
            self.navigator()?
                .selection_rects(&selection)
                .into_iter()
                .map(|r| PageRect {
                    page: r.page as u32,
                    rect: cv::rect(r.rect),
                })
                .collect(),
        ))
    }
    pub fn copy(&self, request: &Payload<Selection>) -> Result<Payload<Bytes>> {
        let selection = self.selection(validate(request)?)?;
        let mut fragment = reprise_clipboard::copy_selection(
            self.doc(),
            &self.document_id,
            &selection,
            self.current()?,
            &self.engine.schemas,
            Some(&self.engine.fonts),
        )?;
        fragment.attach_images(&self.engine.assets)?;
        Ok(Payload::new(Bytes {
            bytes: fragment.encode()?,
        }))
    }
    /// Project a copied selection into plain text/HTML/native bytes with losses.
    pub fn copy_as(&self, request: &Payload<CopyAs>) -> Result<Payload<Exported>> {
        let r = validate(request)?;
        let selection = self.selection(&r.selection)?;
        let mut fragment = reprise_clipboard::copy_selection(
            self.doc(),
            &self.document_id,
            &selection,
            self.current()?,
            &self.engine.schemas,
            Some(&self.engine.fonts),
        )?;
        fragment.attach_images(&self.engine.assets)?;
        let doc = Document::new(1)?;
        ensure_base_style(&doc)?;
        let mut editor = reprise_edit::Editor::new(doc, self.engine.schemas.clone());
        let projected = fragment.paste(
            &mut editor,
            None,
            &format!("clipboard-projection:{}", self.document_id),
        )?;
        let mut layout = LayoutSession::new(&self.engine);
        let snapshot = layout.layout(editor.document())?;
        use reprise_clipboard::Exporter;
        let exporter: &dyn Exporter = match r.format {
            CopyFormat::PlainText => &reprise_clipboard::PlainText,
            CopyFormat::Html => &reprise_clipboard::Html,
        };
        let mut result = exporter.export(
            editor.document(),
            Some(&snapshot),
            &reprise_clipboard::ExportOptions {
                source_namespace: &self.document_id,
                schemas: &self.engine.schemas,
                fonts: Some(&self.engine.fonts),
            },
        )?;
        result.losses.notes.extend(projected.notes);
        Ok(Payload::new(cv::exported(result)))
    }
    pub fn resources(&self) -> Result<Payload<Vec<Resource>>> {
        let assets = self.package.assets()?;
        Ok(Payload::new(
            assets
                .needed
                .into_iter()
                .map(|a| Resource {
                    available: assets.bundled.contains_key(&a.id),
                    id: a.id,
                    kind: match a.kind {
                        reprise_format::AssetKind::Font => ResourceKind::Font,
                        reprise_format::AssetKind::Image => ResourceKind::Image,
                        reprise_format::AssetKind::Other => ResourceKind::Other,
                    },
                    hash: a.hash,
                    font: a.font.map(|f| Face {
                        family: f.face.family,
                        hash: f.face.hash,
                    }),
                    location: a.location.map(|l| match l {
                        reprise_format::ExternalLocation::Path(p) => ResourceLocation::Path(p),
                        reprise_format::ExternalLocation::Url(p) => ResourceLocation::Url(p),
                    }),
                })
                .collect(),
        ))
    }
    pub fn resource_bytes(&self, request: &Payload<String>) -> Result<Payload<Bytes>> {
        let id = validate(request)?;
        if id.len() > 1024 {
            return Err(Error::Limit("asset ID".into()));
        }
        let assets = self.package.assets()?;
        let bytes = assets
            .bundled
            .get(id)
            .ok_or_else(|| Error::InvalidId(id.clone()))?;
        Ok(Payload::new(Bytes {
            bytes: bytes.clone(),
        }))
    }

    fn paste_fragment(
        &mut self,
        fragment: reprise_clipboard::NativeFragment,
        at: Option<&Caret>,
    ) -> Result<Payload<Applied>> {
        // Validate the caret against authored bytes; paste is also legal without layout.
        let at = at
            .map(|c| cv::caret(c).map(|c| (c.node, c.offset)))
            .transpose()?;
        fragment.validate()?;
        // Resource installation must not fail after authored state has committed.
        let fonts = fragment
            .resources
            .values()
            .filter_map(|resource| {
                if let reprise_clipboard::ResourceKind::Font { declaration, .. } = &resource.kind {
                    Some(
                        reprise_font::Face::declared(
                            resource.bytes.clone(),
                            Some(declaration.clone()),
                        )
                        .map_err(|e| Error::Invalid(e.to_string())),
                    )
                } else {
                    None
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let mut assets = self.engine.assets.clone();
        fragment.install_assets(&mut assets)?;
        let result = fragment.paste(&mut self.editor, at, &self.document_id)?;
        for face in fonts {
            self.engine.fonts.add(face);
        }
        self.engine.assets = assets;
        self.reconfigure()?;
        Ok(Payload::new(cv::applied(result.applied, result.notes)))
    }
    pub fn paste(&mut self, request: &Payload<Paste>, bytes: &[u8]) -> Result<Payload<Applied>> {
        let r = validate(request)?;
        if bytes.len() > 96 * 1024 * 1024 {
            return Err(Error::Limit("clipboard bytes".into()));
        }
        self.paste_fragment(
            reprise_clipboard::NativeFragment::decode(bytes)?,
            r.at.as_ref(),
        )
    }
    pub fn import_text(&mut self, request: &Payload<TextImport>) -> Result<Payload<Applied>> {
        let r = validate(request)?;
        let imported = if r.html {
            reprise_clipboard::import_html(&r.text, reprise_clipboard::ImportLimits::default())?
        } else {
            reprise_clipboard::import_plain(&r.text)?
        };
        self.paste_fragment(imported.fragment, r.at.as_ref())
    }
    pub fn save(&mut self) -> Result<Payload<Bytes>> {
        // Embedding all used fonts requires current complete layout. Saving always
        // finishes the incremental coordinator, rather than dropping resources.
        if self.snapshot.is_none() {
            let opts = self.options.clone().unwrap_or_default();
            let mut job = self.start_layout(&Payload::new(opts))?;
            while !job.step(self, 100_000)?.data.complete {}
        }
        let mut package = self
            .package
            .with_document(self.doc(), PersistenceMode::History)?;
        package.embed_layout_fonts(self.current()?, &self.engine.fonts)?;
        package.embed_document_images(self.doc(), &self.engine.assets)?;
        let bytes = package.save()?;
        self.package = package;
        Ok(Payload::new(Bytes { bytes }))
    }
    pub fn export(&self, request: &Payload<ExportFormat>) -> Result<Payload<Exported>> {
        let format = validate(request)?;
        use reprise_clipboard::Exporter;
        let native = reprise_clipboard::NativeWithAssets {
            assets: &self.engine.assets,
        };
        let pdf = reprise_clipboard::PdfWithAssets {
            assets: &self.engine.assets,
        };
        let exporter: &dyn Exporter = match format {
            ExportFormat::PlainText => &reprise_clipboard::PlainText,
            ExportFormat::Html => &reprise_clipboard::Html,
            ExportFormat::Native => &native,
            ExportFormat::Pdf => &pdf,
        };
        let snapshot = if *format == ExportFormat::Pdf {
            Some(self.current()?)
        } else {
            self.snapshot.as_ref()
        };
        let result = exporter.export(
            self.doc(),
            snapshot,
            &reprise_clipboard::ExportOptions {
                source_namespace: &self.document_id,
                schemas: &self.engine.schemas,
                fonts: Some(&self.engine.fonts),
            },
        )?;
        Ok(Payload::new(Exported {
            content: Bytes {
                bytes: result.bytes,
            },
            losses: result
                .losses
                .features
                .into_iter()
                .map(|loss| Loss {
                    code: loss.code.to_string(),
                    disposition: match loss.disposition {
                        reprise_clipboard::Disposition::Preserved => Disposition::Preserved,
                        reprise_clipboard::Disposition::Approximated => Disposition::Approximated,
                        reprise_clipboard::Disposition::Dropped => Disposition::Dropped,
                    },
                    detail: loss.detail,
                })
                .collect(),
            diagnostics: result
                .losses
                .notes
                .into_iter()
                .map(crate::error::diagnostic)
                .collect(),
        }))
    }
    /// Separate read-only geometry; requires a complete layout at this revision.
    pub fn marks(&self, page: u32) -> Result<Payload<MarksPage>> {
        let s = self.current()?;
        if s.pages.get(page as usize).is_none() {
            return Err(Error::Invalid("page out of bounds".into()));
        }
        Ok(Payload::new(MarksPage {
            token: self.token(),
            page,
            marks: s.marks(page as usize).into_iter().map(cv::mark).collect(),
        }))
    }
    pub fn display_page(&self, page: u32) -> Result<Payload<DisplayPage>> {
        let s = self.current()?;
        if page as usize >= s.pages.len() {
            return Err(Error::Invalid("page out of bounds".into()));
        }
        Ok(Payload::new(DisplayPage {
            token: self.token(),
            page,
            settled: true,
            complete: true,
            display: cv::display(
                s.to_display_list(page as usize, reprise_layout::DisplayOptions::default())
                    .content_only(),
            )?,
        }))
    }
    /// Canonical compact JSON, shared byte for byte with WASM.
    pub fn display_json(&self, page: u32) -> Result<Payload<String>> {
        Ok(Payload::new(
            serde_json::to_string(&self.display_page(page)?.data.display)
                .map_err(|e| Error::Invalid(e.to_string()))?,
        ))
    }
    fn display_core(&self, page: u32) -> Result<reprise_display::DisplayList> {
        let s = self.current()?;
        if page as usize >= s.pages.len() {
            return Err(Error::Invalid("page out of bounds".into()));
        }
        Ok(
            s.to_display_list(page as usize, reprise_layout::DisplayOptions::default())
                .content_only(),
        )
    }
    /// Glyph outlines for drawing display lists without the engine (canvas,
    /// WebGL). See [`GlyphOutlines`].
    pub fn glyph_outlines(
        &self,
        request: &Payload<GlyphRequest>,
    ) -> Result<Payload<GlyphOutlines>> {
        let r = validate(request)?;
        if r.glyphs.len() > 4096 {
            return Err(Error::Limit("glyphs per outline request".into()));
        }
        let face = self
            .engine
            .fonts
            .get(&reprise_font::FaceId {
                family: r.face.family.clone(),
                hash: r.face.hash.clone(),
            })
            .map_err(|e| Error::Core {
                code: "bindings.missing-font".into(),
                severity: Severity::Error,
                message: e.to_string(),
                command: None,
            })?;
        Ok(Payload::new(GlyphOutlines {
            units_per_em: u32::from(face.metrics().units_per_em),
            glyphs: r
                .glyphs
                .iter()
                .map(|&id| GlyphOutline {
                    id,
                    path: outline_path(&face.outline(id)),
                })
                .collect(),
        }))
    }
    pub fn svg(&self, page: u32) -> Result<Payload<String>> {
        let list = self.display_core(page)?;
        Ok(Payload::new(
            reprise_display::svg::render_with_assets(
                &list,
                &self.engine.fonts,
                &self.engine.assets,
            )
            .map_err(|e| Error::Core {
                code: "bindings.render".into(),
                severity: Severity::Error,
                message: e.to_string(),
                command: None,
            })?,
        ))
    }
    /// Scale is integer permille pixels/point and affects rendering only.
    pub fn png(&self, page: u32, scale_permille: u32) -> Result<Payload<Bytes>> {
        let list = self.display_core(page)?;
        let scale = u64::from(scale_permille);
        let w = (i64::from(list.width.0).max(0) as u64)
            .saturating_mul(scale)
            .div_ceil(1_024_000);
        let h = (i64::from(list.height.0).max(0) as u64)
            .saturating_mul(scale)
            .div_ceil(1_024_000);
        if scale == 0 || scale > 16_000 || w.saturating_mul(h) > 16_000_000 {
            return Err(Error::Limit("raster pixels/scale".into()));
        }
        let bytes = reprise_display::png::render_with_assets(
            &list,
            &self.engine.fonts,
            &self.engine.assets,
            scale_permille as f32 / 1000.0,
        )
        .map_err(|e| Error::Core {
            code: "bindings.render".into(),
            severity: Severity::Error,
            message: e.to_string(),
            command: None,
        })?;
        Ok(Payload::new(Bytes { bytes }))
    }
    pub fn reading_order(&self) -> Result<Payload<Vec<ReadingStep>>> {
        Ok(Payload::new(
            self.current()?
                .reading_order(self.doc())
                .into_iter()
                .map(|s| ReadingStep {
                    node: s.line.node.to_string(),
                    line: s.line.line as u32,
                    page: s.page as u32,
                })
                .collect(),
        ))
    }
    pub fn diagnostics(&self) -> Payload<Vec<Diagnostic>> {
        let mut notes: Vec<_> = self
            .notes
            .iter()
            .cloned()
            .map(crate::error::diagnostic)
            .collect();
        if let Some(s) = &self.snapshot {
            notes.extend(s.diagnostics.iter().map(cv::layout_diagnostic));
        }
        Payload::new(notes)
    }
    pub fn sync_info(&self) -> Payload<SyncInfo> {
        Payload::new(SyncInfo {
            document_id: self.document_id.clone(),
            peer_id: self.peer_id.clone(),
            vector: self
                .doc()
                .version_vector()
                .into_iter()
                .map(|(p, c)| Clock {
                    peer: p.to_string(),
                    counter: c,
                })
                .collect(),
        })
    }
    /// v1 uses bounded self-contained history snapshots as update packets.
    /// A version vector is still exchanged; transport can suppress equal vectors.
    pub fn export_updates(&self) -> Result<Payload<SyncUpdate>> {
        let info = self.sync_info().data;
        Ok(Payload::new(SyncUpdate {
            document_id: info.document_id,
            from_peer: info.peer_id,
            vector: info.vector,
            content: Bytes {
                bytes: self.doc().try_export(PersistenceMode::History)?,
            },
        }))
    }
    pub fn import_updates(&mut self, request: &Payload<SyncUpdate>) -> Result<Payload<SyncInfo>> {
        let r = validate(request)?;
        if r.document_id != self.document_id {
            return Err(Error::InvalidId(r.document_id.clone()));
        }
        if r.from_peer == self.peer_id {
            return Err(Error::Invalid(
                "concurrent replicas must have distinct peers".into(),
            ));
        }
        cv::peer(&r.from_peer)?;
        if r.content.bytes.len() > 64 * 1024 * 1024 || r.vector.len() > 4096 {
            return Err(Error::Limit("sync bytes/vector".into()));
        }
        for clock in &r.vector {
            cv::peer(&clock.peer)?;
            if clock.counter < 0 {
                return Err(Error::Invalid("negative vector counter".into()));
            }
        }
        let doc = Document::import(&r.content.bytes, cv::peer(&self.peer_id)?)?;
        let actual: Vec<_> = doc
            .version_vector()
            .into_iter()
            .map(|(p, c)| Clock {
                peer: p.to_string(),
                counter: c,
            })
            .collect();
        if actual != r.vector {
            return Err(Error::Invalid(
                "update vector does not describe bytes".into(),
            ));
        }
        self.editor.merge(&doc)?;
        self.snapshot = None;
        Ok(self.sync_info())
    }
    pub fn awareness(&self, bytes: &[u8]) -> Result<Payload<Awareness>> {
        if bytes.len() > MAX_AWARENESS_BYTES {
            return Err(Error::Limit("awareness bytes".into()));
        }
        Ok(Payload::new(Awareness {
            document_id: self.document_id.clone(),
            peer_id: self.peer_id.clone(),
            content: Bytes {
                bytes: bytes.to_vec(),
            },
        }))
    }
    pub fn validate_awareness(&self, request: &Payload<Awareness>) -> Result<Payload<Bytes>> {
        let r = validate(request)?;
        if r.document_id != self.document_id {
            return Err(Error::InvalidId(r.document_id.clone()));
        }
        cv::peer(&r.peer_id)?;
        if r.content.bytes.len() > MAX_AWARENESS_BYTES {
            return Err(Error::Limit("awareness bytes".into()));
        }
        Ok(Payload::new(r.content.clone()))
    }
    pub fn load_plugin(&self, request: &Payload<PluginSpec>, bytes: &[u8]) -> Result<Plugin> {
        let r = validate(request)?;
        if bytes.len() > 1_048_576
            || r.functions.len() > 256
            || r.imports.len() > 2
            || r.grants.len() > 2
        {
            return Err(Error::Limit("plugin bytes/declarations".into()));
        }
        let mut manifest = reprise_plugin::Manifest::new(&r.name, &r.plugin_version, bytes);
        let hash: String = manifest
            .identity
            .sha256
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if hash != r.sha256 {
            return Err(Error::Core {
                code: "plugin.hash".into(),
                severity: Severity::Warning,
                message: "plugin pin does not match bytes".into(),
                command: None,
            });
        }
        manifest.imports = r.imports.iter().copied().map(cv::capability).collect();
        for f in &r.functions {
            if f.params.len() > 256 || manifest.functions.contains_key(&f.name) {
                return Err(Error::Invalid("plugin function declarations".into()));
            }
            manifest.functions.insert(
                f.name.clone(),
                reprise_plugin::FunctionDeclaration {
                    operation: f.operation,
                    signature: reprise_doc::function::Signature {
                        params: f.params.iter().copied().map(cv::dimension).collect(),
                        variadic: f.variadic.map(cv::dimension),
                        returns: cv::dimension(f.returns),
                    },
                },
            );
        }
        let plugin = reprise_plugin::Plugin::load(
            bytes,
            manifest,
            match r.phase {
                PluginPhase::Layout => reprise_plugin::Phase::Layout,
                PluginPhase::Editing => reprise_plugin::Phase::Editing,
            },
            r.grants.iter().copied().map(cv::capability).collect(),
            reprise_plugin::Limits {
                fuel: u64::from(r.fuel),
                memory_pages: r.memory_pages,
                table_elements: r.table_elements,
                buffer_bytes: r.buffer_bytes,
            },
        )
        .map_err(Error::note)?;
        Ok(Plugin { inner: plugin })
    }
    pub fn install_plugin(
        &mut self,
        plugin: &Plugin,
        request: &Payload<PluginInstall>,
    ) -> Result<Payload<bool>> {
        match validate(request)? {
            PluginInstall::Relation { schema, operation } => {
                let schema = cv::schema(schema)?;
                // Validate both registries before updating either.
                let mut registry = self.editor.schemas().clone();
                registry
                    .register(schema.clone())
                    .map_err(|e| Error::Invalid(e.to_string()))?;
                self.engine
                    .install_plugin_relation(
                        schema.clone(),
                        reprise_layout::plugins::RelationBinding {
                            plugin: Some(plugin.inner.clone()),
                            operation: *operation,
                        },
                    )
                    .map_err(|e| Error::Invalid(e.to_string()))?;
                self.editor
                    .register_schema(schema)
                    .map_err(|e| Error::Invalid(e.to_string()))?;
            }

            PluginInstall::Functions => self
                .engine
                .install_plugin_functions(&plugin.inner)
                .map_err(|e| Error::Invalid(e.to_string()))?,
            PluginInstall::Geometry { operation } => self
                .engine
                .install_plugin_geometry(
                    plugin.inner.clone(),
                    *operation,
                    Box::new(reprise_compose::Greedy),
                )
                .map_err(Error::note)?,
        };
        self.reconfigure()?;
        Ok(Payload::new(true))
    }
    /// Explicit node handles only; one staged plugin call becomes one transaction.
    pub fn run_plugin_edit(
        &mut self,
        plugin: &Plugin,
        request: &Payload<PluginEdit>,
        bytes: &[u8],
    ) -> Result<Payload<Applied>> {
        let r = validate(request)?;
        if r.nodes.len() > 256 || bytes.len() > 1_048_576 {
            return Err(Error::Limit("plugin edit input".into()));
        }
        let mut context = reprise_plugin::CallContext::default();
        let mut nodes = Vec::new();
        for (handle, node) in r.nodes.iter().enumerate() {
            let node = cv::id(node)?;
            context
                .texts
                .insert(handle as u32, self.doc().block(node)?.text.to_string());
            context.editable.insert(handle as u32);
            nodes.push(node);
        }
        struct Kernel<'a> {
            editor: &'a mut reprise_edit::Editor,
            nodes: Vec<reprise_doc::NodeId>,
            applied: Option<reprise_edit::Applied>,
        }
        impl reprise_plugin::EditKernel for Kernel<'_> {
            fn apply(
                &mut self,
                ins: &[reprise_plugin::TextInsertion],
            ) -> std::result::Result<(), reprise_diag::Note> {
                let mut commands = Vec::new();
                for i in ins {
                    let node = self.nodes.get(i.handle as usize).copied().ok_or_else(|| {
                        reprise_diag::Note::error(
                            reprise_diag::Code::new("bindings.id"),
                            "invalid plugin node handle",
                        )
                    })?;
                    commands.push(reprise_edit::Command::InsertText {
                        node,
                        at: i.at as usize,
                        text: i.text.clone(),
                    });
                }
                self.applied = Some(self.editor.apply(&commands.into()).map_err(|e| e.note())?);
                Ok(())
            }
        }
        let mut kernel = Kernel {
            editor: &mut self.editor,
            nodes,
            applied: None,
        };
        plugin
            .inner
            .edit(r.operation, bytes, &context, &mut kernel)
            .map_err(Error::note)?;
        let applied = kernel.applied.take().unwrap_or_default();
        self.snapshot = None;
        Ok(Payload::new(cv::applied(applied, Vec::new())))
    }
}
/// Loaded, independently granted plugin; its core runtime remains private.
pub struct Plugin {
    inner: reprise_plugin::Plugin,
}
/// An owned continuation. Its gate includes object identity, revision and generation.
pub struct LayoutJob {
    continuation: Option<LayoutContinuation>,
    owner: Weak<()>,
    token: LayoutToken,
    snapshot: Option<LayoutSnapshot>,
    progress: Option<LayoutProgress>,
    cancelled: bool,
}
impl LayoutJob {
    fn check(&self, session: &DocumentSession) -> Result<()> {
        if self.cancelled {
            return Err(Error::Cancelled);
        }
        if !self.owner.ptr_eq(&Rc::downgrade(&session.identity)) || self.token != session.token() {
            return Err(Error::Stale);
        }
        Ok(())
    }
    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.continuation = None;
        self.snapshot = None;
    }
    pub fn step(
        &mut self,
        session: &mut DocumentSession,
        budget: u32,
    ) -> Result<Payload<LayoutProgress>> {
        self.check(session)?;
        if budget > 100_000 {
            return Err(Error::Limit("job step budget".into()));
        }
        let continuation = self.continuation.take().ok_or(Error::Cancelled)?;
        let mut layout =
            LayoutSession::from_cache(&session.engine, std::mem::take(&mut session.cache));
        layout.set_workers(session.workers.clone());
        let mut job = layout.resume(session.doc(), continuation)?;
        let step = job.step(budget as usize)?;
        let view = job.partial()?;
        view.publish(session.doc())?;
        let coverage = view.coverage();
        let counters = job.counters();
        let progress = LayoutProgress {
            token: self.token.clone(),
            used: step.used as u32,
            pass: cv::pass(step.pass),
            viewport_ready: step.viewport_ready,
            complete: step.complete,
            settled: coverage.settled,
            outside_document: coverage.outside_document,
            pages: coverage.pages.iter().map(|p| *p as u32).collect(),
            counters: Counters {
                units: counters.units as u32,
                shapes: counters.shapes as u32,
                compositions: counters.compositions as u32,
                reused_compositions: counters.reused_compositions as u32,
            },
        };
        self.snapshot = Some(view.snapshot().clone());
        let complete = job.complete()?;
        self.continuation = Some(job.suspend());
        session.cache = layout.into_cache();
        if let Some(s) = complete {
            session.snapshot = Some(s);
        }
        self.progress = Some(progress.clone());
        Ok(Payload::new(progress))
    }
    pub fn display_page(
        &self,
        session: &DocumentSession,
        page: u32,
    ) -> Result<Payload<DisplayPage>> {
        self.check(session)?;
        let progress = self.progress.as_ref().ok_or(Error::NoLayout)?;
        let snapshot = self.snapshot.as_ref().ok_or(Error::NoLayout)?;
        if !progress.pages.contains(&page) {
            return Err(Error::Invalid("page outside partial coverage".into()));
        }
        Ok(Payload::new(DisplayPage {
            token: self.token.clone(),
            page,
            settled: progress.settled,
            complete: progress.complete,
            display: cv::display(
                snapshot
                    .to_display_list(page as usize, reprise_layout::DisplayOptions::default())
                    .content_only(),
            )?,
        }))
    }
}

/// SVG path data for a glyph outline, in font units with y up.
fn outline_path(commands: &[reprise_font::PathCmd]) -> String {
    use reprise_font::PathCmd;
    use std::fmt::Write;
    let mut out = String::new();
    for c in commands {
        let _ = match *c {
            PathCmd::Move(x, y) => write!(out, "M{x} {y}"),
            PathCmd::Line(x, y) => write!(out, "L{x} {y}"),
            PathCmd::Quad(a, b, x, y) => write!(out, "Q{a} {b} {x} {y}"),
            PathCmd::Cubic(a, b, c, d, x, y) => write!(out, "C{a} {b} {c} {d} {x} {y}"),
            PathCmd::Close => write!(out, "Z"),
        };
    }
    out
}
