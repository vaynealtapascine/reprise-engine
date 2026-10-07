//! A structural checker for the tagged PDFs we write. It parses the emitted
//! bytes with lopdf (MIT) and asserts the properties PDF/UA-1 depends on:
//! every MCID is reachable from the structure tree, the ParentTree agrees with
//! the tree, and no content is painted outside marked content or an artifact.
//! It reads nothing from the display list it was rendered from.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use lopdf::{Dictionary, Document, Object, ObjectId, content::Content};

#[derive(Debug, Default)]
pub struct Report {
    /// The structure elements' roles, in tree pre-order (the `Document` root first).
    pub roles: Vec<String>,
    /// ActualText of every content leaf, in structure order.
    pub text: String,
    /// Number of marked-content leaves reachable from the tree.
    pub leaves: usize,
    /// Number of artifact sequences across all pages.
    pub artifacts: usize,
    /// `/ID`s of Note elements.
    pub note_ids: Vec<String>,
    /// Number of Link annotations and how many have a `/StructParent`.
    pub links: usize,
    pub tagged_links: usize,
    pub lang: Option<String>,
    pub xmp: String,
    pub display_doc_title: bool,
    pub marked: bool,
    pub outline_titles: Vec<String>,
    /// For each Figure: its `/Alt`.
    pub figure_alts: Vec<String>,
    /// For each table cell role (`TD` or `TH`): its attributes as text.
    pub cell_attrs: Vec<String>,
}

fn dict<'a>(doc: &'a Document, object: &'a Object) -> Option<&'a Dictionary> {
    match object {
        Object::Dictionary(d) => Some(d),
        Object::Reference(id) => doc.get_dictionary(*id).ok(),
        _ => None,
    }
}

fn text(object: &Object) -> String {
    match object {
        Object::String(..) => lopdf::decode_text_string(object).unwrap_or_default(),
        _ => String::new(),
    }
}

/// One marked-content sequence on a page.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Mark {
    Content { mcid: i64, actual: Option<String> },
    Artifact,
    Other,
}

struct PageMarks {
    mcids: BTreeMap<i64, Option<String>>,
    artifacts: usize,
}

fn page_marks(doc: &Document, page: ObjectId) -> PageMarks {
    let content = Content::decode(&doc.get_page_content(page).unwrap()).unwrap();
    let mut stack: Vec<Mark> = Vec::new();
    let mut marks = PageMarks {
        mcids: BTreeMap::new(),
        artifacts: 0,
    };
    let mut depth = 0usize; // q/Q nesting, to catch marked content straddling it
    let mut mark_depths: Vec<usize> = Vec::new();
    for op in &content.operations {
        match op.operator.as_str() {
            "q" => depth += 1,
            "Q" => {
                depth = depth.checked_sub(1).expect("balanced q/Q");
                assert!(
                    mark_depths.last().is_none_or(|d| *d <= depth),
                    "marked content must not straddle a Q"
                );
            }
            "BDC" | "BMC" => {
                let tag = op.operands[0].as_name().unwrap().to_vec();
                let props = op.operands.get(1).and_then(|o| o.as_dict().ok());
                // krilla wraps clusters in their own ActualText spans inside ours:
                // nesting is legal, but only the outermost sequence carries an MCID.
                let nested = !stack.is_empty();
                let mark = match (tag.as_slice(), props.and_then(|d| d.get(b"MCID").ok())) {
                    (_, Some(_)) if nested => panic!("a nested sequence has an MCID"),
                    (b"Artifact", _) if nested => panic!("an artifact nests inside content"),
                    (_, None) if nested => Mark::Other,
                    (b"Artifact", _) => {
                        marks.artifacts += 1;
                        Mark::Artifact
                    }
                    (_, Some(mcid)) => {
                        let mcid = mcid.as_i64().unwrap();
                        let actual = props
                            .and_then(|d| d.get(b"ActualText").ok())
                            .map(|o| lopdf::decode_text_string(o).unwrap());
                        assert!(
                            marks.mcids.insert(mcid, actual.clone()).is_none(),
                            "MCID {mcid} used twice on a page"
                        );
                        Mark::Content { mcid, actual }
                    }
                    _ => Mark::Other,
                };
                mark_depths.push(depth);
                stack.push(mark);
            }
            "EMC" => {
                stack.pop().expect("balanced marked content");
                mark_depths.pop();
            }
            "Tj" | "TJ" | "'" | "\"" | "Do" | "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b"
            | "b*" | "sh" | "BI" => {
                let tagged = matches!(stack.first(), Some(Mark::Content { .. } | Mark::Artifact));
                assert!(
                    tagged,
                    "operator {} paints outside content or an artifact",
                    op.operator
                );
            }
            _ => {}
        }
    }
    assert!(stack.is_empty(), "unterminated marked content");
    marks
}

