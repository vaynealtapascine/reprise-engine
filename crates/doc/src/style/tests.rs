use loro::LoroMap;

use super::*;
use crate::BlockKind;
use crate::context::Extent;

fn pt(n: i32) -> Length {
    Length::from_pt(n)
}

fn doc() -> Document {
    Document::new(1).unwrap()
}

fn codes_of(notes: &[Note]) -> Vec<&str> {
    notes.iter().map(|n| n.code.as_str()).collect()
}

fn expr_style(size: Option<&str>, line_height: Option<&str>) -> Style {
    let mut s = Style::default();
    if let Some(t) = size {
        s = s.with_expr(Property::Size, t).unwrap();
    }
    if let Some(t) = line_height {
        s = s.with_expr(Property::LineHeight, t).unwrap();
    }
    s
}

/// A paragraph with one named style defined from `style`.
fn block_with(doc: &Document, style: &Style) -> crate::NodeId {
    doc.define_style("test", style).unwrap();
    doc.append_block(BlockKind::Paragraph, "test", "text")
        .unwrap()
}

fn main_frame() -> ResolutionContext {
    ResolutionContext::default()
        .with_page(Extent::definite(pt(600), pt(800)))
        .with_current_frame("main", Extent::auto_height(pt(400)))
}

// --- stored forms -----------------------------------------------------------

#[test]
fn legacy_stored_values_still_read() {
    let doc = doc();
    let style = Style {
        size: Some(LengthExpr::Pt(pt(12))),
        line_height: Some(LengthExpr::Em(1250)),
        ..Default::default()
    };
    doc.define_style("legacy", &style).unwrap();
    assert_eq!(doc.style("legacy"), Some(style));

    // And the strings an older engine wrote, read straight from the map.
    let map = doc
        .doc
        .get_map("styles")
        .insert_container("old", LoroMap::new())
        .unwrap();
    map.insert("size", "pt:18432").unwrap();
    map.insert("line-height", "em:-500").unwrap();
    let read = doc.style("old").unwrap();
    assert_eq!(read.size, Some(LengthExpr::Pt(pt(18))));
    assert_eq!(read.line_height, Some(LengthExpr::Em(-500)));
    assert!(read.values.is_empty());
}

#[test]
fn legacy_values_keep_the_exact_original_numbers() {
    // Chains the old resolver handled, with their old answers.
    let doc = doc();
    doc.define_style(
        "a",
        &Style {
            size: Some(LengthExpr::Pt(pt(12))),
            line_height: Some(LengthExpr::Em(1500)),
            ..Default::default()
        },
    )
    .unwrap();
    doc.define_style(
        "b",
        &Style {
            parent: Some("a".into()),
            size: Some(LengthExpr::Em(750)),
            ..Default::default()
        },
    )
    .unwrap();
    let n = doc.append_block(BlockKind::Paragraph, "b", "x").unwrap();
    doc.set_overrides(
        n,
        &Style {
            size: Some(LengthExpr::Em(1111)),
            ..Default::default()
        },
    )
    .unwrap();
    let s = doc.computed_style(n).unwrap();
    let size = pt(12).mul_ratio(750, 1000).mul_ratio(1111, 1000);
    assert_eq!(s.size, size);
    assert_eq!(s.line_height, size.mul_ratio(1500, 1000));
    assert_eq!(s.explain["size"], "direct");
    assert_eq!(s.explain["line-height"], "style a");
    assert!(s.notes.is_empty() && s.bases.is_empty() && s.clamped.is_empty());
    // Nothing new is serialised when there is nothing new to say.
    let json = serde_json::to_string(&s).unwrap();
    assert!(!json.contains("notes") && !json.contains("bases"), "{json}");
}

#[test]
fn expressions_are_stored_as_versioned_text_and_round_trip() {
    let doc = doc();
    let style = expr_style(Some("calc(1.5em+2pt)"), Some("max(1.2em, 14pt)"));
    doc.define_style("calc", &style).unwrap();
    let map = match doc.doc.get_map("styles").get("calc") {
        Some(loro::ValueOrContainer::Container(loro::Container::Map(m))) => m,
        _ => panic!("the style is a map"),
    };
    assert_eq!(get_str(&map, "size").as_deref(), Some("expr1:1.5em + 2pt"));
    let read = doc.style("calc").unwrap();
    assert_eq!(read, style);
    assert_eq!(read.size, None, "an expression is not a LengthExpr");
    // Writing what was read changes nothing.
    doc.define_style("again", &read).unwrap();
    assert_eq!(doc.style("again"), Some(read));
}

