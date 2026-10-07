//! Chunked shaping: the reference rule that bounds one adapter call (22, 38).
//!
//! `Shaper::shape` makes one adapter call per item. Layout instead shapes an
//! item longer than [`SHAPE_CHUNK_BYTES`] in sub-ranges whose seams are a pure
//! function of the text, then merges them back into one run per item. Both
//! `Engine::layout` and incremental jobs use this module, so they issue the
//! same requests; a job may spread the requests over several steps and
//! threads. Items of at most `SHAPE_CHUNK_BYTES` get exactly the request
//! `Shaper::shape` would make. See `docs/incremental.md` ("Chunked shaping").

use std::ops::Range;

use reprise_font::FontStore;
use reprise_geom::InlineDirection;
use reprise_shape::{Item, ShapeRequest, ShapedGlyph, ShapedRun, ShapedText, ShapingAdapter};

#[cfg(test)]
use crate::workers::{Workers, map};

/// The most bytes one adapter request covers, unless a single grapheme
/// cluster is longer.
pub const SHAPE_CHUNK_BYTES: usize = 16 * 1024;

/// One adapter request: part of `items[item]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Request {
    pub item: usize,
    pub range: Range<usize>,
}

/// Every request for `items`, grouped by item in item order and, inside an
/// item, in logical order. Items `Shaper::shape` would skip (empty after
/// clipping to the text, or with a face missing from `fonts`) get none.
pub(crate) fn plan(text: &str, items: &[Item], fonts: &FontStore) -> Vec<Request> {
    let len = text.len();
    let mut out = Vec::new();
    let mut boundaries: Option<Vec<usize>> = None;
    for (index, item) in items.iter().enumerate() {
        let part = item.range.start..item.range.end.min(len);
        if part.is_empty() || fonts.get(&item.face).is_err() {
            continue;
        }
        let mut pos = part.start;
        if part.len() > SHAPE_CHUNK_BYTES {
            let boundaries =
                boundaries.get_or_insert_with(|| reprise_text::segment::grapheme_boundaries(text));
            while part.end.saturating_sub(pos) > SHAPE_CHUNK_BYTES {
                let next = seam(text, boundaries, pos, part.end);
                out.push(Request {
                    item: index,
                    range: pos..next,
                });
                pos = next;
            }
        }
        if pos < part.end {
            out.push(Request {
                item: index,
                range: pos..part.end,
            });
        }
    }
    out
}

/// The next seam after `pos`, when more than `SHAPE_CHUNK_BYTES` remain before
/// `end`: the last grapheme boundary within the chunk that follows a space,
/// else the last grapheme boundary within it, else the end of the oversized
/// grapheme cluster at `pos`. Always in `pos + 1..=end`.
fn seam(text: &str, boundaries: &[usize], pos: usize, end: usize) -> usize {
    let limit = pos.saturating_add(SHAPE_CHUNK_BYTES);
    let from = boundaries.partition_point(|&b| b <= pos);
    let to = boundaries.partition_point(|&b| b <= limit);
    let window = boundaries.get(from..to).unwrap_or_default();
    let after_space = window
        .iter()
        .rev()
        .find(|&&b| b.checked_sub(1).and_then(|i| text.as_bytes().get(i)) == Some(&b' '));
    after_space
        .or(window.last())
        .copied()
        .unwrap_or_else(|| {
            boundaries
                .get(to)
                .copied()
                .filter(|&b| b < end)
                .unwrap_or(end)
        })
        .clamp(pos.saturating_add(1), end)
}

/// Shapes one request, with the whole paragraph as context like
/// `Shaper::shape`. A pure function of its arguments.
pub(crate) fn shape_request(
    text: &str,
    item: &Item,
    range: Range<usize>,
    fonts: &FontStore,
    adapter: &dyn ShapingAdapter,
) -> Vec<ShapedGlyph> {
    let Ok(face) = fonts.get(&item.face) else {
        return Vec::new();
    };
    adapter.shape(&ShapeRequest {
        text,
        range,
        context: 0..text.len(),
        face,
        size: item.size,
        direction: item.direction(),
        script: item.script,
        language: item.language.as_deref(),
        features: &item.features,
    })
}

/// Merges the results of `requests` (in the same order) into one run per item.
/// A right-to-left item's glyphs are in visual order, so its sub-runs join
/// last seam first.
pub(crate) fn assemble(
    text_len: usize,
    items: &[Item],
    requests: &[Request],
    mut glyphs: Vec<Vec<ShapedGlyph>>,
) -> ShapedText {
    let mut runs = Vec::new();
    let mut at = 0;
    while let Some(first) = requests.get(at) {
        let end = requests
            .get(at..)
            .unwrap_or_default()
            .iter()
            .position(|r| r.item != first.item)
            .map_or(requests.len(), |n| at + n);
        let Some(item) = items.get(first.item) else {
            at = end;
            continue;
        };
        let mut parts: Vec<Vec<ShapedGlyph>> = glyphs
            .get_mut(at..end)
            .unwrap_or_default()
            .iter_mut()
            .map(std::mem::take)
            .collect();
        if item.direction() == InlineDirection::Rtl {
            parts.reverse();
        }
        runs.push(ShapedRun {
            range: item.range.start..item.range.end.min(text_len),
            face: item.face.clone(),
            size: item.size,
            level: item.level,
            glyphs: parts.concat(),
        });
        at = end;
    }
    ShapedText { runs }
}