pub fn check(bytes: &[u8]) -> Report {
    let doc = Document::load_mem(bytes).unwrap();
    let mut report = Report::default();
    let root = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
    let catalog = doc.get_dictionary(root).unwrap();

    // Catalog.
    let mark_info = dict(&doc, catalog.get(b"MarkInfo").expect("MarkInfo")).unwrap();
    report.marked = matches!(mark_info.get(b"Marked"), Ok(Object::Boolean(true)));
    report.lang = catalog.get(b"Lang").ok().map(text);
    report.display_doc_title = catalog
        .get(b"ViewerPreferences")
        .ok()
        .and_then(|o| dict(&doc, o))
        .is_some_and(|d| matches!(d.get(b"DisplayDocTitle"), Ok(Object::Boolean(true))));
    if let Ok(meta) = catalog.get(b"Metadata") {
        let stream = doc.dereference(meta).unwrap().1.as_stream().unwrap();
        report.xmp = String::from_utf8_lossy(&stream.get_plain_content().unwrap()).into_owned();
    }
    if let Some(outlines) = catalog.get(b"Outlines").ok().and_then(|o| dict(&doc, o)) {
        let mut next = outlines.get(b"First").ok().cloned();
        let mut stack: Vec<Object> = Vec::new();
        // Pre-order over First/Next links.
        while let Some(item) = next.take().or_else(|| stack.pop()) {
            let Some(d) = dict(&doc, &item) else { continue };
            report.outline_titles.push(text(d.get(b"Title").unwrap()));
            if let Ok(n) = d.get(b"Next") {
                stack.push(n.clone());
            }
            next = d.get(b"First").ok().cloned();
        }
    }

    // Pages.
    let pages: Vec<(u32, ObjectId)> = doc.get_pages().into_iter().collect();
    let mut marks: BTreeMap<ObjectId, PageMarks> = BTreeMap::new();
    for (_, id) in &pages {
        let m = page_marks(&doc, *id);
        report.artifacts += m.artifacts;
        marks.insert(*id, m);
        let page = doc.get_dictionary(*id).unwrap();
        if let Ok(annots) = page.get(b"Annots").and_then(|a| a.as_array()) {
            for a in annots {
                let d = dict(&doc, a).unwrap();
                if d.get(b"Subtype").ok().and_then(|s| s.as_name().ok()) == Some(b"Link") {
                    report.links += 1;
                    if d.get(b"StructParent").is_ok() {
                        report.tagged_links += 1;
                    }
                }
            }
        }
    }

    // Structure tree.
    let Some(struct_root) = catalog.get(b"StructTreeRoot").ok() else {
        assert!(pages.is_empty() || marks.values().all(|m| m.mcids.is_empty()));
        return report;
    };
    let struct_root_id = struct_root.as_reference().unwrap();
    let struct_root = doc.get_dictionary(struct_root_id).unwrap();
    let has_content = marks.values().any(|m| !m.mcids.is_empty());
    let mut parent_of: BTreeMap<i64, Vec<Object>> = BTreeMap::new();
    match struct_root.get(b"ParentTree") {
        Ok(tree) => {
            let nums = dict(&doc, tree)
                .unwrap()
                .get(b"Nums")
                .unwrap()
                .as_array()
                .unwrap();
            for pair in nums.chunks(2) {
                // A page maps to an array indexed by MCID; an annotation to its
                // parent element directly.
                if let Object::Array(a) = doc.dereference(&pair[1]).unwrap().1 {
                    parent_of.insert(pair[0].as_i64().unwrap(), a.clone());
                }
            }
        }
        Err(_) => assert!(!has_content, "content without a ParentTree"),
    }

    // (page, mcid, parent element)
    let mut reached: Vec<(ObjectId, i64, ObjectId)> = Vec::new();
    let mut seen_elems: BTreeSet<ObjectId> = BTreeSet::new();
    // Stack of (child object, parent id, inherited page).
    let mut stack: Vec<(Object, ObjectId, Option<ObjectId>)> = Vec::new();
    let k = struct_root.get(b"K").expect("StructTreeRoot /K");
    let kids = |k: &Object| -> Vec<Object> {
        match doc.dereference(k).unwrap().1 {
            Object::Array(a) => a.clone(),
            _ => vec![k.clone()],
        }
    };
    for kid in kids(k).into_iter().rev() {
        stack.push((kid, struct_root_id, None));
    }
    while let Some((kid, parent, inherited_page)) = stack.pop() {
        match &kid {
            Object::Integer(mcid) => {
                reached.push((inherited_page.expect("/Pg for a bare MCID"), *mcid, parent));
            }
            Object::Reference(id) => {
                let d = doc.get_dictionary(*id).unwrap();
                if d.get(b"Type").ok().and_then(|t| t.as_name().ok()) == Some(b"MCR") {
                    let page = d.get(b"Pg").unwrap().as_reference().unwrap();
                    reached.push((page, d.get(b"MCID").unwrap().as_i64().unwrap(), parent));
                    continue;
                }
                if d.get(b"Type").ok().and_then(|t| t.as_name().ok()) == Some(b"OBJR") {
                    continue;
                }
                assert!(seen_elems.insert(*id), "structure element {id:?} twice");
                assert_eq!(
                    d.get(b"P").unwrap().as_reference().unwrap(),
                    parent,
                    "/P names the parent"
                );
                let role =
                    String::from_utf8(d.get(b"S").unwrap().as_name().unwrap().to_vec()).unwrap();
                match role.as_str() {
                    "Note" => {
                        report
                            .note_ids
                            .push(text(d.get(b"ID").expect("Note has an /ID")));
                    }
                    "Figure" => {
                        report
                            .figure_alts
                            .push(text(d.get(b"Alt").expect("Figure has /Alt")));
                    }
                    "TD" | "TH" => {
                        let attrs = d
                            .get(b"A")
                            .ok()
                            .map(|a| format!("{:?}", doc.dereference(a).unwrap().1))
                            .unwrap_or_default();
                        report.cell_attrs.push(format!("{role} {attrs}"));
                    }
                    _ => {}
                }
                report.roles.push(role);
                let page = d
                    .get(b"Pg")
                    .ok()
                    .and_then(|p| p.as_reference().ok())
                    .or(inherited_page);
                if let Ok(k) = d.get(b"K") {
                    for kid in kids(k).into_iter().rev() {
                        stack.push((kid, *id, page));
                    }
                }
            }
            Object::Dictionary(d) => {
                if d.get(b"Type").ok().and_then(|t| t.as_name().ok()) == Some(b"OBJR") {
                    continue; // an annotation, checked through its /StructParent
                }
                // An inline marked-content reference.
                let page = d.get(b"Pg").unwrap().as_reference().unwrap();
                reached.push((page, d.get(b"MCID").unwrap().as_i64().unwrap(), parent));
            }
            other => panic!("unexpected /K entry {other:?}"),
        }
    }

    // Every MCID is reachable exactly once, and every reached one exists.
    let mut count = 0;
    for (page, m) in &marks {
        for mcid in m.mcids.keys() {
            let n = reached
                .iter()
                .filter(|(p, c, _)| p == page && c == mcid)
                .count();
            assert_eq!(n, 1, "MCID {mcid} on {page:?} is reached {n} times");
            count += 1;
        }
    }
    assert_eq!(count, reached.len(), "the tree names an MCID no page has");

    // The ParentTree agrees with the tree.
    for (page, mcid, parent) in &reached {
        let d = doc.get_dictionary(*page).unwrap();
        let key = d
            .get(b"StructParents")
            .expect("page with content has /StructParents")
            .as_i64()
            .unwrap();
        let entry = &parent_of[&key][*mcid as usize];
        assert_eq!(
            entry.as_reference().unwrap(),
            *parent,
            "ParentTree[{key}][{mcid}] is the element that holds it"
        );
    }
    let entries: usize = parent_of
        .values()
        .map(|a| a.iter().filter(|o| o.as_reference().is_ok()).count())
        .sum();
    assert!(entries >= reached.len());

    // Text in structure order.
    for (page, mcid, _) in &reached {
        report.leaves += 1;
        if let Some(Some(actual)) = marks[page].mcids.get(mcid) {
            report.text.push_str(actual);
        }
    }
    report
}