#[test]
fn stored_values_this_engine_cannot_read_are_kept_and_reported() {
    let doc = doc();
    let map = doc
        .doc
        .get_map("styles")
        .insert_container("future", LoroMap::new())
        .unwrap();
    for (key, text) in [("size", "expr2:quantum(3)"), ("line-height", "expr1:1pt +")] {
        map.insert(key, text).unwrap();
    }
    let read = doc.style("future").unwrap();
    assert_eq!(
        read.values[&Property::Size],
        Authored::Unparsed("expr2:quantum(3)".into())
    );
    assert_eq!(read.size, None);

    // Saving what was read writes the same text back.
    doc.define_style("saved", &read).unwrap();
    let saved = match doc.doc.get_map("styles").get("saved") {
        Some(loro::ValueOrContainer::Container(loro::Container::Map(m))) => m,
        _ => panic!("the style is a map"),
    };
    assert_eq!(get_str(&saved, "size").as_deref(), Some("expr2:quantum(3)"));
    assert_eq!(
        get_str(&saved, "line-height").as_deref(),
        Some("expr1:1pt +")
    );

    // And resolving it reports both, and uses the defaults.
    let n = doc
        .append_block(BlockKind::Paragraph, "future", "x")
        .unwrap();
    let s = doc.computed_style(n).unwrap();
    assert_eq!(codes_of(&s.notes), ["style.unparsed", "style.unparsed"]);
    assert_eq!(s.size, pt(10));
    assert_eq!(s.line_height, pt(12));
    assert_eq!(s.explain["size"], "default");
}

#[test]
fn damaged_legacy_values_are_kept_too() {
    for text in ["pt:abc", "furlongs:5", "pt", "em:99999999999", ""] {
        let a = Authored::from_stored(text);
        assert_eq!(a, Authored::Unparsed(text.into()), "{text:?}");
        assert_eq!(a.to_stored(), text);
        assert!(a.problem().is_some());
    }
}

#[test]
fn an_oversized_stored_expression_is_reported_as_a_limit() {
    let doc = doc();
    let deep = format!("expr1:{}1pt{}", "(".repeat(40), ")".repeat(40));
    let wide = format!("expr1:{}", vec!["1pt"; 200].join(" + "));
    for text in [deep, wide] {
        let map = doc
            .doc
            .get_map("styles")
            .insert_container("big", LoroMap::new())
            .unwrap();
        map.insert("size", text.as_str()).unwrap();
        let n = doc.append_block(BlockKind::Paragraph, "big", "x").unwrap();
        let s = doc.computed_style(n).unwrap();
        assert_eq!(codes_of(&s.notes), ["style.expr-limit"], "{text}");
        assert_eq!(s.size, pt(10));
    }
}

// --- stages ---------------------------------------------------------------

#[test]
fn every_stage_is_explained() {
    let doc = doc();
    doc.define_style("base", &expr_style(Some("14pt"), Some("1.5em")))
        .unwrap();
    doc.define_style(
        "child",
        &Style {
            parent: Some("base".into()),
            ..expr_style(Some("1.25 * 1em"), None)
        },
    )
    .unwrap();
    let n = doc
        .append_block(BlockKind::Paragraph, "child", "x")
        .unwrap();

    let specified = doc.specified_style(n).unwrap();
    assert_eq!(
        specified
            .layers
            .iter()
            .map(|l| l.name.as_str())
            .collect::<Vec<_>>(),
        ["default", "style base", "style child", "direct"]
    );
    assert!(specified.notes.is_empty());

    let functions = FunctionRegistry::builtin();
    let computed = specified.compute(&functions);
    let size = &computed.size.steps;
    assert_eq!(
        size.iter().map(|s| s.layer.as_str()).collect::<Vec<_>>(),
        ["default", "style base", "style child"]
    );
    assert_eq!(size[2].inherited, "14pt");
    assert_eq!(
        size[2].value,
        ComputedLength::Absolute(pt(14).mul_ratio(5, 4))
    );
    assert_eq!(
        computed.line_height.steps[1].value.describe(),
        "1.5em",
        "a line height stays relative to the size it ends up with"
    );

    let resolution = computed.used(&ResolutionContext::default(), &functions);
    let s = &resolution.stages["size"];
    assert_eq!(s.layer.as_deref(), Some("style child"));
    assert_eq!(s.specified, "1.25 * 1em");
    assert_eq!(s.inherited, "14pt");
    assert_eq!(s.computed, "17.5pt");
    assert_eq!(s.used, pt(14).mul_ratio(5, 4));
    let lh = &resolution.stages["line-height"];
    assert_eq!(lh.layer.as_deref(), Some("style base"));
    assert_eq!(lh.specified, "1.5em");
    assert_eq!(lh.computed, "1.5em");
    assert_eq!(lh.used, pt(14).mul_ratio(5, 4).mul_ratio(3, 2));
    assert_eq!(
        resolution.style,
        doc.computed_style(n).unwrap(),
        "the stages end in exactly computed_style"
    );
}

