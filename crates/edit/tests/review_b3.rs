use reprise_doc::fragment::Fragment;
use reprise_doc::{BlockKind, Document, Domain, NodeId, SchemaRegistry};
use reprise_edit::{Editor, Reason};
use serde_json::{Value, json};
fn base() -> Fragment {
    let d = Document::new(1).unwrap();
    d.append_block(BlockKind::Paragraph, "", "body").unwrap();
    d.commit();
    d.copy_all_fragment("source", &SchemaRegistry::builtin())
        .unwrap()
}
fn dst(nonempty: bool) -> Editor {
    let d = Document::new(2).unwrap();
    if nonempty {
        d.append_block(BlockKind::Paragraph, "", "existing")
            .unwrap();
    }
    d.commit();
    Editor::new(d, SchemaRegistry::builtin())
}
fn story_blocks(f: &Fragment, n: usize) -> Vec<Value> {
    (0..n)
        .map(|i| {
            let mut b = serde_json::to_value(&f.blocks[0]).unwrap();
            b["id"] = json!(format!("{}@99", i + 1));
            b["text"] = json!("story");
            b
        })
        .collect()
}
#[test]
fn aggregate_body_limit_is_copy_limit_with_envelope() {
    let mut f = base();
    let b = f.blocks[0].clone();
    f.blocks = (0..4097)
        .map(|i| {
            let mut c = b.clone();
            c.id = NodeId::parse(&format!("{}@99", i + 1)).unwrap();
            c
        })
        .collect();
    f.sections = Some(json!({"v":1,"mode":"complete"}).to_string());
    let mut e = dst(false);
    let before = e.document().revision();
    let err = e.paste(&f, None, "dest").unwrap_err();
    assert_eq!(e.document().revision(), before);
    assert_eq!((e.undo_count(), e.redo_count()), (0, 0));
    assert!(
        matches!(err.reason, Reason::CopyLimit(_)),
        "wrong error: {:?}",
        err.reason
    );
}
#[test]
fn skipped_story_still_counts_toward_aggregate_limit() {
    let mut f = base();
    f.sections=Some(json!({"v":1,"mode":"complete","stories":[{"label":"s0","source":"1@8","blocks":story_blocks(&f,4097)}]}).to_string());
    let mut e = dst(true);
    let before = e.document().revision();
    let result = e.paste(&f, None, "dest");
    eprintln!(
        "accepted={}, undo={}, body_blocks={}",
        result.is_ok(),
        e.undo_count(),
        e.document().top(Domain::Body).len()
    );
    assert!(
        matches!(result,Err(ref err) if matches!(err.reason,Reason::CopyLimit(_))),
        "over-limit skipped story was accepted"
    );
    assert_eq!(e.document().revision(), before);
}
#[test]
fn story_parent_must_not_cross_into_body() {
    let mut f = base();
    let mut b = story_blocks(&f, 1).remove(0);
    b["parent"] = json!(f.blocks[0].id);
    f.sections = Some(
        json!({"v":1,"mode":"complete","stories":[{"label":"s0","source":"1@8","blocks":[b]}]})
            .to_string(),
    );
    let mut e = dst(false);
    let result = e.paste(&f, None, "dest");
    if let Ok(ref pasted) = result {
        let new = pasted.ids.nodes[&NodeId::parse("1@99").unwrap()];
        eprintln!(
            "transferred story block domain={:?}, stories={:?}",
            e.document().domain_of(new),
            e.document().stories()
        );
    }
    assert!(result.is_err(), "malformed cross-domain parent accepted");
}
#[test]
fn story_parent_must_not_cross_into_another_story() {
    let mut f = base();
    let a = story_blocks(&f, 2);
    let mut b = a[1].clone();
    b["parent"] = json!("1@99");
    f.sections=Some(json!({"v":1,"mode":"complete","stories":[{"label":"s0","source":"1@8","blocks":[a[0].clone()]},{"label":"s1","source":"2@8","blocks":[b]}]}).to_string());
    let mut e = dst(false);
    let result = e.paste(&f, None, "dest");
    if result.is_ok() {
        eprintln!(
            "story sizes: {:?}",
            e.document()
                .stories()
                .iter()
                .map(|s| e.document().order(Domain::Story(*s)).len())
                .collect::<Vec<_>>()
        );
    }
    assert!(result.is_err(), "malformed cross-story parent accepted");
}
#[test]
fn cyclic_story_block_references_are_refused() {
    let mut f = base();
    let mut a = story_blocks(&f, 2);
    a[0]["parent"] = json!("2@99");
    a[1]["parent"] = json!("1@99");
    f.sections = Some(
        json!({"v":1,"mode":"complete","stories":[{"label":"s0","source":"1@8","blocks":a}]})
            .to_string(),
    );
    let mut e = dst(false);
    assert!(e.paste(&f, None, "dest").is_err());
    assert_eq!(e.undo_count(), 0);
}

#[test]
fn over_story_limit_preserves_nonempty_undo_and_redo_and_history() {
    let mut e = dst(true);
    let node = e.document().top(Domain::Body)[0];
    for text in ["a", "b"] {
        e.apply(&reprise_edit::Transaction::from(vec![
            reprise_edit::Command::InsertText {
                node,
                at: 0,
                text: text.into(),
            },
        ]))
        .unwrap();
    }
    assert!(e.undo().unwrap());
    assert!(e.undo_count() > 0 && e.redo_count() > 0);
    let before = (
        e.document().export(reprise_doc::PersistenceMode::History),
        e.document().revision(),
        e.undo_count(),
        e.redo_count(),
    );
    let mut f = base();
    f.sections=Some(json!({"v":1,"mode":"complete","stories":(0..16385).map(|i|json!({"label":format!("s{i}"),"source":format!("{}@99",i+1),"blocks":[]})).collect::<Vec<_>>()} ).to_string());
    let err = e.paste(&f, None, "dest").unwrap_err();
    assert_eq!(err.reason, Reason::CopyLimit("transferred stories"));
    let after = (
        e.document().export(reprise_doc::PersistenceMode::History),
        e.document().revision(),
        e.undo_count(),
        e.redo_count(),
    );
    assert_eq!(before, after);
}
#[test]
fn deep_and_huge_small_envelopes_refuse_without_writes() {
    let mut e = dst(true);
    for raw in [
        format!(
            "{{\"v\":1,\"mode\":\"complete\",\"context\":{{\"props\":{{\"x\":{}{}}}}}}}",
            "[".repeat(1024),
            "]".repeat(1024)
        ),
        "{\"v\":1,\"mode\":\"complete\",\"dropped\":18446744073709551616}".into(),
    ] {
        let mut f = base();
        f.sections = Some(raw);
        let before = (e.document().revision(), e.undo_count(), e.redo_count());
        assert!(e.paste(&f, None, "dest").is_err());
        assert_eq!(
            before,
            (e.document().revision(), e.undo_count(), e.redo_count())
        );
    }
}
#[test]
fn deep_metadata_preflight_is_bounded() {
    for fixture in [
        reprise_fixtures::hostile::metadata_depth_1024().unwrap(),
        reprise_fixtures::hostile::metadata_depth_4096().unwrap(),
    ] {
        let err = fixture.doc.check_edit_preflight().unwrap_err();
        assert!(
            matches!(err, reprise_doc::DocError::EditPreflightDepth(64)),
            "{err:?}"
        );
    }
}
