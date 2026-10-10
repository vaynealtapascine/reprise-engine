use reprise::*;

fn session(peer: &str) -> DocumentSession {
    Workspace::new()
        .create(&Payload::new(Create {
            document_id: "00112233445566778899aabbccddeeff".into(),
            peer_id: peer.into(),
        }))
        .unwrap()
}
fn apply(s: &mut DocumentSession, commands: Vec<Command>) {
    s.apply(&Payload::new(Transaction { commands })).unwrap();
}
fn sync(a: &mut DocumentSession, b: &mut DocumentSession) {
    let from_a = a
        .sync_export(&Payload::new(SyncRequest { since: None }))
        .unwrap();
    let from_b = b
        .sync_export(&Payload::new(SyncRequest { since: None }))
        .unwrap();
    a.sync_import(&from_b).unwrap();
    b.sync_import(&from_a).unwrap();
    assert_eq!(
        a.state().unwrap().data.page_setup,
        b.state().unwrap().data.page_setup
    );
}
fn patch(s: &mut DocumentSession, setup: PageSetupPatch) {
    s.set_page_setup(&Payload::new(setup)).unwrap();
}
fn letter() -> PageSetupPatch {
    PageSetupPatch {
        width: Some(612 * 1024),
        height: Some(792 * 1024),
        top: Some(72 * 1024),
        right: Some(72 * 1024),
        bottom: Some(72 * 1024),
        left: Some(72 * 1024),
    }
}

#[test]
fn state_commands_save_reopen_and_each_undo_step() {
    let mut s = session("1");
    let original = s.state().unwrap().data.page_setup;
    assert_eq!((original.width, original.height), (420 * 1024, 300 * 1024));
    assert_eq!(original.margins.right, 164 * 1024);
    assert!(!original.patched);
    patch(&mut s, PageSetupPatch::default());
    assert!(!s.state().unwrap().data.can_undo);
    patch(&mut s, letter());
    let portrait = s.state().unwrap().data.page_setup;
    assert_eq!(portrait.orientation, PageOrientation::Portrait);
    assert!(portrait.patched);
    apply(&mut s, vec![Command::SwapPageOrientation]);
    let landscape = s.state().unwrap().data.page_setup;
    assert_eq!(
        (landscape.width, landscape.height),
        (portrait.height, portrait.width)
    );
    assert_eq!(landscape.margins, portrait.margins);
    assert_eq!(landscape.orientation, PageOrientation::Landscape);
    assert!(s.undo().unwrap().data);
    assert_eq!(s.state().unwrap().data.page_setup, portrait);
    assert!(s.undo().unwrap().data);
    assert_eq!(s.state().unwrap().data.page_setup, original);
    assert!(s.redo().unwrap().data);
    assert_eq!(s.state().unwrap().data.page_setup, portrait);
    assert!(s.redo().unwrap().data);
    let bytes = s.save().unwrap().data.bytes;
    let opened = Workspace::new()
        .open(
            &Payload::new(Open {
                peer_id: "2".into(),
            }),
            &bytes,
        )
        .unwrap();
    assert_eq!(opened.state().unwrap().data.page_setup, landscape);
}

#[test]
fn independent_first_size_and_margin_edits_merge_on_two_peers() {
    let mut a = session("1");
    let mut b = session("2");
    sync(&mut a, &mut b);
    patch(
        &mut a,
        PageSetupPatch {
            width: Some(612 * 1024),
            height: Some(792 * 1024),
            ..Default::default()
        },
    );
    patch(
        &mut b,
        PageSetupPatch {
            top: Some(72 * 1024),
            right: Some(72 * 1024),
            bottom: Some(72 * 1024),
            left: Some(72 * 1024),
            ..Default::default()
        },
    );
    sync(&mut a, &mut b);
    let merged = a.state().unwrap().data.page_setup;
    assert_eq!((merged.width, merged.height), (612 * 1024, 792 * 1024));
    assert_eq!(
        merged.margins,
        PageMargins {
            top: 72 * 1024,
            right: 72 * 1024,
            bottom: 72 * 1024,
            left: 72 * 1024
        }
    );
    assert!(a.undo().unwrap().data);
    let undone = a.state().unwrap().data.page_setup;
    assert_eq!((undone.width, undone.height), (420 * 1024, 300 * 1024));
    assert_eq!(undone.margins, merged.margins);
    sync(&mut a, &mut b);
    assert!(a.redo().unwrap().data);
    sync(&mut a, &mut b);
    assert_eq!(a.state().unwrap().data.page_setup, merged);
}