/// Every request of [`plan`], on `workers`, then [`assemble`]: what
/// `flow::stage` does in stages. The tests compare it with `Shaper::shape`.
#[cfg(test)]
pub(crate) fn shape(
    text: &str,
    items: &[Item],
    fonts: &FontStore,
    adapter: &dyn ShapingAdapter,
    workers: &dyn Workers,
) -> ShapedText {
    let requests = plan(text, items, fonts);
    let glyphs = map(workers, &requests, &|r: &Request| {
        items.get(r.item).map_or_else(Vec::new, |item| {
            shape_request(text, item, r.range.clone(), fonts, adapter)
        })
    });
    assemble(text.len(), items, &requests, glyphs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::Serial;
    use reprise_geom::Length;
    use reprise_shape::{HarfRust, ParagraphInput, Shaper, StyleRun, itemize};

    fn itemized(text: &str, fonts: &FontStore) -> Vec<Item> {
        let styles = [StyleRun {
            range: 0..text.len(),
            families: vec!["serif".into()],
            size: Length::from_pt(10),
            language: None,
            features: Vec::new(),
        }];
        itemize(
            &ParagraphInput {
                text,
                styles: &styles,
                direction: None,
            },
            fonts,
        )
        .items
    }

    fn whole(text: &str, items: &[Item], fonts: &FontStore) -> ShapedText {
        Shaper {
            text,
            items,
            fonts,
            adapter: &HarfRust,
        }
        .shape()
    }

    fn clusters(shaped: &ShapedText) -> Vec<Vec<u32>> {
        shaped
            .runs
            .iter()
            .map(|r| r.glyphs.iter().map(|g| g.cluster).collect())
            .collect()
    }

    #[test]
    fn short_items_get_exactly_the_shaper_request() {
        let fonts = FontStore::default();
        for text in [
            "",
            "office ffi",
            "abc \u{5d0}\u{5d1}\u{5d2} def",
            "e\u{301}\u{302}",
        ] {
            let items = itemized(text, &fonts);
            assert_eq!(
                shape(text, &items, &fonts, &HarfRust, &Serial),
                whole(text, &items, &fonts),
                "{text:?}"
            );
        }
    }

    #[test]
    fn long_items_split_after_spaces_in_logical_and_visual_order() {
        let fonts = FontStore::default();
        for word in ["lorem ", "\u{5e9}\u{5dc}\u{5d5}\u{5dd} "] {
            let text = word.repeat(3 * SHAPE_CHUNK_BYTES / word.len());
            let items = itemized(&text, &fonts);
            assert_eq!(items.len(), 1);
            let requests = plan(&text, &items, &fonts);
            assert!(requests.len() >= 3, "{}", requests.len());
            for pair in requests.windows(2) {
                assert_eq!(pair[0].range.end, pair[1].range.start);
                assert_eq!(text.as_bytes()[pair[0].range.end - 1], b' ');
            }
            assert!(requests.iter().all(|r| r.range.len() <= SHAPE_CHUNK_BYTES));
            let chunked = shape(&text, &items, &fonts, &HarfRust, &Serial);
            let reference = whole(&text, &items, &fonts);
            // Neither word ligates or kerns with a space in this font, so the
            // seams are invisible here; the order of clusters must match.
            assert_eq!(clusters(&chunked), clusters(&reference));
            assert_eq!(chunked.runs[0].range, reference.runs[0].range);
        }
    }

    #[test]
    fn seams_never_split_graphemes_and_oversized_clusters_stay_whole() {
        let fonts = FontStore::default();
        // No spaces at all: seams fall on grapheme boundaries.
        let text = "e\u{301}".repeat(SHAPE_CHUNK_BYTES);
        let items = itemized(&text, &fonts);
        let bounds = reprise_text::segment::grapheme_boundaries(&text);
        for r in plan(&text, &items, &fonts) {
            assert!(bounds.binary_search(&r.range.start).is_ok());
            assert!(bounds.binary_search(&r.range.end).is_ok());
            assert!(r.range.len() <= SHAPE_CHUNK_BYTES);
        }
        // One grapheme cluster longer than a chunk: shaped whole, then chunks resume.
        let giant = format!("a{}", "\u{301}".repeat(SHAPE_CHUNK_BYTES));
        let text = format!("{giant} {}", "b ".repeat(SHAPE_CHUNK_BYTES));
        let items = itemized(&text, &fonts);
        let requests = plan(&text, &items, &fonts);
        assert_eq!(requests[0].range, 0..giant.len());
        assert!(
            requests
                .iter()
                .skip(1)
                .all(|r| r.range.len() <= SHAPE_CHUNK_BYTES)
        );
        let covered: usize = requests.iter().map(|r| r.range.len()).sum();
        assert_eq!(covered, text.len());
    }

    #[test]
    fn malformed_items_are_skipped_or_clipped_like_the_shaper() {
        let fonts = FontStore::default();
        let text = "plain text";
        let mut items = itemized(text, &fonts);
        let mut missing = items[0].clone();
        missing.face.family = "absent".into();
        items.push(missing);
        let mut past = items[0].clone();
        past.range = 4..usize::MAX;
        items.push(past);
        let mut reversed = items[0].clone();
        reversed.range = std::ops::Range { start: 7, end: 2 };
        items.push(reversed);
        assert_eq!(
            shape(text, &items, &fonts, &HarfRust, &Serial),
            whole(text, &items, &fonts)
        );
    }
}
