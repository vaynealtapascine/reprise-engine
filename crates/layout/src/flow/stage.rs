//! Staged preparation. `prepare_with` is split at its expensive steps (style
//! resolution, itemisation, each shaping request, break analysis) so a job can
//! run them in separate units and on workers. `prepare_with` runs the stages
//! back to back, so both paths make the same computations in the same order
//! of bookkeeping. See `docs/incremental.md` ("Work units and the budget").

use super::*;
use crate::shaping::{self, Request};
use crate::workers::{Serial, Workers};
use reprise_compose::Break;
use reprise_shape::ShapedGlyph;

/// The result of the first stage: either preparation already finished (a
/// failure, an image, or a shaping memo hit), or shaping work remains.
pub(crate) enum Begin {
    Done(Option<Box<Prepared>>, Vec<Diagnostic>),
    Shape(Box<Staged>),
}

/// A paragraph whose style is resolved and whose shaping is in progress.
/// Everything here is owned, so it survives a yield.
#[derive(Clone)]
pub(crate) struct Staged {
    node: NodeId,
    kind: BlockKind,
    style: ComputedStyle,
    vertical: bool,
    text: String,
    styles: Vec<StyleRun>,
    fallback: Option<(FaceId, Length)>,
    shape_key: Option<crate::incremental::ShapingKey>,
    /// Every note so far, in the order `prepare_with` would report them.
    notes: Vec<Diagnostic>,
    /// Where the shaping memo's notes start in `notes`.
    shape_notes: usize,
    itemized: Option<Itemized>,
    requests: Vec<Request>,
    glyphs: Vec<Option<Vec<ShapedGlyph>>>,
    breaks: Option<Vec<Break>>,
}

#[derive(Clone)]
struct Itemized {
    items: Vec<Item>,
    levels: Vec<u8>,
    base_level: u8,
}

/// What a finished preparation did, for the caller's bookkeeping.
pub(crate) struct Finished {
    pub value: Option<Prepared>,
    pub notes: Vec<Diagnostic>,
    /// Itemisation ran (rather than a shaping memo hit or an early failure).
    pub itemized: bool,
}

/// Style resolution and the shaping memo lookup: everything in
/// `prepare_with` before itemisation.
pub(crate) fn begin(
    engine: &Engine,
    doc: &Document,
    node: NodeId,
    ctx: &ResolutionContext,
    evaluation: Option<&crate::incremental::Evaluation>,
) -> Begin {
    let mut notes = Vec::new();
    let subject = Subject::Node(node);
    let block = match doc.block(node) {
        Ok(block) => block,
        Err(error) => {
            notes.push(Diagnostic::new(
                Severity::Error,
                codes::MALFORMED_BLOCK,
                subject,
                error.to_string(),
            ));
            return Begin::Done(None, notes);
        }
    };
    if let Some(e) = evaluation {
        e.style_resolution();
    }
    let style = match doc.computed_style_with(node, ctx, &engine.functions) {
        Ok(s) => s,
        Err(e) => {
            notes.push(Diagnostic::new(
                Severity::Error,
                codes::STYLE,
                subject,
                e.to_string(),
            ));
            return Begin::Done(None, notes);
        }
    };
    notes.extend(
        style
            .notes
            .iter()
            .cloned()
            .map(|note| Diagnostic::from_note(note, subject.clone())),
    );
    for property in &style.clamped {
        notes.push(Diagnostic::new(
            Severity::Warning,
            codes::STYLE_CLAMPED,
            subject.clone(),
            format!("{property} resolved to a negative length; clamped to zero"),
        ));
    }
    let text = block.text.to_string();
    if block.kind == BlockKind::Image {
        let width = match ctx.block.width {
            reprise_doc::context::Resolved::Definite(w) => w,
            _ => Length::MAX,
        };
        let image = crate::image::prepare(engine, doc, node, style.size, width, &mut notes);
        return Begin::Done(
            Some(Box::new(Prepared {
                node,
                kind: block.kind,
                style,
                text,
                items: Vec::new(),
                levels: Vec::new(),
                base_level: 0,
                shaped: ShapedText { runs: Vec::new() },
                breaks: Vec::new(),
                fallback: None,
                image: Some(image),
            })),
            notes,
        );
    }
    let styles = vec![StyleRun {
        range: 0..text.len(),
        families: if style.families.is_empty() {
            vec![style.family.clone()]
        } else {
            style.families.clone()
        },
        size: style.size,
        language: None,
        features: Vec::new(),
    }];
    let fallback = if style.families.is_empty() {
        engine
            .fonts
            .by_family(&style.family)
            .map(|f| (f.id().clone(), style.size))
    } else {
        let generic = style
            .families
            .last()
            .and_then(|family| reprise_font::GenericFamily::parse(family))
            .unwrap_or(reprise_font::GenericFamily::Serif);
        Some((engine.fonts.generic(generic).id().clone(), style.size))
    };
    let shape_key = evaluation.map(|_| {
        // The additive entry point changes fallback semantics even for the same
        // list of names. Version only the owned cache representation; itemization
        // and the adapter receive the original authored families unchanged.
        let mut key_styles = styles.clone();
        if matches!(
            ctx.writing_mode,
            reprise_doc::WritingMode::VerticalRl | reprise_doc::WritingMode::VerticalLr
        ) {
            for run in &mut key_styles {
                run.families.insert(
                    0,
                    format!(
                        "\0vertical:{:?}:{:?}",
                        style.text_orientation, style.text_combine_upright
                    ),
                );
            }
        }
        if !style.families.is_empty() {
            for run in &mut key_styles {
                run.families.insert(0, "\0reprise-font-chain1".into());
            }
        }
        let key_input = ParagraphInput {
            text: &text,
            styles: &key_styles,
            direction: None,
        };
        crate::incremental::ShapingKey::new(&key_input, fallback.clone())
    });
    let shape_notes = notes.len();
    if let (Some(e), Some(key)) = (evaluation, shape_key.as_ref())
        && let Some((value, hit_notes)) = e.shaping_hit(node, key)
    {
        notes.extend(hit_notes);
        return Begin::Done(
            value.map(|mut prepared| {
                prepared.kind = block.kind;
                prepared.style = style;
                Box::new(prepared)
            }),
            notes,
        );
    }
    Begin::Shape(Box::new(Staged {
        vertical: matches!(
            ctx.writing_mode,
            reprise_doc::WritingMode::VerticalRl | reprise_doc::WritingMode::VerticalLr
        ),
        node,
        kind: block.kind,
        style,
        text,
        styles,
        fallback,
        shape_key,
        notes,
        shape_notes,
        itemized: None,
        requests: Vec::new(),
        glyphs: Vec::new(),
        breaks: None,
    }))
}

