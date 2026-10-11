//! List item geometry (see `docs/roles-and-lists.md`): the text inset from the
//! logical inline start, and the marker in the hanging indent.
//!
//! The marker is derived: shaped once per preparation in the item's first-run
//! style (plus the definition's marker patch), and placed on the first line's
//! baseline. It is not part of the block's text, so carets and selection never
//! reach it. Everything is integer `Length` arithmetic and saturates.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use reprise_compose::{Available, GeometryProvider, Interval, LineQuery};
use reprise_diag::Note;
use reprise_doc::formatting::TextStyle;
use reprise_doc::{Document, NodeId};
use reprise_geom::{InlineDirection, Length};
use reprise_shape::{ParagraphInput, ShapedRun, Shaper, StyleRun, visual_order};

use crate::{Engine, LineLayout, MarkerLayout, PositionedRun};

/// What layout needs from an item's numbering. Part of every incremental key,
/// so a renumbered item re-lays out and an unchanged marker does not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ListSpec {
    pub marker: String,
    pub indent: Length,
    pub hanging: Length,
    pub style: Option<TextStyle>,
}

/// A block's semantic input to layout: its list geometry and what reading its
/// role and membership reported.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Semantics {
    pub list: Option<ListSpec>,
    pub notes: Vec<Note>,
}

pub(crate) fn semantics(doc: &Document, node: NodeId) -> Semantics {
    let numbering = doc.list_numbering();
    let mut notes: Vec<Note> = doc
        .stored_role(node)
        .ok()
        .and_then(|r| r.note())
        .into_iter()
        .collect();
    notes.extend(numbering.notes.get(&node).cloned().unwrap_or_default());
    Semantics {
        list: numbering.items.get(&node).map(|item| ListSpec {
            marker: item.marker.clone(),
            indent: item.indent,
            hanging: item.hanging,
            style: item.marker_style.clone(),
        }),
        notes,
    }
}

/// The first run's style, as the marker inherits it.
pub(crate) struct FirstRun<'a> {
    pub style: Option<&'a StyleRun>,
    pub descriptors: Option<reprise_font::Descriptors>,
    pub color: [u8; 4],
    pub family_chains: bool,
}

/// A marker shaped and ready to place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PreparedMarker {
    pub spec: ListSpec,
    /// In visual order; ranges are bytes of `spec.marker`.
    pub runs: Vec<ShapedRun>,
    pub width: Length,
    pub color: [u8; 4],
    pub rtl: bool,
    /// The marker's start, from the room's inline-start edge.
    pub start: Length,
    /// The first line's text inset: the indent, or past a wide marker.
    pub first_inset: Length,
}

/// Shapes `spec`'s marker in the paragraph's direction.
pub(crate) fn prepare(
    engine: &Engine,
    spec: ListSpec,
    base_level: u8,
    first: FirstRun<'_>,
    notes: &mut Vec<Note>,
) -> PreparedMarker {
    let rtl = base_level % 2 == 1;
    let patch = spec.style.clone().unwrap_or_default();
    let base = first.style;
    let size = patch
        .size
        .or(base.map(|s| s.size))
        .unwrap_or(Length::from_pt(12));
    let families = patch
        .families
        .clone()
        .or(base.map(|s| s.families.clone()))
        .unwrap_or_else(|| vec!["serif".into()]);
    let mut descriptors = first.descriptors.unwrap_or(reprise_font::Descriptors {
        weight: 400,
        style: reprise_font::FontStyle::Normal,
        stretch: 1000,
    });
    if let Some(w) = patch.weight {
        descriptors.weight = w;
    }
    if let Some(slant) = patch.slant {
        descriptors.style = match slant {
            reprise_doc::TextSlant::Normal => reprise_font::FontStyle::Normal,
            reprise_doc::TextSlant::Italic => reprise_font::FontStyle::Italic,
            reprise_doc::TextSlant::Oblique => reprise_font::FontStyle::Oblique,
        };
    }
    let text = spec.marker.clone();
    let styles = [StyleRun {
        range: 0..text.len(),
        families,
        size,
        language: base.and_then(|s| s.language.clone()),
        features: Vec::new(),
    }];
    let input = ParagraphInput {
        text: &text,
        styles: &styles,
        direction: Some(if rtl {
            InlineDirection::Rtl
        } else {
            InlineDirection::Ltr
        }),
    };
    let runs = if text.is_empty() {
        Vec::new()
    } else {
        let itemized = reprise_shape::itemize_emphasis(
            &input,
            &engine.fonts,
            first.family_chains || patch.families.is_some(),
            &[descriptors],
        );
        notes.extend(itemized.notes);
        let shaped = Shaper {
            text: &text,
            items: &itemized.items,
            fonts: &engine.fonts,
            adapter: engine.shaper.as_ref(),
        }
        .shape();
        let levels: Vec<u8> = shaped.runs.iter().map(|r| r.level).collect();
        visual_order(&levels)
            .into_iter()
            .filter_map(|i| shaped.runs.get(i).cloned())
            .collect()
    };
    let width = runs
        .iter()
        .fold(Length::ZERO, |w, r: &ShapedRun| w + r.width());
    let start = (spec.indent - spec.hanging).max(Length::ZERO);
    let gap = size.mul_ratio(1, 4);
    // A marker wider than the hanging indent pushes the first line's text.
    let first_inset = if start + width > spec.indent {
        start + width + gap
    } else {
        spec.indent
    };
    PreparedMarker {
        spec,
        runs,
        width,
        color: patch.color.unwrap_or(first.color),
        rtl,
        start,
        first_inset,
    }
}

