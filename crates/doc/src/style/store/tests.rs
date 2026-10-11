use std::collections::BTreeSet;

use reprise_geom::Length;

use crate::style::store::{PARA, STYLES};
use crate::{
    Authored, BlockKind, Document, Expr, LengthExpr, Property, Style, StylePatch, StyleStatus,
    codes,
};

fn size(pt: i32) -> Style {
    Style {
        size: Some(LengthExpr::Pt(Length::from_pt(pt))),
        ..Style::default()
    }
}

fn clear(keys: &[&str]) -> BTreeSet<String> {
    keys.iter().map(|k| k.to_string()).collect()
}

#[test]
fn untouched_documents_create_no_root_and_keep_legacy_reads() {
    let doc = Document::new(1).unwrap();
    doc.define_style("body", &size(11)).unwrap();
    let node = doc.append_block(BlockKind::Paragraph, "body", "x").unwrap();
    doc.commit();
    let before = doc.revision();
    assert_eq!(doc.style("body"), Some(size(11)));
    assert!(doc.style("nowhere").is_none());
    let _ = doc.computed_style(node).unwrap();
    let _ = doc.style_names();
    assert!(!doc.has_paragraph_styles());
    assert!(!doc.has_root(STYLES) && !doc.has_root(PARA));
    assert_eq!(doc.revision(), before);
}

#[test]
fn builtins_resolve_only_for_names_the_document_never_defined() {
    let doc = Document::new(1).unwrap();
    doc.define_style("", &size(10)).unwrap();
    let h = doc
        .append_block(BlockKind::Paragraph, "Heading 1", "Title")
        .unwrap();
    let style = doc.computed_style(h).unwrap();
    assert_eq!(style.size, Length::from_pt(20));
    assert_eq!(style.weight, Some(700));
    assert!(style.keep_with_next);
    assert_eq!(style.space_before, Length::from_pt(20));
    assert!(style.notes.is_empty(), "{:?}", style.notes);
    // A pre-existing definition of the same name wins wholesale.
    doc.define_style("Heading 1", &size(30)).unwrap();
    let style = doc.computed_style(h).unwrap();
    assert_eq!(style.size, Length::from_pt(30));
    assert!(!style.keep_with_next);
    // Without a base, a built-in's missing parent is not the author's problem.
    let bare = Document::new(1).unwrap();
    let t = bare
        .append_block(BlockKind::Paragraph, "Title", "t")
        .unwrap();
    assert!(bare.computed_style(t).unwrap().notes.is_empty());
}

#[test]
fn concurrent_edits_to_different_properties_of_one_style_both_win() {
    let a = Document::new(1).unwrap();
    a.commit();
    let b = a.fork(2).unwrap();
    // Both peers create the same new style concurrently.
    a.patch_named_style(
        "Quote",
        &StylePatch {
            set: size(9),
            ..Default::default()
        },
    )
    .unwrap();
    let mut indent = Style::default();
    indent.set(
        Property::IndentStart,
        Authored::Legacy(LengthExpr::Pt(Length::from_pt(18))),
    );
    b.patch_named_style(
        "Quote",
        &StylePatch {
            set: indent,
            ..Default::default()
        },
    )
    .unwrap();
    a.commit();
    b.commit();
    a.merge(&b).unwrap();
    b.merge(&a).unwrap();
    let merged = a.style("Quote").unwrap();
    assert_eq!(merged, b.style("Quote").unwrap());
    assert_eq!(merged.size, Some(LengthExpr::Pt(Length::from_pt(9))));
    assert_eq!(
        merged.get(Property::IndentStart),
        Some(Authored::Legacy(LengthExpr::Pt(Length::from_pt(18))))
    );
}

