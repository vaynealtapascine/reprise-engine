//! Fonts declared once per workspace and shared by handle.
use reprise::*;

const ATKINSON: &[u8] =
    include_bytes!("../../../fixtures/fonts/AtkinsonHyperlegibleNext-Regular.ttf");

fn declaration() -> Payload<FontDeclaration> {
    Payload::new(FontDeclaration {
        family: "Universal".into(),
        weight: 400,
        style: FontStyle::Normal,
        stretch: 1000,
        face_index: 0,
    })
}
fn create(w: &Workspace, id: &str, peer: &str) -> DocumentSession {
    w.create(&Payload::new(Create {
        document_id: id.into(),
        peer_id: peer.into(),
    }))
    .unwrap()
}
fn lay_out(s: &mut DocumentSession) {
    let mut job = s
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    while !job.step(s, 1000).unwrap().data.complete {}
}
fn text(s: &mut DocumentSession, families: Option<Vec<String>>) {
    let node = s
        .apply(&Payload::new(Transaction {
            commands: vec![Command::InsertBlock {
                parent: None,
                index: 0,
                block_kind: BlockKind::Paragraph,
                text: "Shared faces".into(),
                style: Style::default(),
            }],
        }))
        .unwrap()
        .data
        .blocks[0]
        .clone();
    if let Some(families) = families {
        s.apply(&Payload::new(Transaction {
            commands: vec![Command::FormatText {
                node,
                start: 0,
                end: 6,
                style: TextStyle {
                    families: Some(families),
                    ..TextStyle::default()
                },
            }],
        }))
        .unwrap();
    }
}
fn codes(s: &DocumentSession) -> Vec<String> {
    s.diagnostics().data.into_iter().map(|d| d.code).collect()
}

#[test]
fn sessions_share_one_parsed_face_by_handle() {
    let w = Workspace::new();
    let face = w.declare_font(&declaration(), ATKINSON).unwrap().data;
    // Only the workspace holds it until a session takes it.
    assert_eq!(w.font_handles(&face), Some(1));
    let mut a = create(&w, "00000000000000000000000000000001", "1");
    let mut b = create(&w, "00000000000000000000000000000002", "2");
    assert_eq!(w.font_handles(&face), Some(3), "workspace + two sessions");
    let c = create(&w, "00000000000000000000000000000003", "3");
    assert_eq!(w.font_handles(&face), Some(4));
    drop(c);
    assert_eq!(w.font_handles(&face), Some(3), "a closed session lets go");
    // Declaring it again is the same identity: nothing is parsed or held twice.
    assert_eq!(w.declare_font(&declaration(), ATKINSON).unwrap().data, face);
    assert_eq!(w.font_handles(&face), Some(3));
    // The shared face is usable without any per-session declaration.
    text(&mut a, Some(vec!["Universal".into()]));
    text(&mut b, Some(vec!["Universal".into()]));
    lay_out(&mut a);
    lay_out(&mut b);
    for s in [&a, &b] {
        assert!(
            !codes(s).iter().any(|c| c.starts_with("font.")),
            "{:?}",
            codes(s)
        );
    }
    // A session saves the shared face it used, as its own package resource.
    let saved = a.save().unwrap().data.bytes;
    let resources = Workspace::new()
        .open(
            &Payload::new(Open {
                peer_id: "9".into(),
            }),
            &saved,
        )
        .unwrap()
        .resources()
        .unwrap()
        .data;
    assert!(
        resources
            .iter()
            .any(|r| r.kind == ResourceKind::Font && r.font.as_ref() == Some(&face)),
        "{resources:?}"
    );
}

#[test]
fn faces_declared_later_reach_open_sessions() {
    let w = Workspace::new();
    let mut a = create(&w, "00000000000000000000000000000001", "1");
    text(&mut a, Some(vec!["Universal".into()]));
    lay_out(&mut a);
    assert!(
        codes(&a).iter().any(|c| c.starts_with("font.")),
        "the family is not declared yet"
    );
    let face = w.declare_font(&declaration(), ATKINSON).unwrap().data;
    assert_eq!(w.font_handles(&face), Some(1), "not taken until used");
    lay_out(&mut a);
    assert_eq!(w.font_handles(&face), Some(2));
    assert!(!codes(&a).iter().any(|c| c.starts_with("font.")));
}

#[test]
fn a_session_can_still_declare_its_own_face() {
    let w = Workspace::new();
    let mut a = create(&w, "00000000000000000000000000000001", "1");
    let b = create(&w, "00000000000000000000000000000002", "2");
    let face = a.declare_font(&declaration(), ATKINSON).unwrap().data;
    assert_eq!(w.font_handles(&face), None, "the workspace never saw it");
    text(&mut a, Some(vec!["Universal".into()]));
    lay_out(&mut a);
    assert!(!codes(&a).iter().any(|c| c.starts_with("font.")));
    assert!(codes(&b).is_empty());
}

#[test]
fn workspace_settings_are_the_default_until_a_session_chooses() {
    let w = Workspace::new();
    let mut settings = FontSettings::default();
    settings
        .generics
        .insert(GenericFamily::Serif, "Not Declared".into());
    w.set_font_settings(&Payload::new(settings)).unwrap();
    let mut a = create(&w, "00000000000000000000000000000001", "1");
    let mut b = create(&w, "00000000000000000000000000000002", "2");
    text(&mut a, None);
    text(&mut b, None);
    lay_out(&mut a);
    lay_out(&mut b);
    assert!(codes(&a).contains(&"font.fallback".to_string()));
    assert!(codes(&b).contains(&"font.fallback".to_string()));
    // b chooses its own: it stops following the workspace.
    b.set_font_settings(&Payload::new(FontSettings::default()))
        .unwrap();
    lay_out(&mut b);
    assert!(!codes(&b).contains(&"font.fallback".to_string()));
    let mut changed = FontSettings::default();
    changed
        .generics
        .insert(GenericFamily::Serif, "Another".into());
    w.set_font_settings(&Payload::new(changed)).unwrap();
    lay_out(&mut a);
    lay_out(&mut b);
    assert!(codes(&a).contains(&"font.fallback".to_string()));
    assert!(!codes(&b).contains(&"font.fallback".to_string()));
}

#[test]
fn workspace_font_input_is_bounded_and_atomic() {
    let w = Workspace::new();
    assert!(w.declare_font(&declaration(), b"not a font").is_err());
    let too_big = vec![0u8; 33 * 1024 * 1024];
    assert_eq!(
        w.declare_font(&declaration(), &too_big).unwrap_err().code(),
        "bindings.limit"
    );
    let bad = Payload {
        version: 7,
        data: declaration().data,
    };
    assert_eq!(
        w.declare_font(&bad, ATKINSON).unwrap_err().code(),
        "bindings.version"
    );
    let mut settings = FontSettings::default();
    settings.fallbacks = vec![String::new()];
    assert!(w.set_font_settings(&Payload::new(settings)).is_err());
    // Nothing was kept from the refusals.
    let face = w.declare_font(&declaration(), ATKINSON).unwrap().data;
    assert_eq!(w.font_handles(&face), Some(1));
}
