//! Section, break, story and region commands join the isolated preflight: a
//! batch containing any of them is exercised on a replica of the document
//! before anything is published, so a refusal - here, pending raw writes that
//! staging would otherwise commit - changes no bytes, revision or undo state.
use reprise_doc::sections::{BreakKind, SectionId};
use reprise_doc::{BlockKind, Document, NewBlock, NodeId};
use reprise_edit::{BreakAt, Command, Editor, Reason, RegionBinding, StoryFrom, Transaction};

fn story_block(t: &str) -> NewBlock {
    NewBlock {
        kind: BlockKind::Paragraph,
        style: "body".into(),
        text: t.into(),
        overrides: Default::default(),
    }
}

/// An editor over a committed document with two paragraphs, a story and a
/// break, and one more raw edit that has not been committed.
fn with_pending() -> (Editor, NodeId, NodeId, NodeId, NodeId) {
    let doc = Document::new(1).unwrap();
    let a = doc
        .append_block(BlockKind::Paragraph, "body", "AB")
        .unwrap();
    let b = doc
        .append_block(BlockKind::Paragraph, "body", "CD")
        .unwrap();
    let story = doc.create_story(&[story_block("head")]).unwrap();
    let broken = doc
        .append_block(BlockKind::Paragraph, "body", "EF")
        .unwrap();
    let life = doc.mint_lifetime(broken).unwrap();
    doc.activate_start(broken, &life, BreakKind::Section, &Default::default())
        .unwrap();
    doc.commit();
    let e = Editor::new(doc, reprise_fixtures::engine().schemas.clone());
    // A native caller's uncommitted write: a composite edit must not publish it.
    e.document().block(a).unwrap().text.insert(2, "!").unwrap();
    (e, a, b, story, broken)
}

/// Revision and both undo depths. (Exporting history would commit the pending
/// raw write it is meant to watch.)
fn frozen(e: &Editor) -> (reprise_doc::Revision, usize, usize) {
    (e.document().revision(), e.undo_count(), e.redo_count())
}

#[test]
fn every_section_break_story_and_region_command_is_preflighted() {
    let commands = |a: NodeId, b: NodeId, story: NodeId, broken: NodeId| {
        vec![
            Command::InsertBreak {
                at: BreakAt::Before(b),
                kind: BreakKind::Page,
                patch: Default::default(),
            },
            Command::RemoveBreak { block: broken },
            Command::SetSection {
                section: SectionId::First,
                patch: [("columns.count".into(), serde_json::json!(2))].into(),
                clear: Vec::new(),
            },
            Command::CreateStory {
                blocks: vec![story_block("new")],
            },
            Command::SetRegion {
                section: SectionId::First,
                slot: "header".into(),
                variant: "default".into(),
                binding: RegionBinding::Story(story),
            },
            Command::CreateRegion {
                section: SectionId::First,
                slot: "footer".into(),
                variant: "default".into(),
                from: StoryFrom::Empty,
            },
            Command::CreateRegion {
                section: SectionId::First,
                slot: "background".into(),
                variant: "default".into(),
                from: StoryFrom::CopyOf(story),
            },
            Command::DeleteStory { story },
        ]
        .into_iter()
        .map(move |c| (c, a))
    };
    let (probe, a, b, story, broken) = with_pending();
    let names: Vec<String> = commands(a, b, story, broken)
        .map(|(c, _)| {
            format!("{c:?}")
                .split([' ', '{'])
                .next()
                .unwrap()
                .to_owned()
        })
        .collect();
    drop(probe);
    for (index, name) in names.iter().enumerate() {
        // Alone, and after an ordinary edit in the same batch.
        for lead in [false, true] {
            let (mut e, a, b, story, broken) = with_pending();
            let before = frozen(&e);
            let command = commands(a, b, story, broken).nth(index).unwrap().0;
            let mut batch = Vec::new();
            if lead {
                batch.push(Command::DeleteText {
                    node: a,
                    range: 0..1,
                });
            }
            batch.push(command);
            let error = match e.apply(&Transaction::from(batch)) {
                Err(error) => error,
                Ok(applied) => panic!("{name} (lead {lead}) was applied: {applied:?}"),
            };
            assert_eq!(
                error.reason,
                Reason::UncommittedEdit,
                "{name} (lead {lead}) was not preflighted"
            );
            assert_eq!(before, frozen(&e), "{name} (lead {lead}) published");
            assert!(
                e.document().check_edit_preflight().is_err(),
                "{name}: the raw write was committed"
            );
            assert_eq!(e.document().block(a).unwrap().text.to_string(), "AB!");
        }
    }
    // Committed, the same batches apply: the preflight refuses nothing valid.
    let (mut e, a, b, story, broken) = with_pending();
    e.document().commit();
    for (command, _) in commands(a, b, story, broken) {
        let name = format!("{command:?}");
        e.apply(&Transaction::from(vec![
            Command::InsertText {
                node: a,
                at: 0,
                text: "x".into(),
            },
            command,
        ]))
        .unwrap_or_else(|err| panic!("{name}: {err:?}"));
    }
}
