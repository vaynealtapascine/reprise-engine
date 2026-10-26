//! Astra's review of Stage B part 2, finding 3: what a memoised flow
//! transition read of its section is part of its key. The restart a continuous
//! section applies on its first owned page is read when that page is made,
//! which may be pages after the section starts; keying the section by identity,
//! lifetime and number format alone missed it. The audit tests edit other
//! properties in place (one lifetime, so identity and lifetime do not change)
//! and compare one persistent session with full layout each time.
mod common;
use common::*;
use reprise_doc::Document;
use reprise_doc::NodeId;
use reprise_doc::sections::{BreakKind, SectionId};
use reprise_geom::Length;
use reprise_layout::incremental::LayoutSession;
use reprise_layout::{Engine, LayoutSnapshot};
use serde_json::{Value, json};

/// A 200 pt square page with 20 pt margins; a one-line first section, then a
/// continuous section with `lines` forced lines that fills several pages.
/// Returns the continuous section's start block and lifetime.
fn continuous(doc: &Document, lines: usize, props: &[(&str, Value)]) -> (NodeId, String) {
    doc.set_page_setup_patch(reprise_doc::page_setup::PageSetupPatch {
        width: Some(Length::from_pt(200)),
        height: Some(Length::from_pt(200)),
        top: Some(Length::from_pt(20)),
        bottom: Some(Length::from_pt(20)),
        left: Some(Length::from_pt(20)),
        right: Some(Length::from_pt(20)),
    })
    .unwrap();
    doc.write_section_properties(
        SectionId::First,
        "0",
        &[("mode".into(), json!("paged"))].into(),
        &[],
    )
    .unwrap();
    block(doc, "one");
    let b = block(doc, &vec!["line"; lines].join("\n"));
    let life = doc.mint_lifetime(b).unwrap();
    let mut all = vec![("start".to_owned(), json!("continuous"))];
    all.extend(props.iter().map(|(k, v)| ((*k).to_owned(), v.clone())));
    doc.activate_start(b, &life, BreakKind::Section, &all.into_iter().collect())
        .unwrap();
    doc.commit();
    (b, life)
}

fn labels(s: &LayoutSnapshot) -> Vec<String> {
    s.sections
        .as_ref()
        .unwrap()
        .pages
        .iter()
        .map(|p| p.label.clone())
        .collect()
}

/// Lays out, runs `edit` on the same document, and checks the session's second
/// layout against a full one. The edit must have changed the layout, or the
/// test would prove nothing.
fn edited(
    engine: &Engine,
    doc: &Document,
    edit: impl FnOnce(),
) -> (LayoutSnapshot, LayoutSnapshot, LayoutSnapshot) {
    let mut session = LayoutSession::new(engine);
    let before = session.layout(doc).unwrap();
    assert_eq!(before, engine.layout(doc));
    edit();
    doc.commit();
    let after = session.layout(doc).unwrap();
    let full = engine.layout(doc);
    (before, after, full)
}

#[test]
fn continuous_section_pending_restart_invalidates_flow() {
    let (engine, doc) = setup();
    let (b, life) = continuous(&doc, 40, &[("numbering.restart", json!("5"))]);
    let (before, after, full) = edited(&engine, &doc, || {
        doc.write_section_properties(
            SectionId::Block(b),
            &life,
            &[("numbering.restart".into(), json!("20"))].into(),
            &[],
        )
        .unwrap();
    });
    assert_eq!(before.sections.as_ref().unwrap().pages[0].bands.len(), 2);
    assert_eq!(labels(&before), ["1", "5", "6", "7"]);
    assert_eq!(labels(&full), ["1", "20", "21", "22"]);
    assert_eq!(labels(&after), labels(&full));
    assert!(
        after == full,
        "pending restart in shared page must invalidate flow"
    );
}

#[test]
fn an_inherited_number_format_edited_in_place_invalidates_the_inheriting_flow() {
    // The continuous section sets no format of its own: it takes the first
    // section's. Editing the first section's format in place leaves the
    // continuous section's own record untouched.
    let (engine, doc) = setup();
    let (_, _) = continuous(&doc, 40, &[]);
    doc.write_section_properties(
        SectionId::First,
        "0",
        &[("numbering.format".into(), json!("lower-roman"))].into(),
        &[],
    )
    .unwrap();
    doc.commit();
    let (before, after, full) = edited(&engine, &doc, || {
        doc.write_section_properties(
            SectionId::First,
            "0",
            &[("numbering.format".into(), json!("upper-alpha"))].into(),
            &[],
        )
        .unwrap();
    });
    assert_eq!(labels(&before)[1], "ii");
    assert_eq!(labels(&full)[1], "B");
    assert_eq!(labels(&after), labels(&full));
    assert!(
        after == full,
        "an inherited format edit must invalidate flow"
    );
}

#[test]
fn a_column_count_edited_in_place_invalidates_the_flow() {
    let (engine, doc) = setup();
    let (b, life) = continuous(&doc, 40, &[("columns.count", json!(1))]);
    let (before, after, full) = edited(&engine, &doc, || {
        doc.write_section_properties(
            SectionId::Block(b),
            &life,
            &[("columns.count".into(), json!(2))].into(),
            &[],
        )
        .unwrap();
    });
    assert_ne!(
        before.pages.len(),
        full.pages.len(),
        "two columns hold twice the lines, so the page count changes"
    );
    assert!(after == full, "a column count edit must invalidate flow");
}

#[test]
fn a_restart_edit_in_the_first_section_renumbers_the_document() {
    let (engine, doc) = setup();
    let (_, _) = continuous(&doc, 40, &[]);
    doc.write_section_properties(
        SectionId::First,
        "0",
        &[("numbering.restart".into(), json!("10"))].into(),
        &[],
    )
    .unwrap();
    doc.commit();
    let (before, after, full) = edited(&engine, &doc, || {
        doc.write_section_properties(
            SectionId::First,
            "0",
            &[("numbering.restart".into(), json!("30"))].into(),
            &[],
        )
        .unwrap();
    });
    assert_eq!(labels(&before)[0], "10");
    assert_eq!(labels(&full)[0], "30");
    assert!(
        after == full,
        "the first section's restart numbers page one"
    );
}

#[test]
fn a_column_rule_edited_in_place_invalidates_the_flow() {
    // Pages the transition makes carry the section's column rules.
    let (engine, doc) = setup();
    let (b, life) = continuous(
        &doc,
        40,
        &[("columns.count", json!(2)), ("columns.rule", json!(2048))],
    );
    let (before, after, full) = edited(&engine, &doc, || {
        doc.write_section_properties(
            SectionId::Block(b),
            &life,
            &[("columns.rule", json!(4096))]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v))
                .collect(),
            &[],
        )
        .unwrap();
    });
    let rules = |s: &LayoutSnapshot| s.sections.as_ref().unwrap().rules.clone();
    assert!(!rules(&before).is_empty());
    assert_ne!(rules(&before), rules(&full), "the rule is wider");
    assert_eq!(rules(&after), rules(&full));
    assert!(after == full, "a column rule edit must invalidate flow");
}
