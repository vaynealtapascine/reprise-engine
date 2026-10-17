//! Oracles over the facade's `State` and saved packages.
//!
//! These are pure functions of data a host can see: they hold for any
//! document, whatever sequence of commands made it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use reprise::{AnnotationStatus, BlockRole, State};
use reprise_format::{Container, Limits, features};

use crate::codes;
use crate::ensure;
use crate::violation::R;

const OBJECT: char = '\u{FFFC}';

/// A state with the fields that legitimately differ between replicas, and
/// between a state and its undo, cleared.
pub fn authored(state: &State) -> State {
    let mut s = state.clone();
    s.revision.clear();
    s.can_undo = false;
    s.can_redo = false;
    s.peer_id.clear();
    s.document_id.clear();
    // Layout notes depend on whether a layout is current, not on the document.
    s.diagnostics.clear();
    s
}

/// Every diagnostic in `state` has a documented code.
pub fn documented_diagnostics(state: &State) -> R {
    for d in &state.diagnostics {
        ensure!(
            codes::is_documented(&d.code),
            "undocumented-code",
            "State.diagnostics reported `{}`",
            d.code
        );
    }
    Ok(())
}

fn on_boundary(text: &str, at: u32) -> bool {
    text.is_char_boundary(at as usize)
}

/// Structural invariants of a `State` that hold whatever produced it.
pub fn state_is_coherent(state: &State) -> R {
    let ids: BTreeSet<&str> = state.blocks.iter().map(|b| b.id.as_str()).collect();
    ensure!(
        ids.len() == state.blocks.len(),
        "state-coherent",
        "two blocks share an ID"
    );
    let limited = state.diagnostics.iter().any(|d| d.code == "bindings.limit");
    let list_ids: BTreeSet<&str> = state.lists.iter().map(|l| l.id.as_str()).collect();
    let mut inline_ids = BTreeSet::new();
    for b in &state.blocks {
        ensure!(
            (b.style.resolved as usize) < state.resolved.len(),
            "state-coherent",
            "block {} names resolved style {} of {}",
            b.id,
            b.style.resolved,
            state.resolved.len()
        );
        if let Some(parent) = &b.parent {
            ensure!(
                ids.contains(parent.as_str()),
                "state-coherent",
                "block {} has the missing parent {parent}",
                b.id
            );
        }
        // Atoms: exactly the U+FFFC characters of the text, in order, each
        // with an entry (docs/inline.md, "Visibility").
        let atoms: Vec<usize> = b
            .text
            .char_indices()
            .filter(|&(_, c)| c == OBJECT)
            .map(|(i, _)| i)
            .collect();
        let entries: Vec<usize> = b.inline.iter().map(|e| e.at as usize).collect();
        ensure!(
            atoms == entries,
            "inline-matches-text",
            "block {}: U+FFFC at {atoms:?} but inline entries at {entries:?}",
            b.id
        );
        for entry in &b.inline {
            ensure!(
                inline_ids.insert(entry.id.as_str()),
                "state-coherent",
                "inline object ID {} appears twice",
                entry.id
            );
        }
        // Formatting runs: ordered, disjoint, in range, on boundaries.
        if let Some(runs) = &b.formatting {
            let mut previous = 0u32;
            for run in runs {
                ensure!(
                    run.start >= previous
                        && run.start <= run.end
                        && run.end as usize <= b.text.len()
                        && on_boundary(&b.text, run.start)
                        && on_boundary(&b.text, run.end),
                    "state-coherent",
                    "block {}: formatting run {}..{} of {} bytes",
                    b.id,
                    run.start,
                    run.end,
                    b.text.len()
                );
                previous = run.end;
            }
        }
        if let Some(item) = &b.list {
            ensure!(
                item.level <= 8,
                "state-coherent",
                "list level {}",
                item.level
            );
            ensure!(
                list_ids.contains(item.id.as_str()) || limited,
                "state-coherent",
                "block {} is in the list {} that State.lists doesn't have",
                b.id,
                item.id
            );
        }
    }
    let mut seen = BTreeSet::new();
    for a in &state.annotations {
        ensure!(
            seen.insert(a.id.as_str()),
            "state-coherent",
            "annotation {} is listed twice",
            a.id
        );
        if a.status == AnnotationStatus::Active {
            ensure!(
                !a.spans.is_empty(),
                "state-coherent",
                "active annotation {} has no span",
                a.id
            );
        }
        for span in &a.spans {
            let Some(block) = state.blocks.iter().find(|b| b.id == span.node) else {
                // Spans of a block outside the listed ones can happen only
                // when the listing was cut.
                ensure!(
                    limited,
                    "state-coherent",
                    "annotation {} spans the missing block {}",
                    a.id,
                    span.node
                );
                continue;
            };
            ensure!(
                span.start <= span.end
                    && span.end as usize <= block.text.len()
                    && on_boundary(&block.text, span.start)
                    && on_boundary(&block.text, span.end),
                "state-coherent",
                "annotation {} span {}..{} in block {} of {} bytes",
                a.id,
                span.start,
                span.end,
                block.id,
                block.text.len()
            );
            if a.status == AnnotationStatus::Active {
                ensure!(
                    span.start < span.end,
                    "state-coherent",
                    "active annotation {} has an empty span",
                    a.id
                );
            }
        }
    }
    let names: Vec<&str> = state.styles.iter().map(|s| s.name.as_str()).collect();
    ensure!(
        names.windows(2).all(|w| w[0] < w[1]),
        "state-coherent",
        "State.styles isn't sorted by name: {names:?}"
    );
    Ok(())
}