#[test]
fn independent_first_width_height_and_each_margin_merge() {
    let mut a = session("1");
    let mut b = session("2");
    sync(&mut a, &mut b);
    patch(
        &mut a,
        PageSetupPatch {
            width: Some(700 * 1024),
            top: Some(10 * 1024),
            left: Some(20 * 1024),
            ..Default::default()
        },
    );
    patch(
        &mut b,
        PageSetupPatch {
            height: Some(900 * 1024),
            bottom: Some(30 * 1024),
            right: Some(40 * 1024),
            ..Default::default()
        },
    );
    sync(&mut a, &mut b);
    let merged = a.state().unwrap().data.page_setup;
    assert_eq!((merged.width, merged.height), (700 * 1024, 900 * 1024));
    assert_eq!(
        merged.margins,
        PageMargins {
            top: 10 * 1024,
            left: 20 * 1024,
            bottom: 30 * 1024,
            right: 40 * 1024
        }
    );
}

#[test]
fn hostile_requests_refuse_atomically_and_leave_no_feature_history() {
    let mut s = session("1");
    let initial = s.state().unwrap();
    for value in [i32::MIN, -1, 0, 14_400 * 1024 + 1, i32::MAX] {
        for setup in [
            PageSetupPatch {
                width: Some(value),
                ..Default::default()
            },
            PageSetupPatch {
                height: Some(value),
                ..Default::default()
            },
        ] {
            let error = s.set_page_setup(&Payload::new(setup)).unwrap_err();
            assert_eq!(error.code(), "edit.page-setup-invalid");
            assert_eq!(error.payload().data.severity, Severity::Error);
            assert_eq!(s.state().unwrap(), initial);
        }
    }
    for value in [i32::MIN, -1, 420 * 1024, i32::MAX] {
        for setup in [
            PageSetupPatch {
                left: Some(value),
                ..Default::default()
            },
            PageSetupPatch {
                right: Some(value),
                ..Default::default()
            },
            PageSetupPatch {
                top: Some(value),
                ..Default::default()
            },
            PageSetupPatch {
                bottom: Some(value),
                ..Default::default()
            },
        ] {
            assert_eq!(
                s.set_page_setup(&Payload::new(setup)).unwrap_err().code(),
                "edit.page-setup-invalid"
            );
            assert_eq!(s.state().unwrap(), initial);
        }
    }
    let error = s
        .apply(&Payload::new(Transaction {
            commands: vec![
                Command::SetPageSetup { setup: letter() },
                Command::SetPageSetup {
                    setup: PageSetupPatch {
                        left: Some(612 * 1024),
                        ..Default::default()
                    },
                },
            ],
        }))
        .unwrap_err();
    assert_eq!(error.payload().data.command, Some(1));
    assert_eq!(s.state().unwrap(), initial);
    patch(
        &mut s,
        PageSetupPatch {
            width: Some(1),
            height: Some(1),
            top: Some(0),
            right: Some(0),
            bottom: Some(0),
            left: Some(0),
        },
    );
    assert_eq!(
        s.state().unwrap().data.page_setup.orientation,
        PageOrientation::Square
    );
}

#[test]
fn conflicting_merged_margins_warn_then_can_be_repaired() {
    let mut a = session("1");
    let mut b = session("2");
    sync(&mut a, &mut b);
    patch(
        &mut a,
        PageSetupPatch {
            left: Some(200 * 1024),
            ..Default::default()
        },
    );
    patch(
        &mut b,
        PageSetupPatch {
            right: Some(250 * 1024),
            ..Default::default()
        },
    );
    sync(&mut a, &mut b);
    let state = a.state().unwrap().data;
    assert!(!state.page_setup.patched);
    assert!(
        state
            .diagnostics
            .iter()
            .any(|d| d.code == "layout.page-setup-invalid" && d.severity == Severity::Warning)
    );
    patch(
        &mut a,
        PageSetupPatch {
            width: Some(700 * 1024),
            ..Default::default()
        },
    );
    sync(&mut a, &mut b);
    let repaired = a.state().unwrap().data.page_setup;
    assert_eq!(repaired.margins.left, 200 * 1024);
    assert_eq!(repaired.margins.right, 250 * 1024);
    assert!(repaired.patched);
}