#[test]
fn deleting_an_applied_style_falls_back_to_its_parent_on_both_peers() {
    let a = Document::new(1).unwrap();
    a.define_named_style("Base", &size(14)).unwrap();
    a.define_named_style(
        "Child",
        &Style {
            parent: Some("Base".into()),
            weight: Some(700),
            ..Style::default()
        },
    )
    .unwrap();
    let node = a.append_block(BlockKind::Paragraph, "", "x").unwrap();
    a.commit();
    let b = a.fork(2).unwrap();
    a.delete_named_style("Child").unwrap();
    b.set_style_name(node, "Child").unwrap();
    a.commit();
    b.commit();
    a.merge(&b).unwrap();
    b.merge(&a).unwrap();
    for doc in [&a, &b] {
        let style = doc.computed_style(node).unwrap();
        assert_eq!(style.size, Length::from_pt(14));
        assert_eq!(style.weight, None);
        assert!(style.notes.iter().any(|n| n.code == codes::STYLE_DELETED));
    }
    assert!(!a.style_names().0.contains(&"Child".to_string()));
}

#[test]
fn rename_forwards_blocks_and_children_and_loops_are_cut() {
    let doc = Document::new(1).unwrap();
    doc.define_named_style("A", &size(12)).unwrap();
    doc.define_named_style(
        "Kid",
        &Style {
            parent: Some("A".into()),
            ..Style::default()
        },
    )
    .unwrap();
    let node = doc.append_block(BlockKind::Paragraph, "Kid", "x").unwrap();
    doc.rename_named_style("A", "B").unwrap();
    assert_eq!(
        doc.style_record("A").unwrap().status,
        StyleStatus::Renamed("B".into())
    );
    assert_eq!(doc.computed_style(node).unwrap().size, Length::from_pt(12));
    let names = doc.style_names().0;
    assert!(names.contains(&"B".to_string()) && !names.contains(&"A".to_string()));
    // A forward loop written by hostile peers terminates and is reported.
    let flat = doc.doc.get_map(STYLES);
    flat.insert("state/B", "renamed:A").unwrap();
    flat.insert("state/A", "renamed:B").unwrap();
    let style = doc.computed_style(node).unwrap();
    assert!(
        style
            .notes
            .iter()
            .any(|n| n.code == codes::STYLE_RENAME_CYCLE)
    );
}

#[test]
fn hostile_chains_terminate_with_their_codes() {
    let doc = Document::new(1).unwrap();
    for i in 0..1000 {
        doc.define_named_style(
            &format!("s{i}"),
            &Style {
                parent: Some(format!("s{}", i + 1)),
                ..Style::default()
            },
        )
        .unwrap();
    }
    let node = doc.append_block(BlockKind::Paragraph, "s0", "x").unwrap();
    let style = doc.computed_style(node).unwrap();
    assert!(
        style
            .notes
            .iter()
            .any(|n| n.code == codes::STYLE_CHAIN_TOO_LONG)
    );
    doc.define_named_style(
        "s1000",
        &Style {
            parent: Some("s0".into()),
            ..Style::default()
        },
    )
    .unwrap();
    doc.define_named_style(
        "loop",
        &Style {
            parent: Some("loop".into()),
            ..Style::default()
        },
    )
    .unwrap();
    doc.set_style_name(node, "loop").unwrap();
    let style = doc.computed_style(node).unwrap();
    assert!(
        style
            .notes
            .iter()
            .any(|n| n.code == codes::STYLE_PARENT_CYCLE)
    );
}

#[test]
fn clearing_masks_nested_values_without_creating_containers() {
    let doc = Document::new(1).unwrap();
    let node = doc.append_block(BlockKind::Paragraph, "", "x").unwrap();
    doc.set_overrides(
        node,
        &Style {
            weight: Some(700),
            color: Some([1, 2, 3, 255]),
            ..Style::default()
        },
    )
    .unwrap();
    let mut set = Style::default();
    set.set(
        Property::SpaceAfter,
        Authored::Expr(Expr::parse("1lh").unwrap()),
    );
    doc.patch_overrides(
        node,
        &StylePatch {
            set,
            clear: clear(&["weight", "alignment"]),
        },
    )
    .unwrap();
    let overrides = doc.block(node).unwrap().overrides;
    assert_eq!(overrides.weight, None);
    assert_eq!(overrides.color, Some([1, 2, 3, 255]));
    assert!(overrides.get(Property::SpaceAfter).is_some());
    // A replacement drops the observed patches too.
    doc.set_overrides(node, &Style::default()).unwrap();
    assert_eq!(doc.block(node).unwrap().overrides, Style::default());
    assert!(doc.has_paragraph_styles());
}

