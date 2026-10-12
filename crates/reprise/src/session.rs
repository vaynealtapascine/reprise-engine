mod collab;
use crate::convert as cv;
use crate::*;
use reprise_doc::{Document, PersistenceMode};
use reprise_layout::incremental::{LayoutCache, LayoutContinuation, LayoutSession};
use reprise_layout::{Engine, LayoutSnapshot};
use std::collections::BTreeMap;
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

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

/// Font state shared by every session of one [`Workspace`].
#[derive(Default)]
struct SharedFonts {
    /// Parsed once; sessions copy the `Arc`, never the bytes.
    faces: Vec<Arc<reprise_font::Face>>,
    settings: Option<reprise_font::FontSettings>,
    /// Bumped by every change; a session compares it with the one it last saw.
    generation: u64,
}
type Fonts = Arc<Mutex<SharedFonts>>;
fn lock(fonts: &Fonts) -> MutexGuard<'_, SharedFonts> {
    // Nothing panics while holding the lock; recover the data regardless.
    fonts.lock().unwrap_or_else(PoisonError::into_inner)
}
/// The most faces one workspace holds (a font menu is far smaller).
const MAX_WORKSPACE_FACES: usize = 256;

/// Workspace factory. Documents opened from one workspace share its declared
/// fonts and font settings; plugin configuration stays per document.
///
/// Fonts declared here are parsed once and held as shared handles, so opening
/// a document copies pointers instead of megabytes. A session also keeps its
/// own `declare_font` for a face that belongs to that document alone.
#[derive(Default)]
pub struct Workspace {
    fonts: Fonts,
}
impl Workspace {
    pub fn new() -> Self {
        Self::default()
    }
    /// Declares a font for every session of this workspace, present and
    /// future. Sessions pick it up when they next lay out, save or paste.
    /// Declaring the same bytes and descriptors again changes nothing.
    pub fn declare_font(
        &self,
        request: &Payload<FontDeclaration>,
        bytes: &[u8],
    ) -> Result<Payload<Face>> {
        let r = validate(request)?;
        if bytes.len() > 32 * 1024 * 1024 {
            return Err(Error::Limit("font bytes".into()));
        }
        let face = reprise_font::Face::declared(bytes.to_vec(), Some(cv::font_declaration(r)))
            .map_err(|e| Error::note(e.note()))?;
        let out = Face {
            family: face.id().family.clone(),
            hash: face.id().hash.clone(),
        };
        let mut shared = lock(&self.fonts);
        if !shared.faces.iter().any(|f| f.id() == face.id()) {
            if shared.faces.len() >= MAX_WORKSPACE_FACES {
                return Err(Error::Limit("workspace faces".into()));
            }
            shared.faces.push(Arc::new(face));
            shared.generation = shared.generation.saturating_add(1);
        }
        Ok(Payload::new(out))
    }
    /// Default font settings for every session of this workspace. A session
    /// that calls its own `set_font_settings` stops following them.
    pub fn set_font_settings(&self, request: &Payload<FontSettings>) -> Result<Payload<()>> {
        let settings = cv::font_settings(validate(request)?);
        settings.validate().map_err(Error::note)?;
        let mut shared = lock(&self.fonts);
        shared.settings = Some(settings);
        shared.generation = shared.generation.saturating_add(1);
        Ok(Payload::new(()))
    }
    /// How many holders share `face`: one for the workspace plus one for each
    /// session that has picked it up. `None` when the workspace has no such
    /// face. A count rather than a timing, for tests and diagnostics.
    pub fn font_handles(&self, face: &Face) -> Option<usize> {
        lock(&self.fonts)
            .faces
            .iter()
            .find(|f| f.id().family == face.family && f.id().hash == face.hash)
            .map(Arc::strong_count)
    }
    fn adopt(&self, mut session: DocumentSession) -> Result<DocumentSession> {
        session.shared = Some(self.fonts.clone());
        session.sync_fonts()?;
        Ok(session)
    }
    pub fn create(&self, request: &Payload<Create>) -> Result<DocumentSession> {
        let r = validate(request)?;
        let id = cv::document_id(&r.document_id)?;
        let peer = cv::peer(&r.peer_id)?;
        let doc = Document::new(peer)?;
        ensure_base_style(&doc)?;
        let package = reprise_format::Package::new(&doc, id, PersistenceMode::History)?;
        self.adopt(DocumentSession::new(
            doc,
            package,
            r.document_id.clone(),
            r.peer_id.clone(),
            Engine::new(reprise_font::FontStore::default()),
            Vec::new(),
        ))
    }
    /// Imports a package as a new, independent document. See `docs/plumbing.md`.
    ///
    /// *Kept:* text and formatting, the block tree, every named style, page
    /// setup and templates, relations, ranges, tables, images, bundled assets,
    /// font pins, package settings, extension bytes and unknown sections.
    /// *Dropped:* CRDT history, undo, presence, the source document id and
    /// peers. The content is replayed into a fresh CRDT document authored by
    /// `peer_id`, so the copy shares no operation with the source: a source
    /// delta cannot apply to it, and a packet naming the source is refused
    /// with `bindings.copy-source`. The source bytes are not modified.
    pub fn import_copy(
        &self,
        request: &Payload<ImportCopy>,
        bytes: &[u8],
    ) -> Result<DocumentSession> {
        let r = validate(request)?;
        let id = cv::document_id(&r.document_id)?;
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
        if opened.is_read_only() {
            return Err(Error::ReadOnly);
        }
        let source = opened.package().clone();
        if source.document_id() == id {
            return Err(Error::InvalidId(r.document_id.clone()));
        }
        let source_id: String = source
            .document_id()
            .0
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let mut engine = Engine::new(reprise_font::FontStore::default());
        opened.restore_fonts(&mut engine.fonts);
        opened
            .notes
            .extend(opened.assets.restore_images(&mut engine.assets));
        let mut notes = opened.notes.clone();
        let old = opened.into_document()?;
        // A duplicate keeps every annotation, comment anchors included.
        let fragment = old
            .copy_document_fragment(&source_id, &engine.schemas)
            .map_err(|e| Error::Invalid(e.to_string()))?;
        // Every named style first, so unreferenced ones survive and the
        // fragment's own styles are identical, not clashing. Each live style
        // is written as its effective definition (nested and flat patches
        // folded together); an untouched built-in is left to the engine so
        // the copy needs no more format features than its source.
        let fresh = Document::new(peer)?;
        let (names, cut) = old.style_names();
        if cut {
            return Err(Error::Limit("named styles".into()));
        }
        for name in &names {
            let Some(record) = old.style_record(name) else {
                continue;
            };
            let builtin = record
                .builtin
                .then(|| reprise_doc::builtin_style(name))
                .flatten();
            let Some(base) = builtin else {
                fresh.define_style(name, &record.style)?;
                continue;
            };
            // A patched built-in stays a patch, so it keeps following the
            // engine's definition for the properties it did not change.
            let mut patch = reprise_doc::StylePatch::default();
            for property in reprise_doc::STORED_KEYS {
                match (record.style.stored(property), base.stored(property)) {
                    (mine, theirs) if mine == theirs => {}
                    (Some(raw), _) => patch.set.apply_stored(property, raw),
                    (None, _) => {
                        patch.clear.insert(property.to_string());
                    }
                }
            }
            if patch != reprise_doc::StylePatch::default() {
                fresh.patch_named_style(name, &patch)?;
            }
        }
        // A built-in the source deleted or renamed away stays gone.
        for name in reprise_doc::BUILTIN_STYLES {
            if old
                .style_record(name)
                .is_some_and(|r| r.status != reprise_doc::StyleStatus::Live)
            {
                fresh.delete_named_style(name)?;
            }
        }
        fresh.commit();
        let mut editor = reprise_edit::Editor::new(fresh, engine.schemas.clone());
        let pasted = editor
            .paste(&fragment, None, &r.document_id)
            .map_err(Error::from)?;
        notes.extend(pasted.notes);
        let doc = editor.into_document();
        doc.commit();
        ensure_base_style(&doc)?;
        let mut package = reprise_format::Package::new(&doc, id, PersistenceMode::History)?;
        use reprise_format::{Asset, FontPin, ids};
        let sections = &source.container().sections;
        package.set_settings(&source.settings()?)?;
        package.set_optional_features(source.container().header.features.optional)?;
        if let Some(raw) = sections.get(&ids::FONTS) {
            let pins: Vec<FontPin> =
                serde_json::from_slice(&raw.bytes).map_err(|e| Error::Invalid(e.to_string()))?;
            package.set_fonts(pins)?;
        }
        if let Some(raw) = sections.get(&ids::ASSETS) {
            let assets: Vec<Asset> =
                serde_json::from_slice(&raw.bytes).map_err(|e| Error::Invalid(e.to_string()))?;
            let bundles: BTreeMap<_, _> = sections
                .iter()
                .filter(|(id, _)| (ids::FIRST_ASSET..=ids::LAST_ASSET).contains(id))
                .map(|(id, s)| (*id, s.bytes.clone()))
                .collect();
            package.set_assets(assets, bundles)?;
        }
        if let Some(extensions) = source.extensions() {
            package.set_extensions(extensions.to_vec())?;
        }
        for (section, content) in sections {
            match package.set_unknown_section(*section, content.clone()) {
                // Known and derived sections were handled or are rebuilt.
                Ok(()) | Err(reprise_format::FormatError::Metadata(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }
        let mut session = DocumentSession::new(
            doc,
            package,
            r.document_id.clone(),
            r.peer_id.clone(),
            engine,
            notes,
        );
        session.copied_from = Some(source_id);
        self.adopt(session)
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
        self.adopt(DocumentSession::new(
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
    /// The workspace's shared fonts, and the generation of them this session
    /// has taken. `settings_local` is set once the session chose its own.
    shared: Option<Fonts>,
    shared_seen: u64,
    settings_local: bool,
    /// The document this one was copied from (`import_copy`), if any.
    copied_from: Option<String>,
    /// Resolved paragraph styles for `state`, per (style, overrides), valid
    /// for one revision and session generation.
    style_cache: std::cell::RefCell<StyleCache>,
}

/// Paragraph styles resolved for `State` (`docs/paragraph-styles.md`):
/// blocks sharing a style and overrides resolve once per revision.
#[derive(Default)]
struct StyleCache {
    key: Option<(reprise_doc::Revision, u64)>,
    resolved: BTreeMap<(Option<String>, String), ResolvedStyle>,
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
            shared: None,
            shared_seen: 0,
            settings_local: false,
            copied_from: None,
            style_cache: Default::default(),
        }
    }
    /// Takes the workspace's font changes since last time, sharing its faces
    /// by handle. Invalidates layout only when something changed.
    fn sync_fonts(&mut self) -> Result<()> {
        let Some(shared) = self.shared.clone() else {
            return Ok(());
        };
        let shared = lock(&shared);
        if shared.generation == self.shared_seen {
            return Ok(());
        }
        for face in &shared.faces {
            let held = self
                .engine
                .fonts
                .get(face.id())
                .is_ok_and(|have| Arc::ptr_eq(have, face));
            if !held {
                self.engine.fonts.add_shared(face.clone());
            }
        }
        if !self.settings_local
            && let Some(settings) = &shared.settings
            && self.engine.fonts.settings() != settings
        {
            self.engine
                .fonts
                .set_settings(settings.clone())
                .map_err(Error::note)?;
        }
        self.shared_seen = shared.generation;
        self.reconfigure()
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
        let mut cache = self.style_cache.borrow_mut();
        let key = (self.doc().revision(), self.generation);
        if cache.key.as_ref() != Some(&key) {
            *cache = StyleCache {
                key: Some(key),
                resolved: BTreeMap::new(),
            };
        }
        let context = self.engine.paragraph_context(self.doc());
        let mut table: Vec<ResolvedStyle> = Vec::new();
        let mut indices: BTreeMap<(Option<String>, String), u32> = BTreeMap::new();
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
                style: {
                    let overrides = serde_json::to_string(&block.overrides).unwrap_or_default();
                    let k = (block.style.clone(), overrides);
                    let index = match indices.get(&k) {
                        Some(i) => *i,
                        None => {
                            let resolved = cache
                                .resolved
                                .entry(k.clone())
                                .or_insert_with(|| {
                                    let used = reprise_doc::specify_parts(
                                        self.doc(),
                                        block.style.clone(),
                                        &block.overrides,
                                    )
                                    .compute(&self.engine.functions)
                                    .used(&context, &self.engine.functions)
                                    .style;
                                    cv::resolved_style(&used)
                                })
                                .clone();
                            let i = u32::try_from(table.len()).unwrap_or(u32::MAX);
                            table.push(resolved);
                            indices.insert(k, i);
                            i
                        }
                    };
                    BlockStyle {
                        name: block
                            .style
                            .as_deref()
                            .map(|n| self.doc().current_style_name(n))
                            .unwrap_or_default(),
                        direct: cv::style_out(&block.overrides),
                        resolved: index,
                    }
                },
                // Records that cannot be read are reported by layout diagnostics,
                // so an unreadable object is left out here, not failed on.
                inline: self
                    .doc()
                    .inline_objects(node)
                    .unwrap_or_default()
                    .into_iter()
                    .map(cv::inline_entry)
                    .collect(),
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
        drop(cache);
        let (names, cut) = self.doc().style_names();
        if cut {
            diagnostics.push(Diagnostic {
                code: "bindings.limit".into(),
                severity: Severity::Warning,
                message: format!(
                    "more than {} named styles; the listing is cut short",
                    reprise_doc::MAX_STYLES
                ),
                subject: None,
                start: None,
                end: None,
            });
        }
        let view = self.doc().annotation_view();
        let mut annotations: Vec<AnnotationBrief> = view
            .iter()
            .filter_map(cv::annotation_brief)
            .take(MAX_STATE_ANNOTATIONS.saturating_add(1))
            .collect();
        if annotations.len() > MAX_STATE_ANNOTATIONS {
            annotations.truncate(MAX_STATE_ANNOTATIONS);
            diagnostics.push(Diagnostic {
                code: "bindings.limit".into(),
                severity: Severity::Warning,
                message: format!(
                    "more than {MAX_STATE_ANNOTATIONS} annotations; State lists the first, ask the annotation queries for the rest"
                ),
                subject: None,
                start: None,
                end: None,
            });
        }
        let styles = names
            .into_iter()
            .filter_map(|name| {
                let record = self.doc().style_record(&name)?;
                Some(NamedStyle {
                    parent: record.style.parent.clone(),
                    builtin: record.builtin,
                    style: cv::style_out(&record.style),
                    name,
                })
            })
            .collect();
        Ok(Payload::new(State {
            annotations,
            styles,
            resolved: table,
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
        let applied = self.apply_commands(&r.commands)?;
        Ok(Payload::new(cv::applied(applied, Vec::new())))
    }
    fn apply_commands(&mut self, commands: &[Command]) -> Result<reprise_edit::Applied> {
        if commands.len() > reprise_edit::MAX_COMMANDS {
            return Err(Error::Limit("transaction commands".into()));
        }
        let mut bytes = 0usize;
        for command in commands {
            let added = match command {
                Command::InsertText { text, .. } | Command::InsertBlock { text, .. } => text.len(),
                // Inline parameters count too, each entry's name and value (and one
                // byte, so empty names still cost something).
                Command::InsertInline { object, .. } => {
                    object
                        .params
                        .iter()
                        .fold(object.kind.len(), |n, (name, value)| {
                            n.saturating_add(name.len())
                                .saturating_add(value.len())
                                .saturating_add(1)
                        })
                }
                Command::UpdateInline { patch, .. } => {
                    patch.params.iter().fold(0usize, |n, (name, value)| {
                        n.saturating_add(name.len())
                            .saturating_add(value.as_ref().map_or(0, String::len))
                            .saturating_add(1)
                    })
                }
                _ => 0,
            };
            bytes = bytes.saturating_add(added);
        }
        if bytes > reprise_edit::MAX_TRANSACTION_BYTES {
            return Err(Error::Limit("transaction text".into()));
        }
        let kernel = commands
            .iter()
            .map(|c| cv::command(c, self.engine.medium))
            .collect::<Result<Vec<_>>>()?;
        let applied = self.editor.apply(&kernel.into())?;
        self.snapshot = None;
        Ok(applied)
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
        let declaration = cv::font_declaration(r);
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
    /// Replace host font settings and invalidate revision-bound layout jobs.
    pub fn set_font_settings(&mut self, request: &Payload<FontSettings>) -> Result<Payload<()>> {
        let r = validate(request)?;
        let settings = cv::font_settings(r);
        settings.validate().map_err(Error::note)?;
        self.settings_local = true;
        self.reconfigure()?;
        self.engine
            .fonts
            .set_settings(settings)
            .map_err(Error::note)?;
        Ok(Payload::new(()))
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
        self.sync_fonts()?;
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
    /// Page geometry of the atom at a UTF-8 marker offset, from current layout.
    pub fn atom_rect(&self, request: &Payload<FieldTarget>) -> Result<Payload<PageRect>> {
        let r = validate(request)?;
        let node = cv::id(&r.node)?;
        let atom = self
            .navigator()?
            .atom_rect(node, r.at as usize)
            .ok_or_else(|| Error::Invalid("no placed inline object at that offset".into()))?;
        Ok(Payload::new(PageRect {
            page: u32::try_from(atom.page).unwrap_or(u32::MAX),
            rect: cv::rect(atom.rect),
        }))
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
        self.sync_fonts()?;
        let (fonts, assets) = self.preflight_resources(&fragment)?;
        let result = fragment.paste(&mut self.editor, at, &self.document_id)?;
        self.install_resources(fonts, assets)?;
        Ok(Payload::new(cv::applied(result.applied, result.notes)))
    }
    /// Resource installation must not fail after authored state has committed,
    /// so everything fallible happens before the edit.
    fn preflight_resources(
        &self,
        fragment: &reprise_clipboard::NativeFragment,
    ) -> Result<(Vec<reprise_font::Face>, reprise_display::AssetStore)> {
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
        Ok((fonts, assets))
    }
    fn install_resources(
        &mut self,
        fonts: Vec<reprise_font::Face>,
        assets: reprise_display::AssetStore,
    ) -> Result<()> {
        for face in fonts {
            self.engine.fonts.add(face);
        }
        self.engine.assets = assets;
        self.reconfigure()
    }
    fn replace_fragment(
        &mut self,
        fragment: reprise_clipboard::NativeFragment,
        selection: &StableSelection,
    ) -> Result<Payload<Replaced>> {
        let stable = collab::selection_in(selection)
            .ok_or_else(|| Error::Invalid("malformed stable selection".into()))?;
        fragment.validate()?;
        self.sync_fonts()?;
        let (fonts, assets) = self.preflight_resources(&fragment)?;
        let staged = self
            .editor
            .stage_replacement(&fragment.fragment, &self.document_id)?;
        let mut result = self.editor.replace_selection(&stable, &staged)?;
        self.install_resources(fonts, assets)?;
        let caret = result.caret.map(|(node, offset)| reprise_edit::Caret {
            node,
            offset,
            affinity: reprise_edit::Affinity::Downstream,
        });
        if self.selection.is_some() {
            self.selection = caret.and_then(|c| {
                reprise_edit::StableSelection::anchor(
                    self.doc(),
                    &reprise_edit::Selection::collapsed(c),
                )
                .ok()
            });
        }
        result.notes.extend(fragment.notes);
        Ok(Payload::new(Replaced {
            applied: cv::applied(result.applied, result.notes),
            selection: caret.map(|c| collab::selection_dto(reprise_edit::Selection::collapsed(c))),
        }))
    }
    /// Replaces an anchored selection with a native clipboard fragment in one
    /// transaction and one undo step. The selection is resolved inside the
    /// transaction, so a remote edit cannot land between the delete and the
    /// insert; an unreadable fragment is refused before anything is deleted.
    pub fn replace_selection(
        &mut self,
        request: &Payload<ReplaceSelection>,
        bytes: &[u8],
    ) -> Result<Payload<Replaced>> {
        let r = validate(request)?;
        if bytes.len() > 96 * 1024 * 1024 {
            return Err(Error::Limit("clipboard bytes".into()));
        }
        self.replace_fragment(
            reprise_clipboard::NativeFragment::decode(bytes)?,
            &r.selection,
        )
    }
    /// [`DocumentSession::replace_selection`] with plain text or HTML.
    pub fn replace_text(&mut self, request: &Payload<ReplaceText>) -> Result<Payload<Replaced>> {
        let r = validate(request)?;
        let imported = if r.html {
            reprise_clipboard::import_html(&r.text, reprise_clipboard::ImportLimits::default())?
        } else {
            reprise_clipboard::import_plain(&r.text)?
        };
        self.replace_fragment(imported.fragment, &r.selection)
    }
    /// Several commands as one transaction and one undo step, all or nothing:
    /// every command is validated against the document as the commands before
    /// it leave it, and nothing is written unless all pass (the error names
    /// the first refused command). `expected_revision` makes it a compare and
    /// set; `selection` comes back mapped through the batch's effects.
    pub fn batch(&mut self, request: &Payload<Batch>) -> Result<Payload<BatchReport>> {
        let r = validate(request)?;
        if r.commands.is_empty() {
            return Err(Error::Invalid("empty batch".into()));
        }
        if let Some(expected) = &r.expected_revision
            && *expected != cv::revision(&self.doc().revision())
        {
            return Err(Error::Stale);
        }
        let selection = match &r.selection {
            Some(s) => Some((cv::caret(&s.anchor)?, cv::caret(&s.focus)?)),
            None => None,
        };
        let applied = self.apply_commands(&r.commands)?;
        let mapped = selection.and_then(|(a, f)| {
            let map = |c: reprise_edit::Caret| {
                let bias = match c.affinity {
                    reprise_edit::Affinity::Downstream => reprise_edit::Bias::After,
                    reprise_edit::Affinity::Upstream => reprise_edit::Bias::Before,
                };
                applied
                    .map_position(c.node, c.offset, bias)
                    .map(|(node, offset)| reprise_edit::Caret { node, offset, ..c })
            };
            Some(collab::selection_dto(reprise_edit::Selection {
                anchor: map(a)?,
                focus: map(f)?,
            }))
        });
        Ok(Payload::new(BatchReport {
            applied: cv::applied(applied, Vec::new()),
            selection: mapped,
        }))
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
    /// The persistent `target` for a `page-of` field: the paragraph `node` at
    /// byte `at`, which follows later splits, joins and edits. Store the
    /// returned string as the `target` parameter of a field object, such as
    /// `InlineObject { kind: "field", params: { field: "page-of", target } }`
    /// in an `InsertInline` or `UpdateInline` command. Errors when `node` is
    /// unknown or `at` is not a character boundary of its text.
    pub fn field_target(&self, request: &Payload<FieldTarget>) -> Result<Payload<String>> {
        let r = validate(request)?;
        let node = cv::id(&r.node)?;
        let target = self.doc().field_target(node, r.at as usize)?;
        Ok(Payload::new(target))
    }
    pub fn save(&mut self) -> Result<Payload<Bytes>> {
        self.sync_fonts()?;
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
            links: s
                .page_links(self.doc(), page as usize)
                .links
                .iter()
                .map(cv::page_link)
                .collect(),
        }))
    }
    /// The links drawn on a page, for hit-testing and hover cards (the same
    /// list `display_page` carries). Requires a complete layout at this revision.
    pub fn page_links(&self, page: u32) -> Result<Payload<PageLinks>> {
        let s = self.current()?;
        if page as usize >= s.pages.len() {
            return Err(Error::Invalid("page out of bounds".into()));
        }
        let links = s.page_links(self.doc(), page as usize);
        Ok(Payload::new(PageLinks {
            token: self.token(),
            page,
            truncated: links.truncated,
            links: links.links.iter().map(cv::page_link).collect(),
        }))
    }
    /// One annotation with its payload, or `null` when there is none (or its
    /// payload cannot be read, or a link lost all its text).
    pub fn annotation(&self, request: &Payload<String>) -> Result<Payload<Option<Annotation>>> {
        let id = cv::range_id_of(validate(request)?)?;
        Ok(Payload::new(
            self.doc()
                .annotation(id)
                .and_then(|a| cv::annotation_out(&a)),
        ))
    }
    /// The annotations over a byte of a block (`start <= at < end`), and the
    /// orphans at it, in start order. At most 4,096.
    pub fn annotations_at(
        &self,
        request: &Payload<AnnotationsAt>,
    ) -> Result<Payload<AnnotationList>> {
        let r = validate(request)?;
        let node = cv::id(&r.node)?;
        Ok(Payload::new(cv::annotation_list(
            self.doc().annotations_at(node, r.at as usize),
        )))
    }
    /// The annotations overlapping a byte range of a block, in start order.
    pub fn annotations_in(
        &self,
        request: &Payload<AnnotationsIn>,
    ) -> Result<Payload<AnnotationList>> {
        let r = validate(request)?;
        let node = cv::id(&r.node)?;
        Ok(Payload::new(cv::annotation_list(
            self.doc()
                .annotations_in(node, r.start as usize..r.end as usize),
        )))
    }
    /// All annotations of a kind that are not gone, in ID order. At most 4,096.
    pub fn annotations_of_kind(
        &self,
        request: &Payload<AnnotationKind>,
    ) -> Result<Payload<AnnotationList>> {
        let kind = cv::annotation_kind(*validate(request)?);
        Ok(Payload::new(cv::annotation_list(
            self.doc().annotations_of_kind(kind),
        )))
    }
    /// Where an annotation is on the pages: rectangles per line fragment and
    /// a top y for a sidebar. `null` when there is no such annotation.
    /// Requires a complete layout at this revision.
    pub fn annotation_geometry(
        &self,
        request: &Payload<String>,
    ) -> Result<Payload<Option<AnnotationGeometry>>> {
        let id = cv::range_id_of(validate(request)?)?;
        let s = self.current()?;
        Ok(Payload::new(
            s.annotation_geometry(self.doc(), id)
                .map(|g| cv::annotation_geometry(&id, &g)),
        ))
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
        self.outlines(&r.face, &r.glyphs, false, false)
    }
    pub fn glyph_outlines_styled(
        &self,
        request: &Payload<StyledGlyphRequest>,
    ) -> Result<Payload<GlyphOutlines>> {
        let r = validate(request)?;
        self.outlines(&r.face, &r.glyphs, r.synthetic_oblique, true)
    }
    fn outlines(
        &self,
        face: &Face,
        glyphs: &[u32],
        synthetic_oblique: bool,
        include_bounds: bool,
    ) -> Result<Payload<GlyphOutlines>> {
        if glyphs.len() > 4096 {
            return Err(Error::Limit("glyphs per outline request".into()));
        }
        let face = self
            .engine
            .fonts
            .get(&reprise_font::FaceId {
                family: face.family.clone(),
                hash: face.hash.clone(),
            })
            .map_err(|e| Error::Core {
                code: "bindings.missing-font".into(),
                severity: Severity::Error,
                message: e.to_string(),
                command: None,
            })?;
        Ok(Payload::new(GlyphOutlines {
            units_per_em: u32::from(face.metrics().units_per_em),
            glyphs: glyphs
                .iter()
                .map(|&id| {
                    let path = outline_path(&face.outline_oblique(id, synthetic_oblique));
                    let ink_bounds = (include_bounds && !path.is_empty())
                        .then(|| {
                            face.design_bounds().map(|[x_min, y_min, x_max, y_max]| {
                                let min_shear = if synthetic_oblique {
                                    y_min.div_euclid(4)
                                } else {
                                    0
                                };
                                let max_shear = if synthetic_oblique {
                                    y_max.saturating_add(3).div_euclid(4)
                                } else {
                                    0
                                };
                                GlyphBounds {
                                    x_min: x_min.saturating_add(min_shear),
                                    y_min,
                                    x_max: x_max.saturating_add(max_shear),
                                    y_max,
                                }
                            })
                        })
                        .flatten();
                    GlyphOutline {
                        id,
                        path,
                        ink_bounds,
                    }
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
        let layout = self.current()?;
        let steps = layout
            .reading_order(self.doc())
            .into_iter()
            .map(|s| {
                let line = layout
                    .line(s.line)
                    .ok_or_else(|| Error::Invalid("reading step names a missing line".into()))?;
                Ok(ReadingStep {
                    node: s.line.node.to_string(),
                    line: u32::try_from(s.line.line).unwrap_or(u32::MAX),
                    page: u32::try_from(s.page).unwrap_or(u32::MAX),
                    start: u32::try_from(line.text.start).unwrap_or(u32::MAX),
                    end: u32::try_from(line.text.end).unwrap_or(u32::MAX),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Payload::new(steps))
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
        // Annotations this engine could not read or apply (a hostile or
        // newer peer's), and the limit.
        notes.extend(
            self.doc()
                .annotation_notes()
                .into_iter()
                .map(crate::error::diagnostic),
        );
        Payload::new(notes)
    }
    /// A bounded, filtered view of [`DocumentSession::diagnostics`], so a host
    /// can ask about one relation or one code without a full dump. Order is the
    /// order of `diagnostics()`.
    pub fn diagnostics_for(
        &self,
        request: &Payload<DiagnosticFilter>,
    ) -> Result<Payload<DiagnosticList>> {
        let f = validate(request)?;
        let rank = |s: Severity| match s {
            Severity::Info => 0,
            Severity::Warning => 1,
            Severity::Error => 2,
        };
        let limit = f.limit.unwrap_or(MAX_DIAGNOSTICS).min(MAX_DIAGNOSTICS) as usize;
        let mut out = Vec::new();
        let mut truncated = false;
        for d in self.diagnostics().data {
            let subject_ok = f.subject.as_ref().is_none_or(|want| {
                d.subject.as_deref().is_some_and(|have| {
                    have == want || have.split_once(':').is_some_and(|(_, id)| id == want)
                })
            });
            if !subject_ok
                || f.code_prefix
                    .as_ref()
                    .is_some_and(|p| !d.code.starts_with(p.as_str()))
                || f.min_severity.is_some_and(|m| rank(d.severity) < rank(m))
            {
                continue;
            }
            if out.len() >= limit {
                truncated = true;
                break;
            }
            out.push(d);
        }
        Ok(Payload::new(DiagnosticList {
            diagnostics: out,
            truncated,
        }))
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
        self.refuse_source(&r.document_id)?;
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
            links: snapshot
                .page_links(session.doc(), page as usize)
                .links
                .iter()
                .map(cv::page_link)
                .collect(),
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