impl Staged {
    /// Bytes of paragraph text.
    pub(crate) fn len(&self) -> usize {
        self.text.len()
    }
    /// The byte length of every planned shaping request.
    pub(crate) fn request_bytes(&self) -> Vec<usize> {
        self.requests.iter().map(|r| r.range.len()).collect()
    }
    /// Shaping requests not yet run.
    pub(crate) fn remaining(&self) -> usize {
        self.glyphs.iter().filter(|g| g.is_none()).count()
    }
    pub(crate) fn has_breaks(&self) -> bool {
        self.breaks.is_some()
    }

    /// Itemises the paragraph and plans its shaping requests. Pure.
    pub(crate) fn itemize(&mut self, fonts: &reprise_font::FontStore) {
        if self.itemized.is_some() {
            return;
        }
        let input = ParagraphInput {
            text: &self.text,
            styles: &self.styles,
            direction: None,
        };
        let mut itemized = if self.style.families.is_empty() {
            itemize(&input, fonts)
        } else {
            itemize_families(&input, fonts)
        };
        if self.vertical {
            itemized.items = reprise_shape::vertical_items(
                &self.text,
                &itemized.items,
                self.style.text_orientation,
                self.style.text_combine_upright,
            );
        }
        if self.vertical && self.style.text_orientation == reprise_geom::TextOrientation::Upright {
            itemized.base_level = 0;
            itemized.levels.fill(0);
            for item in &mut itemized.items {
                item.level = 0;
            }
        }
        let subject = Subject::Node(self.node);
        self.notes.extend(
            itemized
                .notes
                .into_iter()
                .map(|n| Diagnostic::from_note(n, subject.clone())),
        );
        if !self.unshapeable_items(&itemized.items) {
            self.requests = shaping::plan(&self.text, &itemized.items, fonts);
            self.glyphs = vec![None; self.requests.len()];
        }
        self.itemized = Some(Itemized {
            items: itemized.items,
            levels: itemized.levels,
            base_level: itemized.base_level,
        });
    }

    fn unshapeable_items(&self, items: &[Item]) -> bool {
        items.is_empty() && !self.text.is_empty()
    }

    /// Runs up to `max` of the remaining shaping requests on `workers`, in
    /// request order. Returns the byte length of each request that ran. Pure.
    pub(crate) fn shape(
        &mut self,
        fonts: &reprise_font::FontStore,
        adapter: &dyn reprise_shape::ShapingAdapter,
        workers: &dyn Workers,
        max: usize,
    ) -> Vec<usize> {
        let Some(itemized) = &self.itemized else {
            return Vec::new();
        };
        let pending: Vec<usize> = self
            .glyphs
            .iter()
            .enumerate()
            .filter(|(_, g)| g.is_none())
            .map(|(i, _)| i)
            .take(max)
            .collect();
        let text = &self.text;
        let requests = &self.requests;
        let results = crate::workers::map(workers, &pending, &|&i: &usize| {
            requests
                .get(i)
                .and_then(|r| itemized.items.get(r.item).map(|item| (r, item)))
                .map_or_else(Vec::new, |(r, item)| {
                    shaping::shape_request(text, item, r.range.clone(), fonts, adapter)
                })
        });
        let mut sizes = Vec::with_capacity(pending.len());
        for (i, glyphs) in pending.iter().zip(results) {
            sizes.push(self.requests.get(*i).map_or(0, |r| r.range.len()));
            if let Some(slot) = self.glyphs.get_mut(*i) {
                *slot = Some(glyphs);
            }
        }
        sizes
    }