#[test]
fn symbolic_values_wait_for_a_context() {
    let doc = doc();
    let n = block_with(&doc, &expr_style(Some("2% * frame-width(\"main\")"), None));
    let functions = FunctionRegistry::builtin();
    let computed = doc.specified_style(n).unwrap().compute(&functions);
    // Computing needed no context, and left the frame in the value.
    let last = computed.size.steps.last().unwrap();
    assert_eq!(last.value.describe(), "2% * frame-width(\"main\")");
    let deps: Vec<_> = computed.dependencies(&functions)["size"]
        .iter()
        .map(|d| d.to_string())
        .collect();
    assert_eq!(deps, ["frame-width(\"main\")"]);
    assert!(
        computed.dependencies(&functions)["line-height"]
            .iter()
            .all(|d| *d == Dependency::Em)
    );

    // Used in two frames, from the same computed style.
    let wide =
        ResolutionContext::default().with_current_frame("main", Extent::definite(pt(500), pt(900)));
    let narrow =
        ResolutionContext::default().with_current_frame("main", Extent::definite(pt(250), pt(900)));
    assert_eq!(computed.used(&wide, &functions).style.size, pt(10));
    assert_eq!(computed.used(&narrow, &functions).style.size, pt(5));
    // The entry points agree.
    assert_eq!(doc.computed_style_in(n, &wide).unwrap().size, pt(10));
}

#[test]
fn the_basis_is_explained() {
    let doc = doc();
    let n = block_with(
        &doc,
        &expr_style(Some("2% * frame-width(\"main\")"), Some("120%")),
    );
    let s = doc.computed_style_in(n, &main_frame()).unwrap();
    assert!(s.notes.is_empty(), "{:?}", s.notes);
    assert_eq!(s.size, Length(8192)); // 2% of 400pt
    assert_eq!(s.line_height, Length(9830)); // 120% of 8pt, rounded
    assert_eq!(s.bases["size"], "frame-width(\"main\") = 400pt");
    assert_eq!(
        s.bases["line-height"],
        "percentages are of the element's font size; font size = 8pt"
    );
    assert_eq!(s.explain["size"], "style test");
}

#[test]
fn percentages_are_of_the_declared_basis_of_the_property() {
    let doc = doc();
    let n = block_with(&doc, &expr_style(Some("150%"), Some("110%")));
    let s = doc.computed_style(n).unwrap();
    assert_eq!(s.size, pt(15), "size: 150% of the inherited 10pt");
    assert_eq!(
        s.line_height,
        pt(15).mul_ratio(11, 10),
        "line height: 110% of its own size"
    );
    assert_eq!(
        s.bases["size"],
        "percentages are of the inherited font size; inherited font size = 10pt"
    );
}

// --- fallbacks ------------------------------------------------------------

#[test]
fn an_unresolved_basis_falls_back_to_the_inherited_value() {
    let doc = doc();
    doc.define_style("base", &expr_style(Some("12pt"), None))
        .unwrap();
    doc.define_style(
        "child",
        &Style {
            parent: Some("base".into()),
            ..expr_style(Some("frame-width(\"nowhere\") / 20"), None)
        },
    )
    .unwrap();
    let n = doc
        .append_block(BlockKind::Paragraph, "child", "x")
        .unwrap();
    // `frame-width / 20` is a length over a number: fine to compute.
    let s = doc.computed_style(n).unwrap();
    assert_eq!(codes_of(&s.notes), ["style.basis-unresolved"]);
    assert_eq!(s.size, pt(12), "the child's layer is skipped");
    assert_eq!(
        s.explain["size"], "style base",
        "and explain says who really set it"
    );
    assert!(s.notes[0].message.starts_with("size in style child"));
    assert!(s.notes[0].message.contains("inherited"));

    // With the frame known, the child's value is used.
    let ctx = ResolutionContext::default()
        .with_named_frame("nowhere", Extent::definite(pt(400), pt(400)));
    let s = doc.computed_style_in(n, &ctx).unwrap();
    assert!(s.notes.is_empty());
    assert_eq!(s.size, pt(20));
    assert_eq!(s.explain["size"], "style child");
}