/// The room's inline edges a first line was given, and the inset it used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Edges {
    pub lo: Length,
    pub hi: Length,
    pub inset: Length,
}

/// Insets any geometry provider from the room's logical inline start: the
/// left edge of the room in a left-to-right paragraph, the right edge in a
/// right-to-left one. Frame space is logical, so a vertical frame needs
/// nothing more. An inset that would leave less than `min` is reduced.
pub(crate) struct Inset<'a> {
    pub inner: &'a dyn GeometryProvider,
    pub marker: &'a PreparedMarker,
    /// The composition starts at the paragraph's first line.
    pub from_start: bool,
    pub min: Length,
    /// The first line's edges, by the block offset it was asked at.
    pub edges: RefCell<BTreeMap<Length, Edges>>,
    pub clamped: Cell<bool>,
}

impl<'a> Inset<'a> {
    pub(crate) fn new(
        inner: &'a dyn GeometryProvider,
        marker: &'a PreparedMarker,
        from_start: bool,
        min: Length,
    ) -> Self {
        Inset {
            inner,
            marker,
            from_start,
            min: min.max(Length(1)),
            edges: RefCell::new(BTreeMap::new()),
            clamped: Cell::new(false),
        }
    }

    /// The edges of the first line, composed at `block_offset`.
    pub(crate) fn first_edges(&self, block_offset: Length) -> Option<Edges> {
        self.edges.borrow().get(&block_offset).copied()
    }
}

impl GeometryProvider for Inset<'_> {
    fn available(&self, query: &LineQuery<'_>) -> Available {
        let intervals = match self.inner.available(query) {
            Available::Room(intervals) if !intervals.is_empty() => intervals,
            other => return other,
        };
        let lo = intervals.iter().map(|i| i.start).min().unwrap_or_default();
        let hi = intervals.iter().map(|i| i.end).max().unwrap_or_default();
        let first = self.from_start && query.line == 0;
        let wanted = if first {
            self.marker.first_inset
        } else {
            self.marker.spec.indent
        };
        let most = ((hi - lo) - self.min).max(Length::ZERO);
        let inset = wanted.min(most);
        if inset < wanted {
            self.clamped.set(true);
        }
        if first {
            self.edges
                .borrow_mut()
                .insert(query.block_offset, Edges { lo, hi, inset });
        }
        if inset == Length::ZERO {
            return Available::Room(intervals);
        }
        let rtl = self.marker.rtl;
        Available::Room(
            intervals
                .into_iter()
                .filter_map(|i| {
                    let clipped = if rtl {
                        Interval::new(i.start, i.end.min(hi - inset))
                    } else {
                        Interval::new(i.start.max(lo + inset), i.end)
                    };
                    (clipped.width() > Length::ZERO).then_some(clipped)
                })
                .collect(),
        )
    }
}

