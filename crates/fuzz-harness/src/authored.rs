//! A canonical, comparable digest of authored state (05).
//!
//! Only authored data goes in: the live content tree, relations, named styles
//! and the page template choice. Layout results never do. Two documents with
//! equal digests hold the same authored content, whatever their histories.

use reprise_doc::{Document, NodeId, RangeId};

/// Style names the harness defines and the digest looks up. A document has no
/// style listing, so the digest probes this fixed pool.
pub const STYLE_POOL: [&str; 6] = ["", "body", "note", "alpha", "beta", "gamma"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Authored {
    pub blocks: Vec<String>,
    pub relations: Vec<String>,
    pub styles: Vec<String>,
    pub templates: Vec<String>,
}

/// Live blocks in document order (children included), skipping any node that
/// isn't a readable block.
pub fn live_blocks(doc: &Document) -> Vec<NodeId> {
    doc.document_order()
        .into_iter()
        .filter(|&node| doc.block(node).is_ok())
        .collect()
}

pub fn authored(doc: &Document) -> Authored {
    let blocks = live_blocks(doc)
        .into_iter()
        .filter_map(|node| {
            let block = doc.block(node).ok()?;
            Some(format!(
                "{node}|parent={:?}|{:?}|{:?}|{:?}|{:?}|image={:?}|table={:?}",
                doc.parent_of(node),
                block.kind,
                block.style,
                block.overrides,
                block.text.to_string(),
                doc.image_record(node).ok().flatten(),
                doc.table_role(node).ok().flatten(),
            ))
        })
        .collect();
    let relations = doc
        .relations()
        .into_iter()
        .map(|(id, relation)| format!("{id}|{relation:?}"))
        .collect();
    let styles = STYLE_POOL
        .iter()
        .map(|name| format!("{name:?}={:?}", doc.style(name)))
        .collect();
    let mut templates: Vec<String> = doc
        .page_templates()
        .into_iter()
        .map(|t| format!("{}={:?}", t.name, t.template))
        .collect();
    templates.push(format!("choice={:?}", doc.page_template()));
    Authored {
        blocks,
        relations,
        styles,
        templates,
    }
}

/// Where the given persistent ranges resolve. Peers must agree on this after a
/// full sync; undo need not restore it, because re-inserted text is new text.
pub fn ranges(doc: &Document, ids: &[RangeId]) -> Vec<String> {
    ids.iter()
        .map(|id| format!("{id}={:?}", doc.resolve_range(*id)))
        .collect()
}

/// All text of the live blocks, in document order, with no separators. Joins
/// and splits keep this string unchanged, which makes it the invariant that
/// copy and paste must preserve.
pub fn concat_text(doc: &Document) -> String {
    live_blocks(doc)
        .into_iter()
        .filter_map(|node| doc.block(node).ok())
        .map(|block| block.text.to_string())
        .collect()
}

/// Human-readable difference, for violation messages.
pub fn diff(left: &Authored, right: &Authored) -> String {
    let mut out = String::new();
    for (name, a, b) in [
        ("blocks", &left.blocks, &right.blocks),
        ("relations", &left.relations, &right.relations),
        ("styles", &left.styles, &right.styles),
        ("templates", &left.templates, &right.templates),
    ] {
        if a != b {
            out.push_str(&format!("{name}:\n  left:  {a:#?}\n  right: {b:#?}\n"));
        }
    }
    out
}