#[test]
fn a_percentage_of_an_indefinite_basis_is_reported() {
    let doc = doc();
    let n = block_with(&doc, &expr_style(Some("50% * frame-height"), None));
    let s = doc.computed_style_in(n, &main_frame()).unwrap();
    assert_eq!(codes_of(&s.notes), ["style.basis-indefinite"]);
    assert_eq!(s.size, pt(10), "falls back to the default size");
    assert_eq!(s.explain["size"], "default");
    // A definite frame is fine.
    let ctx =
        ResolutionContext::default().with_current_frame("main", Extent::definite(pt(400), pt(100)));
    let s = doc.computed_style_in(n, &ctx).unwrap();
    assert!(s.notes.is_empty());
    assert_eq!(s.size, pt(50));
}

#[test]
fn lh_in_size_or_line_height_is_a_cycle() {
    let doc = doc();
    for (size, line_height) in [
        (Some("2lh"), None),
        (None, Some("2lh")),
        (Some("1lh + 1pt"), Some("1.4em")),
        (Some("12pt"), Some("min(2lh, 20pt)")),
    ] {
        let n = block_with(&doc, &expr_style(size, line_height));
        let s = doc.computed_style(n).unwrap();
        assert_eq!(
            codes_of(&s.notes),
            ["style.cycle"],
            "{size:?} {line_height:?}"
        );
        // Defaults stand in for the layer that couldn't be used.
        assert_eq!(
            s.explain["size"],
            if size == Some("12pt") {
                "style test"
            } else {
                "default"
            }
        );
        if let Some(lh) = line_height {
            let expected = if lh.contains("lh") {
                "default"
            } else {
                "style test"
            };
            assert_eq!(s.explain["line-height"], expected);
        }
        assert!(s.size >= Length::ZERO && s.line_height >= Length::ZERO);
    }
}

#[test]
fn type_errors_and_unknown_functions_leave_the_inherited_value() {
    let doc = doc();
    for (text, code) in [
        ("1pt * 2pt", "style.type-error"),
        ("2", "style.type-error"),
        ("frob(1pt)", "style.unknown-function"),
        ("scale(1pt)", "style.type-error"),
    ] {
        let n = block_with(&doc, &expr_style(Some(text), None));
        let s = doc.computed_style(n).unwrap();
        assert_eq!(codes_of(&s.notes), [code], "{text}");
        assert_eq!(s.size, pt(10), "{text}");
        assert_eq!(s.explain["size"], "default");
    }
}

#[test]
fn a_failing_function_at_the_used_stage_falls_back() {
    let doc = doc();
    doc.define_style("base", &expr_style(Some("12pt"), None))
        .unwrap();
    doc.define_style(
        "child",
        &Style {
            parent: Some("base".into()),
            ..expr_style(Some("scale(1em, ratio(1, 0))"), None)
        },
    )
    .unwrap();
    let n = doc
        .append_block(BlockKind::Paragraph, "child", "x")
        .unwrap();
    let s = doc.computed_style(n).unwrap();
    assert!(
        codes_of(&s.notes)
            .iter()
            .all(|c| *c == "style.function-failed")
    );
    assert_eq!(s.size, pt(12));
}

#[test]
fn saturation_is_reported_but_the_value_is_used() {
    let doc = doc();
    let n = block_with(&doc, &expr_style(Some("1000000000pt * 1000000000"), None));
    let s = doc.computed_style(n).unwrap();
    assert_eq!(codes_of(&s.notes), ["style.saturated"]);
    assert_eq!(s.size, Length::MAX);
    assert_eq!(s.explain["size"], "style test");
}

#[test]
fn division_by_zero_in_a_style_is_reported() {
    let doc = doc();
    let n = block_with(&doc, &expr_style(Some("12pt / 0"), None));
    let s = doc.computed_style(n).unwrap();
    assert_eq!(codes_of(&s.notes), ["style.divide-by-zero"]);
    assert_eq!(s.size, Length::MAX);
}

