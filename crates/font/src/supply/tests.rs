use super::*;
const SERIF: &[u8] = include_bytes!("../../../../fixtures/fonts/SourceSerifPro-Regular.otf");
fn decl(weight: u16, style: FontStyle, stretch: u16) -> FontDeclaration {
    FontDeclaration {
        family: "Test".into(),
        descriptors: Descriptors {
            weight,
            style,
            stretch,
        },
        face_index: 0,
    }
}
#[test]
fn defaults_are_pinned_and_overrides_are_checked() {
    let mut store = FontStore::default();
    let ids = store.generic_ids();
    assert_eq!(ids.len(), 4);
    for (_, id) in &ids {
        let f = store.get(id).unwrap();
        assert!(f.covers('a'));
        assert!(!f.version().is_empty());
    }
    assert_eq!(ids[1].1.hash, "08df266400933d3178d081a45f94a088");
    assert_eq!(ids[2].1.hash, "9f9664e2edf6f045c11e774f9bd0be69");
    assert_eq!(ids[3].1.hash, "21808625578fe8d8cd10cb684be546dc");
    store
        .set_generic(GenericFamily::Serif, ids[2].1.clone())
        .unwrap();
    assert_eq!(store.generic(GenericFamily::Serif).id(), &ids[2].1);
    let bad = FaceId {
        family: "missing".into(),
        hash: "é".into(),
    };
    assert!(
        store
            .set_generic(GenericFamily::Serif, bad.clone())
            .is_err()
    );
    assert!(!format!("{bad:?}").is_empty());
    assert_eq!(store.generic(GenericFamily::Serif).id(), &ids[2].1);
}
#[test]
fn css_matching_priorities_and_registration_order() {
    let mut store = FontStore::default();
    let a = store
        .register(SERIF, decl(400, FontStyle::Normal, 1000))
        .unwrap();
    let sans = bundled()[1].data();
    let b = store
        .register(sans, decl(500, FontStyle::Normal, 1000))
        .unwrap();
    assert_eq!(
        store
            .match_family(
                "test",
                Descriptors {
                    weight: 450,
                    ..Descriptors::default()
                }
            )
            .unwrap()
            .face
            .id(),
        &b
    );
    assert_eq!(
        store
            .match_family(
                "Test",
                Descriptors {
                    weight: 350,
                    ..Descriptors::default()
                }
            )
            .unwrap()
            .face
            .id(),
        &a
    );
    assert_eq!(
        store
            .match_family("Test", Descriptors::default())
            .unwrap()
            .notes
            .len(),
        0
    );
    assert_eq!(stretch_rank(900, 1000).0, 0);
    assert!(stretch_rank(900, 1000) < stretch_rank(1001, 1000));
    assert!(stretch_rank(1100, 1050) < stretch_rank(1049, 1050));
    assert!(
        style_rank(FontStyle::Oblique, FontStyle::Italic)
            < style_rank(FontStyle::Normal, FontStyle::Italic)
    );
    assert!(weight_rank(500, 400) < weight_rank(399, 400));
    assert!(weight_rank(600, 550) < weight_rank(549, 550));
    let mut first = FontStore::default();
    let mut second = FontStore::default();
    for bytes in [SERIF, sans] {
        first
            .register(bytes, decl(400, FontStyle::Italic, 900))
            .unwrap();
    }
    for bytes in [sans, SERIF] {
        second
            .register(bytes, decl(400, FontStyle::Italic, 900))
            .unwrap();
    }
    assert_eq!(
        first
            .match_family("Test", Descriptors::default())
            .unwrap()
            .face
            .id(),
        second
            .match_family("Test", Descriptors::default())
            .unwrap()
            .face
            .id()
    );
}
#[test]
fn parser_boundary_is_bounded_and_never_panics() {
    for len in (0..SERIF.len()).step_by(137) {
        assert!(Face::from_bytes(&SERIF[..len]).is_err());
    }
    for pos in (0..SERIF.len()).step_by(251) {
        let mut bytes = SERIF.to_vec();
        bytes[pos] ^= 0xff;
        if let Ok(face) = Face::from_bytes(bytes) {
            let _ = face.covers('é');
            let _ = face.outline(0);
        }
    }
    let font = Face::from_bytes(SERIF).unwrap();
    let head = font
        .font_ref()
        .table_directory()
        .table_records()
        .iter()
        .find(|r| r.tag() == skrifa::raw::types::Tag::new(b"head"))
        .unwrap()
        .offset() as usize;
    let mut zero = SERIF.to_vec();
    zero[head + 18..head + 20].fill(0);
    assert!(Face::from_bytes(zero).is_err());
    let mut tables = SERIF.to_vec();
    tables[20..24].fill(0xff);
    assert!(Face::from_bytes(tables).is_err());
    let mut invalid = decl(0, FontStyle::Normal, 1000);
    assert!(Face::declared(SERIF, Some(invalid.clone())).is_err());
    invalid.descriptors.weight = 400;
    invalid.face_index = u32::MAX;
    assert!(Face::declared(SERIF, Some(invalid)).is_err());
    assert!(Face::from_bytes(vec![0; MAX_FONT_BYTES + 1]).is_err());
}