/// The package feature bits in saved bytes.
pub fn package_bits(bytes: &[u8]) -> R<u64> {
    let (container, _) = Container::decode(bytes, Limits::default()).map_err(|e| {
        crate::violation::Violation::new("package-decodes", format!("a saved package: {e}"))
    })?;
    Ok(container.header.features.required)
}

/// Whether each wave-1 stored form could have been written, tracked by the
/// scenario from what it did. An over-approximation: a flag is set when an
/// op that may write the form succeeded in the document's lineage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    /// Roles, list definitions, list membership (bit 14).
    pub semantics: bool,
    /// Inline object records (bit 15).
    pub inline: bool,
    /// Annotations (bit 16).
    pub annotations: bool,
    /// Durable style identities (bit 18).
    pub styles: bool,
}

impl Flags {
    pub fn merge(&mut self, other: Flags) {
        self.semantics |= other.semantics;
        self.inline |= other.inline;
        self.annotations |= other.annotations;
        self.styles |= other.styles;
    }
}

/// Bits 14, 15, 16 and 18 are set when their stored form is *live* in the
/// state (soundness: an older reader would otherwise silently drop it), and
/// only when the scenario could have written that form (minimality: a
/// document that never used a feature stays byte-compatible with old
/// readers).
pub fn bits_match(bytes: &[u8], state: &State, may: Flags) -> R {
    let bits = package_bits(bytes)?;
    let live_semantics = !state.lists.is_empty()
        || state
            .blocks
            .iter()
            .any(|b| b.role != BlockRole::Body || b.list.is_some());
    let live_inline = state.blocks.iter().any(|b| !b.inline.is_empty());
    let live_annotations = !state.annotations.is_empty();
    for (name, bit, live, allowed) in [
        (
            "block-semantics (14)",
            features::REQUIRED_BLOCK_SEMANTICS,
            live_semantics,
            may.semantics,
        ),
        (
            "inline-objects (15)",
            features::REQUIRED_INLINE_OBJECTS,
            live_inline,
            may.inline,
        ),
        (
            "annotations (16)",
            features::REQUIRED_ANNOTATIONS,
            live_annotations,
            may.annotations,
        ),
    ] {
        ensure!(
            !live || bits & bit != 0,
            "package-bit-missing",
            "{name} is live in the document but the package doesn't declare it (bits {bits:#x})"
        );
        ensure!(
            bits & bit == 0 || allowed,
            "package-bit-spurious",
            "{name} is declared but nothing in this scenario could have written it (bits {bits:#x})"
        );
    }
    let bit18 = features::REQUIRED_STYLE_IDENTITIES;
    ensure!(
        bits & bit18 == 0 || may.styles,
        "package-bit-spurious",
        "style-identities (18) is declared but no style op ran (bits {bits:#x})"
    );
    Ok(())
}