#[test]
fn negative_used_lengths_are_still_clamped() {
    let doc = doc();
    let n = block_with(&doc, &expr_style(Some("-5pt"), Some("2pt - 10pt")));
    let s = doc.computed_style(n).unwrap();
    assert_eq!((s.size, s.line_height), (Length::ZERO, Length::ZERO));
    assert_eq!(s.clamped, ["size", "line-height"]);
    assert!(s.notes.is_empty(), "clamping is layout's to report");
}

#[test]
fn an_expression_size_chains_through_inherited_ems() {
    let doc = doc();
    doc.define_style(
        "base",
        &expr_style(Some("50% * frame-width(\"main\")"), None),
    )
    .unwrap();
    doc.define_style(
        "child",
        &Style {
            parent: Some("base".into()),
            size: Some(LengthExpr::Em(500)),
            ..Default::default()
        },
    )
    .unwrap();
    let n = doc
        .append_block(BlockKind::Paragraph, "child", "x")
        .unwrap();
    let functions = FunctionRegistry::builtin();
    let computed = doc.specified_style(n).unwrap().compute(&functions);
    // The legacy em is applied to the symbolic inherited size.
    assert_eq!(
        computed.size.steps[2].value.describe(),
        "0.5 * (50% * frame-width(\"main\"))"
    );
    let s = doc.computed_style_in(n, &main_frame()).unwrap();
    assert_eq!(s.size, pt(100));
    // Without the frame, the base layer is skipped; the child's 0.5em then
    // applies to the 10pt that survived, not to the failed layer.
    let s = doc.computed_style(n).unwrap();
    assert_eq!(s.size, pt(5));
    assert_eq!(codes_of(&s.notes), ["style.basis-unresolved"]);
    assert_eq!(s.explain["size"], "style child");
}

// --- the style chain ------------------------------------------------------

#[test]
fn a_parent_cycle_is_cut_and_reported() {
    let doc = doc();
    for (name, parent, pts) in [("a", "b", 11), ("b", "c", 12), ("c", "a", 13)] {
        doc.define_style(
            name,
            &Style {
                parent: Some(parent.into()),
                size: Some(LengthExpr::Pt(pt(pts))),
                ..Default::default()
            },
        )
        .unwrap();
    }
    let n = doc.append_block(BlockKind::Paragraph, "a", "x").unwrap();
    let specified = doc.specified_style(n).unwrap();
    assert_eq!(codes_of(&specified.notes), ["style.parent-cycle"]);
    assert!(specified.notes[0].message.contains("style a"));
    // Each style once, parents first: c, b, a (a's parent b is cut off at the repeat).
    assert_eq!(
        specified
            .layers
            .iter()
            .map(|l| l.name.as_str())
            .collect::<Vec<_>>(),
        ["default", "style c", "style b", "style a", "direct"]
    );
    let s = doc.computed_style(n).unwrap();
    assert_eq!(s.size, pt(11));
    assert_eq!(codes_of(&s.notes), ["style.parent-cycle"]);

    // A style that is its own parent.
    doc.define_style(
        "self",
        &Style {
            parent: Some("self".into()),
            size: Some(LengthExpr::Pt(pt(9))),
            ..Default::default()
        },
    )
    .unwrap();
    let n = doc.append_block(BlockKind::Paragraph, "self", "x").unwrap();
    let s = doc.computed_style(n).unwrap();
    assert_eq!(
        (s.size, codes_of(&s.notes)),
        (pt(9), vec!["style.parent-cycle"])
    );
}

#[test]
fn a_missing_style_is_reported() {
    let doc = doc();
    doc.define_style(
        "orphan",
        &Style {
            parent: Some("ghost".into()),
            size: Some(LengthExpr::Pt(pt(9))),
            ..Default::default()
        },
    )
    .unwrap();
    let n = doc
        .append_block(BlockKind::Paragraph, "orphan", "x")
        .unwrap();
    let s = doc.computed_style(n).unwrap();
    assert_eq!(codes_of(&s.notes), ["style.parent-missing"]);
    assert!(s.notes[0].message.contains("ghost"));
    assert_eq!(s.size, pt(9));

    let n = doc
        .append_block(BlockKind::Paragraph, "nothing", "x")
        .unwrap();
    let s = doc.computed_style(n).unwrap();
    assert_eq!(codes_of(&s.notes), ["style.parent-missing"]);
    assert_eq!(s.size, pt(10));
}

