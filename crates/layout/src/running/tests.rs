use super::*;
use reprise_doc::sections::{BindingView, SectionProperties, SectionView};

fn view(id: SectionId, entries: &[(&str, &str, &str, Option<NodeId>)]) -> SectionView {
    // (slot, variant, authored, story)
    let mut bindings = Vec::new();
    for slot in ["header", "footer", "background"] {
        for variant in ["default", "first", "even"] {
            let found = entries.iter().find(|e| e.0 == slot && e.1 == variant);
            bindings.push(match found {
                Some((_, _, authored, story)) => BindingView {
                    slot: slot.into(),
                    variant: variant.into(),
                    authored: (*authored).into(),
                    story: *story,
                    resolved: if *authored == "story" { *story } else { None },
                    from_section: id,
                },
                None => BindingView {
                    slot: slot.into(),
                    variant: variant.into(),
                    authored: "inherit".into(),
                    story: None,
                    resolved: None,
                    from_section: SectionId::First,
                },
            });
        }
    }
    SectionView {
        id,
        life: "0".into(),
        first_block: None,
        end: None,
        own: SectionProperties::new(),
        bindings,
        unknown_keys: Vec::new(),
    }
}

fn node(n: u64) -> NodeId {
    NodeId::parse(&format!("{n}@1")).unwrap()
}

#[test]
fn registry_is_ordered_and_tokens_round_trip() {
    let names: Vec<_> = SLOTS.iter().map(|s| s.name).collect();
    assert_eq!(names, ["header", "footer", "background"]);
    for (i, spec) in SLOTS.iter().enumerate() {
        let id = SlotId(u16::try_from(i).unwrap());
        assert_eq!(id.token(), spec.name);
        assert_eq!(SlotId::parse(spec.name), Some(id));
        assert_eq!(slot_of_role(&role_of(id)), Some(id));
    }
    assert_eq!(SlotId::parse("nope"), None);
    assert_eq!(SlotId(99).token(), "?");
    assert_eq!(slot_of_role("main"), None);
    // The background differs from header and footer in box, reservation and paint order.
    assert_eq!(SLOTS[2].side, Side::None);
    assert_eq!(SLOTS[2].paint, Paint::Below);
    assert_eq!(SLOTS[0].paint, Paint::Above);
    assert_eq!(SLOTS[2].artifact, Artifact::Watermark);
}

#[test]
fn slot_ids_serialise_as_their_token() {
    assert_eq!(serde_json::to_string(&SlotId(1)).unwrap(), "\"footer\"");
}

#[test]
fn inheritance_is_one_forward_scan_and_none_is_defined() {
    let a = node(1);
    let b = node(2);
    let views = vec![
        view(SectionId::First, &[("header", "default", "story", Some(a))]),
        view(SectionId::Block(node(10)), &[]),
        view(
            SectionId::Block(node(11)),
            &[("header", "default", "none", None)],
        ),
        view(SectionId::Block(node(12)), &[]),
        view(
            SectionId::Block(node(13)),
            &[("header", "default", "story", Some(b))],
        ),
        view(
            SectionId::Block(node(14)),
            &[("footer", "first", "story", None)],
        ),
    ];
    let b = Bindings::resolve(&views);
    assert!(b.any());
    let cell = |s: usize| b.of(s).unwrap().get(0, Variant::Default).0.clone();
    assert_eq!(cell(0), Effective::Story(a));
    assert_eq!(cell(1), Effective::Story(a));
    assert_eq!(cell(2), Effective::None);
    assert_eq!(cell(3), Effective::None);
    assert!(matches!(cell(4), Effective::Story(_)));
    // The explicit story that names nothing live renders empty and never
    // falls through to an earlier footer; it stays missing downstream.
    assert!(matches!(
        b.of(5).unwrap().get(1, Variant::First).0,
        Effective::Missing(_)
    ));
    // The authoring section is remembered.
    assert_eq!(
        b.of(3).unwrap().get(0, Variant::Default).1,
        SectionId::Block(node(11))
    );
}

#[test]
fn no_bindings_means_no_regions() {
    let views = vec![
        view(SectionId::First, &[]),
        view(SectionId::Block(node(3)), &[]),
    ];
    assert!(!Bindings::resolve(&views).any());
    assert!(
        !Bindings::resolve(&[view(
            SectionId::First,
            &[("header", "default", "none", None)]
        )])
        .any()
    );
    assert!(!Bindings::resolve(&[]).any());
}

#[test]
fn variants_follow_first_then_even_then_default_and_none_stops_fallback() {
    let s = node(1);
    let views = vec![view(
        SectionId::First,
        &[
            ("header", "default", "story", Some(s)),
            ("header", "first", "story", Some(s)),
            ("header", "even", "none", None),
            ("footer", "default", "story", Some(s)),
            ("footer", "even", "story", Some(s)),
        ],
    )];
    let b = Bindings::resolve(&views);
    let sec = b.of(0).unwrap();
    let page = |index, first_owned, use_first, use_even| PageFacts {
        index,
        first_owned,
        use_first,
        use_even,
        filler: false,
    };
    // header: first page, switch on -> first.
    assert_eq!(select(sec, 0, &page(0, true, true, true)), Variant::First);
    // Switch off, or not the first owned page, falls to even / default.
    assert_eq!(
        select(sec, 0, &page(0, true, false, false)),
        Variant::Default
    );
    assert_eq!(
        select(sec, 0, &page(2, false, true, true)),
        Variant::Default
    );
    // Physical, 1-based parity: index 1 is page 2 (even), index 2 is page 3.
    assert_eq!(select(sec, 0, &page(1, false, true, true)), Variant::Even);
    assert_eq!(
        select(sec, 0, &page(2, false, true, true)),
        Variant::Default
    );
    // The footer's first variant is unset, so an unset `first` does not hide
    // `default` on the first page, and an even page with `even` uses it.
    assert_eq!(select(sec, 1, &page(0, true, true, true)), Variant::Default);
    assert_eq!(select(sec, 1, &page(1, false, true, true)), Variant::Even);
    // A filler never takes the first variant.
    let filler = PageFacts {
        filler: true,
        ..page(0, true, true, true)
    };
    assert_eq!(select(sec, 0, &filler), Variant::Default);
    // An explicit none for `even` is defined: it selects the empty region.
    assert_eq!(
        sec.get(0, select(sec, 0, &page(1, false, false, true))).0,
        &Effective::None
    );
}