/// The content of a state with every identity replaced by its place in the
/// document, so a document and its `import_copy` (which has new IDs) compare
/// equal when they hold the same content.
///
/// Annotations that cross paragraphs are compared by kind only: a copy
/// truncates them to their first paragraph (Astra 4 R3, known).
pub fn shape(state: &State) -> String {
    let mut out = String::new();
    let block_ord: BTreeMap<&str, usize> = state
        .blocks
        .iter()
        .enumerate()
        .map(|(n, b)| (b.id.as_str(), n))
        .collect();
    let mut list_ord: BTreeMap<&str, usize> = BTreeMap::new();
    for b in &state.blocks {
        if let Some(item) = &b.list {
            let next = list_ord.len();
            list_ord.entry(item.id.as_str()).or_insert(next);
        }
    }
    let ord = |map: &BTreeMap<&str, usize>, id: &str| {
        map.get(id)
            .map_or_else(|| "?".to_owned(), |n| n.to_string())
    };
    for b in &state.blocks {
        let _ = writeln!(
            out,
            "block parent={:?} kind={:?} text={:?} fmt={:?} style={:?}/{:?}/{:?} role={:?}",
            b.parent.as_deref().map(|p| ord(&block_ord, p)),
            b.kind,
            b.text,
            b.formatting,
            b.style.name,
            b.style.direct,
            state.resolved.get(b.style.resolved as usize),
            b.role,
        );
        if let Some(item) = &b.list {
            let _ = writeln!(
                out,
                "  list {} level={} value={} restart={} marker={:?}",
                ord(&list_ord, &item.id),
                item.level,
                item.value,
                item.restart,
                item.marker
            );
        }
        for e in &b.inline {
            let mut object = e.object.clone();
            object.params.remove("target");
            let _ = writeln!(out, "  inline at={} {object:?}", e.at);
        }
    }
    let mut lists: Vec<String> = state
        .lists
        .iter()
        .map(|l| {
            let mut definition = l.definition.clone();
            if let Some(d) = definition.as_mut() {
                d.base = d.base.as_deref().map(|id| ord(&list_ord, id));
            }
            format!("{definition:?} error={:?}", l.error)
        })
        .collect();
    lists.sort();
    let _ = writeln!(out, "lists {lists:?}");
    let _ = writeln!(out, "styles {:?}", state.styles);
    let _ = writeln!(out, "page {:?}", state.page_setup);
    out
}

/// The annotations of `state` by place, for comparing a document with its
/// `import_copy`: the lines of those inside one paragraph, and how many there
/// are in all. One that crosses paragraphs is only counted, because the copy
/// keeps just its first paragraph's part (Astra 4 R3, known).
pub fn annotation_lines(state: &State) -> (Vec<String>, usize) {
    let block_ord: BTreeMap<&str, usize> = state
        .blocks
        .iter()
        .enumerate()
        .map(|(n, b)| (b.id.as_str(), n))
        .collect();
    let mut lines: Vec<String> = state
        .annotations
        .iter()
        .filter(|a| a.spans.len() == 1)
        .map(|a| {
            let span = &a.spans[0];
            format!(
                "{:?} {:?} {:?} {}:{}..{}",
                a.kind,
                a.status,
                a.thread,
                block_ord
                    .get(span.node.as_str())
                    .copied()
                    .unwrap_or(usize::MAX),
                span.start,
                span.end
            )
        })
        .collect();
    lines.sort();
    (lines, state.annotations.len())
}