/// Places a marker on the first line of its item, on that line's baseline.
pub(crate) fn place(
    marker: &PreparedMarker,
    line: &LineLayout,
    edges: Edges,
) -> Option<MarkerLayout> {
    if marker.runs.is_empty() {
        return None;
    }
    // A reduced inset pulls the marker in with it.
    let start = marker
        .start
        .min((edges.inset - marker.width).max(Length::ZERO));
    let mut x = if marker.rtl {
        edges.hi - start - marker.width
    } else {
        edges.lo + start
    };
    let origin = x;
    let runs = marker
        .runs
        .iter()
        .map(|run| {
            let width = run.width();
            let placed = PositionedRun {
                synthetic_oblique: run.synthetic_oblique,
                color: marker.color,
                underline: None,
                strike: None,
                upright: run.upright,
                combined: run.combined,
                horizontal_scale: run.horizontal_scale,
                range: run.range.clone(),
                face: run.face.clone(),
                size: run.size,
                level: run.level,
                x,
                width,
                glyphs: run.glyphs.clone(),
            };
            x += width;
            placed
        })
        .collect();
    Some(MarkerLayout {
        text: marker.spec.marker.clone(),
        x: origin,
        width: marker.width,
        baseline: line.baseline,
        runs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use reprise_compose::Measure;

    fn marker(rtl: bool, width: i32) -> PreparedMarker {
        PreparedMarker {
            spec: ListSpec {
                marker: "1.".into(),
                indent: Length::from_pt(36),
                hanging: Length::from_pt(18),
                style: None,
            },
            runs: Vec::new(),
            width: Length::from_pt(width),
            color: [0, 0, 0, 255],
            rtl,
            start: Length::from_pt(18),
            first_inset: Length::from_pt(36),
        }
    }

    fn ask(inset: &Inset<'_>, line: u32) -> Available {
        inset.available(&LineQuery {
            line,
            block_offset: Length::ZERO,
            line_height: Length::from_pt(12),
            previous: &[],
        })
    }

    #[test]
    fn insets_mirror_in_right_to_left_and_record_first_line_edges() {
        let measure = Measure(Length::from_pt(200));
        let ltr = marker(false, 10);
        let inset = Inset::new(&measure, &ltr, true, Length::from_pt(12));
        assert_eq!(
            ask(&inset, 0),
            Available::Room(vec![Interval::new(
                Length::from_pt(36),
                Length::from_pt(200)
            )])
        );
        assert_eq!(
            inset.first_edges(Length::ZERO),
            Some(Edges {
                lo: Length::ZERO,
                hi: Length::from_pt(200),
                inset: Length::from_pt(36)
            })
        );
        let rtl = marker(true, 10);
        let inset = Inset::new(&measure, &rtl, true, Length::from_pt(12));
        assert_eq!(
            ask(&inset, 1),
            Available::Room(vec![Interval::new(Length::ZERO, Length::from_pt(164))])
        );
        assert_eq!(inset.first_edges(Length::ZERO), None, "only line 0 records");
    }

    #[test]
    fn absurd_indents_leave_room_and_report() {
        let measure = Measure(Length::from_pt(20));
        let mut m = marker(false, 10);
        m.first_inset = Length::MAX;
        m.spec.indent = Length::MAX;
        let inset = Inset::new(&measure, &m, true, Length::from_pt(12));
        assert_eq!(
            ask(&inset, 0),
            Available::Room(vec![Interval::new(Length::from_pt(8), Length::from_pt(20))])
        );
        assert!(inset.clamped.get());
        // A room narrower than the minimum is left whole.
        let narrow = Measure(Length::from_pt(5));
        let inset = Inset::new(&narrow, &m, false, Length::from_pt(12));
        assert_eq!(
            ask(&inset, 3),
            Available::Room(vec![Interval::new(Length::ZERO, Length::from_pt(5))])
        );
        let zero = Measure(Length::ZERO);
        let inset = Inset::new(&zero, &m, false, Length::from_pt(12));
        assert_eq!(
            ask(&inset, 0),
            Available::Room(vec![Interval::new(Length::ZERO, Length::ZERO)]),
            "no inset leaves the room exactly as it was"
        );
    }
}