#[test]
fn a_runaway_chain_is_cut_and_reported() {
    let doc = doc();
    for i in 0..40 {
        doc.define_style(
            &format!("s{i}"),
            &Style {
                parent: Some(format!("s{}", i + 1)),
                size: Some(LengthExpr::Pt(pt(i + 1))),
                ..Default::default()
            },
        )
        .unwrap();
    }
    let n = doc.append_block(BlockKind::Paragraph, "s0", "x").unwrap();
    let s = doc.computed_style(n).unwrap();
    assert_eq!(codes_of(&s.notes), ["style.chain-too-long"]);
    assert!(s.size > Length::ZERO);
}

// --- shape of the API -----------------------------------------------------

#[test]
fn the_default_context_is_exactly_the_old_resolver() {
    // A block with no style at all: engine defaults, nothing to say.
    let doc = doc();
    let n = doc.append_block(BlockKind::Paragraph, "", "x").unwrap();
    let s = doc.computed_style(n).unwrap();
    assert_eq!(s.size, pt(10));
    assert_eq!(s.line_height, pt(12));
    assert_eq!(s.family, "Source Serif Pro");
    // Only the missing style is reported.
    assert_eq!(codes_of(&s.notes), ["style.parent-missing"]);
}

#[test]
fn style_values_serialise_compactly() {
    let s = Style {
        size: Some(LengthExpr::Pt(pt(12))),
        ..Default::default()
    };
    assert!(!serde_json::to_string(&s).unwrap().contains("values"));
    let e = expr_style(Some("1em + 2pt"), None);
    let json = serde_json::to_string(&e).unwrap();
    assert_eq!(serde_json::from_str::<Style>(&json).unwrap(), e);
}

#[test]
fn length_exprs_convert_to_equal_expressions() {
    let ctx = ResolutionContext::default();
    let f = FunctionRegistry::builtin();
    for le in [
        LengthExpr::Pt(pt(12)),
        LengthExpr::Pt(Length(1)),
        LengthExpr::Pt(pt(-3)),
        LengthExpr::Em(1200),
        LengthExpr::Em(-250),
        LengthExpr::Em(1),
    ] {
        let e = le.to_expr();
        assert_eq!(Expr::parse(&e.to_string()).unwrap(), e, "{le:?}");
        let computed = e.compute(&Scope::known(pt(10), pt(12), None), &f).unwrap();
        let used = computed.used(&crate::expr::UsedEnv {
            functions: &f,
            context: &ctx,
            em: None,
            lh: None,
        });
        assert_eq!(used.value, le.resolve(pt(10)), "{le:?}");
    }
}

// --- keyword properties (20) -------------------------------------------------

fn style_map(doc: &Document, name: &str) -> LoroMap {
    match doc.doc.get_map("styles").get(name) {
        Some(loro::ValueOrContainer::Container(loro::Container::Map(m))) => m,
        _ => panic!("the style is a map"),
    }
}

#[test]
fn orientation_keywords_round_trip_through_loro() {
    let doc = doc();
    for (o, c) in [
        (TextOrientation::Mixed, TextCombineUpright::None),
        (TextOrientation::Upright, TextCombineUpright::All),
        (TextOrientation::Sideways, TextCombineUpright::Digits(4)),
    ] {
        let style = Style {
            text_orientation: Some(o),
            text_combine_upright: Some(c),
            ..Default::default()
        };
        doc.define_style("v", &style).unwrap();
        assert_eq!(doc.style("v"), Some(style));
        let map = style_map(&doc, "v");
        assert_eq!(
            get_str(&map, TEXT_ORIENTATION).as_deref(),
            Some(o.keyword())
        );
        assert_eq!(get_str(&map, TEXT_COMBINE_UPRIGHT), Some(c.keyword()));
    }
}