    /// UAX #14 break analysis. Pure.
    pub(crate) fn analyse_breaks(&mut self) {
        if self.breaks.is_none() {
            let mut breaks = break_opportunities(&self.text);
            if let Some(items) = &self.itemized {
                breaks.retain(|b| {
                    !items
                        .items
                        .iter()
                        .any(|i| i.combined && i.range.start < b.at && b.at < i.range.end)
                });
            }
            self.breaks = Some(breaks);
        }
    }

    /// Break analysis, unless itemisation found nothing it could shape (then
    /// `prepare_with` returns before analysing breaks). Pure.
    pub(crate) fn analyse_breaks_if_shapeable(&mut self) {
        if !self
            .itemized
            .as_ref()
            .is_some_and(|i| self.unshapeable_items(&i.items))
        {
            self.analyse_breaks();
        }
    }

    /// Every remaining stage, inline. Pure: no counters, no memo.
    pub(crate) fn run_pure(
        &mut self,
        fonts: &reprise_font::FontStore,
        adapter: &dyn reprise_shape::ShapingAdapter,
        workers: &dyn Workers,
    ) {
        self.itemize(fonts);
        self.shape(fonts, adapter, workers, usize::MAX);
        self.analyse_breaks_if_shapeable();
    }

    /// Finishes any stage not yet run (serially), then assembles the value,
    /// counts the work and records the shaping memo, exactly as
    /// `prepare_with` does after itemisation.
    pub(crate) fn finish(
        mut self,
        engine: &Engine,
        evaluation: Option<&crate::incremental::Evaluation>,
    ) -> Finished {
        self.run_pure(&engine.fonts, engine.shaper.as_ref(), &Serial);
        if let Some(e) = evaluation {
            e.itemization();
        }
        let Some(itemized) = self.itemized.take() else {
            return Finished {
                value: None,
                notes: self.notes,
                itemized: false,
            };
        };
        if self.unshapeable_items(&itemized.items) {
            if let (Some(e), Some(key)) = (evaluation, self.shape_key) {
                e.shaping_miss(
                    self.node,
                    key,
                    None,
                    self.notes
                        .get(self.shape_notes..)
                        .unwrap_or_default()
                        .to_vec(),
                );
            }
            return Finished {
                value: None,
                notes: self.notes,
                itemized: true,
            };
        }
        if let Some(e) = evaluation {
            e.shape();
        }
        let glyphs = self
            .glyphs
            .into_iter()
            .map(Option::unwrap_or_default)
            .collect();
        let shaped = shaping::assemble(self.text.len(), &itemized.items, &self.requests, glyphs);
        let prepared = Prepared {
            node: self.node,
            kind: self.kind,
            breaks: self.breaks.unwrap_or_default(),
            style: self.style,
            text: self.text,
            items: itemized.items,
            levels: itemized.levels,
            base_level: itemized.base_level,
            shaped,
            fallback: self.fallback,
            image: None,
        };
        if let (Some(e), Some(key)) = (evaluation, self.shape_key) {
            e.shaping_miss(
                self.node,
                key,
                Some(prepared.clone()),
                self.notes
                    .get(self.shape_notes..)
                    .unwrap_or_default()
                    .to_vec(),
            );
        }
        Finished {
            value: Some(prepared),
            notes: self.notes,
            itemized: true,
        }
    }
}

/// `prepare_with`, as its stages back to back.
pub(crate) fn run(
    engine: &Engine,
    doc: &Document,
    node: NodeId,
    ctx: &ResolutionContext,
    evaluation: Option<&crate::incremental::Evaluation>,
    workers: &dyn Workers,
) -> Finished {
    match begin(engine, doc, node, ctx, evaluation) {
        Begin::Done(value, notes) => Finished {
            value: value.map(|p| *p),
            notes,
            itemized: false,
        },
        Begin::Shape(mut staged) => {
            staged.run_pure(&engine.fonts, engine.shaper.as_ref(), workers);
            if let Some(e) = evaluation {
                e.shaped(staged.request_bytes());
                e.scanned(staged.len());
                if staged.has_breaks() {
                    e.scanned(staged.len());
                }
            }
            staged.finish(engine, evaluation)
        }
    }
}