#[test]
fn boxes_of_the_three_slots() {
    let (w, h) = (Length::from_pt(600), Length::from_pt(800));
    let (l, r) = (Length::from_pt(50), Length::from_pt(70));
    let header = region_rect(
        Region::HeaderBand,
        w,
        h,
        l,
        r,
        Length::from_pt(36),
        Length::from_pt(20),
    );
    assert_eq!(header.origin.x, l);
    assert_eq!(header.origin.y, Length::from_pt(36));
    assert_eq!(header.width, Length::from_pt(480));
    assert_eq!(header.height, Length::from_pt(20));
    let footer = region_rect(
        Region::FooterBand,
        w,
        h,
        l,
        r,
        Length::from_pt(36),
        Length::from_pt(20),
    );
    assert_eq!(footer.origin.y, Length::from_pt(800 - 36 - 20));
    assert_eq!(footer.width, Length::from_pt(480));
    let page = region_rect(Region::Page, w, h, l, r, Length::ZERO, Length::ZERO);
    assert_eq!(
        (page.origin.x, page.origin.y, page.width, page.height),
        (Length::ZERO, Length::ZERO, w, h)
    );
    // Margins wider than the page do not make negative boxes.
    let tight = region_rect(Region::HeaderBand, w, h, w, w, Length::ZERO, Length::ZERO);
    assert_eq!(tight.width, Length::ZERO);
}

fn budget(h: i32, t: i32, b: i32, hd: i32, fd: i32) -> Budget {
    Budget::new(Length(h), Length(t), Length(b), Length(hd), Length(fd))
}

#[test]
fn budget_keeps_a_quarter_of_the_authored_body() {
    // H = 1000, T = B = 100: D = 800, keep = 200.
    let b = budget(1000, 100, 100, 36, 36);
    assert_eq!(b.keep, 200);
    // A header that fits the margin reserves nothing.
    assert_eq!(b.top_for(80), 100);
    // A taller one takes the difference.
    assert_eq!(b.top_for(300), 300);
    // The cap leaves the body its quarter even against a huge header.
    assert_eq!(b.top_for(i64::MAX), 1000 - 100 - 200);
    assert_eq!(b.header_cap(), Length(1000 - 100 - 200 - 36));
    // The footer takes what the header left.
    let top = b.top_for(500);
    assert_eq!(top, 500);
    assert_eq!(b.footer_cap(top), Length(1000 - 500 - 200 - 36));
    assert_eq!(b.bottom_for(top, i64::MAX), 1000 - 500 - 200);
    // Both capped: exactly `keep` remains.
    let bottom = b.bottom_for(b.top_for(i64::MAX), i64::MAX);
    assert_eq!(1000 - b.top_for(i64::MAX) - bottom, 200);
}

#[test]
fn budget_invariant_holds_for_extreme_inputs() {
    let values: [i32; 10] = [0, 1, 2, 3, 7, 100, 1023, 4096, 1 << 20, 14_745_600];
    for &h in &values {
        for &t in &values {
            for &b in &values {
                if t.saturating_add(b) >= h {
                    continue;
                }
                for &need_h in &[0i64, 1, 100, 1 << 40, i64::MAX] {
                    for &need_f in &[0i64, 1, 100, 1 << 40, i64::MAX] {
                        let budget = budget(h, t, b, 36, 36);
                        let top = budget.top_for(need_h);
                        let bottom = budget.bottom_for(top, need_f);
                        assert!(
                            top >= i64::from(t),
                            "header never takes the authored margin"
                        );
                        assert!(bottom >= i64::from(b));
                        assert!(
                            i64::from(h) - top - bottom >= budget.keep && budget.keep >= 1,
                            "h={h} t={t} b={b}: body {} < keep {}",
                            i64::from(h) - top - bottom,
                            budget.keep
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn budget_survives_a_degenerate_page() {
    // No positive text area (the geometry merge refuses it, but this must not panic).
    let b = budget(10, 20, 20, 36, 36);
    assert!(b.keep >= 1);
    let _ = (
        b.header_cap(),
        b.footer_cap(b.top_for(5)),
        b.bottom_for(0, 5),
    );
    let b = budget(i32::MAX, i32::MAX, i32::MAX, i32::MAX, i32::MAX);
    let top = b.top_for(i64::MAX);
    let _ = b.bottom_for(top, i64::MAX);
    let b = budget(100, -5, -5, -1, -1);
    assert_eq!((b.top, b.bottom, b.header_distance), (0, 0, 0));
    assert_eq!(b.need(b.header_distance, Length(-3), false), 0);
    assert_eq!(
        b.need(36, Length(10), true),
        0,
        "an empty story reserves nothing"
    );
    assert_eq!(b.need(36, Length(10), false), 46);
}