#[test]
fn orientation_keywords_inherit_override_and_are_explained() {
    let doc = doc();
    doc.define_style(
        "parent",
        &Style {
            text_orientation: Some(TextOrientation::Upright),
            text_combine_upright: Some(TextCombineUpright::Digits(3)),
            ..Default::default()
        },
    )
    .unwrap();
    doc.define_style(
        "child",
        &Style {
            parent: Some("parent".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let n = doc
        .append_block(BlockKind::Paragraph, "child", "x")
        .unwrap();
    let s = doc.computed_style(n).unwrap();
    assert_eq!(s.text_orientation, TextOrientation::Upright);
    assert_eq!(s.text_combine_upright, TextCombineUpright::Digits(3));
    assert_eq!(s.explain[TEXT_ORIENTATION], "style parent");
    doc.set_overrides(
        n,
        &Style {
            text_orientation: Some(TextOrientation::Sideways),
            ..Default::default()
        },
    )
    .unwrap();
    let s = doc.computed_style(n).unwrap();
    assert_eq!(s.text_orientation, TextOrientation::Sideways);
    assert_eq!(s.explain[TEXT_ORIENTATION], "direct");
    assert_eq!(s.explain[TEXT_COMBINE_UPRIGHT], "style parent");
    assert!(s.notes.is_empty());
}

#[test]
fn unset_keywords_leave_explanations_and_json_unchanged() {
    let doc = doc();
    let n = doc.append_block(BlockKind::Paragraph, "", "x").unwrap();
    let s = doc.computed_style(n).unwrap();
    assert_eq!(s.text_orientation, TextOrientation::Mixed);
    assert_eq!(s.text_combine_upright, TextCombineUpright::None);
    assert!(!s.explain.contains_key(TEXT_ORIENTATION));
    let json = serde_json::to_string(&s).unwrap();
    assert!(!json.contains("text_orientation") && !json.contains("combine"));
    let json = serde_json::to_string(&default_style()).unwrap();
    assert!(!json.contains("text_orientation") && !json.contains("unparsed_keywords"));
}

#[test]
fn unreadable_keywords_are_kept_reported_and_inherit_through() {
    let doc = doc();
    doc.define_style(
        "base",
        &Style {
            text_orientation: Some(TextOrientation::Upright),
            text_combine_upright: Some(TextCombineUpright::All),
            ..Default::default()
        },
    )
    .unwrap();
    let map = doc
        .doc
        .get_map("styles")
        .insert_container("future", LoroMap::new())
        .unwrap();
    map.insert("parent", "base").unwrap();
    for (key, text) in [
        (TEXT_ORIENTATION, "sideways-right"),
        (TEXT_COMBINE_UPRIGHT, "digits 9"),
    ] {
        map.insert(key, text).unwrap();
    }
    let read = doc.style("future").unwrap();
    assert_eq!(read.text_orientation, None);
    assert_eq!(read.unparsed_keywords[TEXT_ORIENTATION], "sideways-right");
    // Saving what was read writes the same text back, even over a readable value.
    let mut edited = read.clone();
    edited.text_orientation = Some(TextOrientation::Mixed);
    doc.define_style("saved", &edited).unwrap();
    let saved = style_map(&doc, "saved");
    assert_eq!(
        get_str(&saved, TEXT_ORIENTATION).as_deref(),
        Some("sideways-right")
    );
    assert_eq!(
        get_str(&saved, TEXT_COMBINE_UPRIGHT).as_deref(),
        Some("digits 9")
    );
    let n = doc
        .append_block(BlockKind::Paragraph, "future", "x")
        .unwrap();
    let s = doc.computed_style(n).unwrap();
    assert_eq!(codes_of(&s.notes), ["style.unparsed", "style.unparsed"]);
    // The unreadable layer is skipped: the parent's values survive.
    assert_eq!(s.text_orientation, TextOrientation::Upright);
    assert_eq!(s.text_combine_upright, TextCombineUpright::All);
    assert_eq!(s.explain[TEXT_ORIENTATION], "style base");
}

#[test]
fn concurrent_keyword_edits_converge() {
    let a = doc();
    let n = a.append_block(BlockKind::Paragraph, "", "x").unwrap();
    a.commit();
    let b = a.fork(2).unwrap();
    a.set_overrides(
        n,
        &Style {
            text_orientation: Some(TextOrientation::Upright),
            ..Default::default()
        },
    )
    .unwrap();
    b.set_overrides(
        n,
        &Style {
            text_combine_upright: Some(TextCombineUpright::Digits(2)),
            ..Default::default()
        },
    )
    .unwrap();
    a.commit();
    b.commit();
    a.merge(&b).unwrap();
    b.merge(&a).unwrap();
    assert_eq!(a.computed_style(n).unwrap(), b.computed_style(n).unwrap());
}