#[test]
fn malformed_and_unknown_patches_are_refused_or_kept_verbatim() {
    let doc = Document::new(1).unwrap();
    let node = doc.append_block(BlockKind::Paragraph, "", "x").unwrap();
    for patch in [
        StylePatch {
            clear: clear(&["no-such-property"]),
            ..Default::default()
        },
        StylePatch {
            set: Style {
                weight: Some(700),
                ..Style::default()
            },
            clear: clear(&["weight"]),
        },
        StylePatch {
            set: Style {
                widows: Some(0),
                ..Style::default()
            },
            ..Default::default()
        },
    ] {
        assert!(doc.patch_overrides(node, &patch).is_err());
    }
    assert!(
        doc.define_named_style(&"x".repeat(257), &Style::default())
            .is_err()
    );
    assert!(doc.define_named_style("a\u{7}", &Style::default()).is_err());
    // Values a newer or hostile peer stored are kept and reported, not used.
    let flat = doc.doc.get_map(STYLES);
    flat.insert("widows/W", "900").unwrap();
    flat.insert("indent-start/W", "expr9:future").unwrap();
    flat.insert("keep-with-next/W", 7).unwrap();
    let w = doc.style("W").unwrap();
    assert_eq!(
        w.unparsed_keywords.get("widows").map(String::as_str),
        Some("900")
    );
    assert!(matches!(
        w.get(Property::IndentStart),
        Some(Authored::Unparsed(_))
    ));
    doc.set_style_name(node, "W").unwrap();
    let style = doc.computed_style(node).unwrap();
    assert_eq!(style.widows, None);
    assert_eq!(style.indent_start, Length::ZERO);
    assert!(style.notes.iter().any(|n| n.code == codes::STYLE_UNPARSED));
}

#[test]
fn paragraph_lengths_resolve_through_the_four_stages() {
    use crate::context::{Extent, ResolutionContext};
    let doc = Document::new(1).unwrap();
    let mut style = size(10);
    for (p, text) in [
        (Property::IndentStart, "10%"),
        (Property::IndentFirst, "-2em"),
        (Property::SpaceBefore, "1lh"),
        (Property::SpaceAfter, "-5pt"),
        (Property::IndentEnd, "min(1em, 3pt)"),
    ] {
        style.set(p, Authored::Expr(Expr::parse(text).unwrap()));
    }
    doc.define_named_style("P", &style).unwrap();
    let node = doc.append_block(BlockKind::Paragraph, "P", "x").unwrap();
    let ctx = ResolutionContext::default().with_block(Extent::auto_height(Length::from_pt(400)));
    let used = doc.computed_style_in(node, &ctx).unwrap();
    assert_eq!(used.indent_start, Length::from_pt(40));
    assert_eq!(used.indent_first, Length::from_pt(-20));
    assert_eq!(used.space_before, used.line_height);
    assert_eq!(used.space_after, Length::ZERO);
    assert!(used.clamped.contains(&"space-after".to_string()));
    assert!(!used.clamped.contains(&"indent-first".to_string()));
    assert_eq!(used.indent_end, Length::from_pt(3));
    // Without a block, the percentage has no basis: zero, and a warning.
    let default = doc.computed_style(node).unwrap();
    assert_eq!(default.indent_start, Length::ZERO);
    assert!(
        default
            .notes
            .iter()
            .any(|n| n.code == codes::STYLE_BASIS_UNRESOLVED)
    );
    // Legacy JSON is unchanged for styles that don't use the new fields.
    let plain = Document::new(1).unwrap();
    let n = plain.append_block(BlockKind::Paragraph, "", "x").unwrap();
    let json = serde_json::to_string(&plain.computed_style(n).unwrap()).unwrap();
    assert!(!json.contains("indent") && !json.contains("keep") && !json.contains("space"));
}
