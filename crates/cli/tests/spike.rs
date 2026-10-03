//! End-to-end checks for the spike: anchored text, reflow, a following
//! annotation and headless output.

use reprise_cli::{EDIT, engine, spike_document};
use reprise_layout::{DisplayOptions, RelationStatus};

fn note_line(snapshot: &reprise_layout::LayoutSnapshot, note: reprise_doc::NodeId) -> (usize, i32) {
    let rel = snapshot
        .relations
        .iter()
        .find(|r| r.source == note)
        .expect("relation laid out");
    assert_eq!(rel.status, RelationStatus::Valid);
    let (_, line) = rel.target_line.expect("target line found");
    let y = snapshot.block(note).expect("note placed").frame.origin.y;
    (line, y.0)
}

#[test]
fn annotation_follows_its_line_through_a_reflow() {
    let engine = engine();
    let spike = spike_document().unwrap();
    let before = engine.layout(&spike.doc);
    spike.edit().unwrap();
    let after = engine.layout(&spike.doc);

    let (line_before, y_before) = note_line(&before, spike.hallway_note);
    let (line_after, y_after) = note_line(&after, spike.hallway_note);
    assert!(
        line_after > line_before,
        "the edit pushes the range onto a later line"
    );
    assert!(y_after > y_before, "the note moves down with it");

    // The note sits level with the top of the line it targets.
    let hallway = after.block(spike.hallway).unwrap();
    assert_eq!(hallway.lines[line_after].rect.origin.y.0, y_after);
    assert!(hallway.lines[line_after].preview.contains("a corridor"));
    assert!(after.diagnostics.is_empty(), "{:?}", after.diagnostics);

    insta::assert_snapshot!("before", before.to_json());
    insta::assert_snapshot!("after", after.to_json());
}

#[test]
fn layout_is_deterministic() {
    let engine = engine();
    let spike = spike_document().unwrap();
    let a = engine
        .layout(&spike.doc)
        .to_display_list(DisplayOptions { debug: true });
    let b = engine
        .layout(&spike_document().unwrap().doc)
        .to_display_list(DisplayOptions { debug: true });
    assert_eq!(a.to_json(), b.to_json());
    insta::assert_snapshot!("display", a.content_only().to_json());
}

#[test]
fn deleting_the_target_reports_and_skips_the_note() {
    let engine = engine();
    let spike = spike_document().unwrap();
    let text = spike.doc.block(spike.hallway).unwrap().text;
    let at = text.to_string().find("a corridor").unwrap();
    text.delete(at..at + "a corridor".len()).unwrap();
    let snapshot = engine.layout(&spike.doc);

    let rel = snapshot
        .relations
        .iter()
        .find(|r| r.source == spike.hallway_note)
        .unwrap();
    assert_eq!(rel.status, RelationStatus::Missing);
    assert!(snapshot.block(spike.hallway_note).is_none());
    assert!(
        snapshot
            .diagnostics
            .iter()
            .any(|d| d.message.contains("target range"))
    );
    // Everything else still lays out (37).
    assert!(snapshot.block(spike.opening).is_some());
}

#[test]
fn concurrent_edits_converge_and_lay_out_identically() {
    let engine = engine();
    let spike = spike_document().unwrap();
    let other = spike.doc.fork(2).unwrap();

    // Peer 1 types the spike edit; peer 2 inserts inside the target range's paragraph.
    spike.edit().unwrap();
    let theirs = other.block(spike.hallway).unwrap().text;
    let at = theirs.to_string().find("further").unwrap();
    theirs.insert(at, "much ").unwrap();

    spike.doc.merge(&other).unwrap();
    other.merge(&spike.doc).unwrap();

    let a = engine.layout(&spike.doc);
    let b = engine.layout(&other);
    assert_eq!(a.revision, b.revision);
    assert_eq!(a.to_json(), b.to_json());
    let hallway = spike.doc.block(spike.hallway).unwrap().text.to_string();
    assert!(hallway.starts_with(EDIT) && hallway.contains("much further"));
    note_line(&a, spike.hallway_note);
}

#[test]
fn every_backend_renders() {
    let engine = engine();
    let snapshot = engine.layout(&spike_document().unwrap().doc);
    let dir = std::env::temp_dir().join(format!("reprise-spike-{}", std::process::id()));
    reprise_cli::write_outputs(&engine, &snapshot, &dir, "spike").unwrap();
    let svg = std::fs::read_to_string(dir.join("spike.svg")).unwrap();
    assert!(svg.starts_with("<svg") && svg.contains("<use"));
    assert!(
        std::fs::read(dir.join("spike.png"))
            .unwrap()
            .starts_with(b"\x89PNG")
    );
    assert!(
        std::fs::read(dir.join("spike.pdf"))
            .unwrap()
            .starts_with(b"%PDF-")
    );
    let _ = std::fs::remove_dir_all(dir);
}
