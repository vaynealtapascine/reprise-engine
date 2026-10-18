//! Root metadata queries must not enter Loro's recursive tree value builder.
//!
//! A valid document can be 1,024 or 4,096 levels deep. Numbering, feature
//! detection, `State`, save, package open and sync export must not recurse once
//! per level: this runs on the test harness's default stack, and the same bytes
//! run in the 1 MiB WASM smoke build (`REPRISE_DEEP_FIXTURES`).
use reprise::*;
use reprise_doc::{BlockKind, Document, NewBlock, PersistenceMode};

/// `(depth, hidden)`. A live chain past 1,024 levels is refused by `State`,
/// and every read of one walks its ancestry per node, so its 4,096-level form
/// only goes through the codec (`a_4096_deep_live_chain_survives_the_codec`).
/// The WASM smoke test opens the hidden forms.
const CASES: [(usize, bool); 3] = [(1024, false), (1024, true), (4096, true)];

/// A chain of `depth` nested paragraphs. With `hidden`, its root is deleted
/// and a live sibling follows: the physical tree stays deep, the live one not.
fn chain(depth: usize, hidden: bool) -> Document {
    let doc = Document::new(1).unwrap();
    let new = |text: &str| NewBlock {
        kind: BlockKind::Paragraph,
        style: String::new(),
        text: text.into(),
        overrides: Default::default(),
    };
    // Built from the leaf up: each new top-level block takes the chain below it,
    // so no step walks the chain's ancestry and building stays linear.
    let mut root = doc.insert_block_at(None, 0, &new("")).unwrap();
    for _ in 1..depth {
        let above = doc.insert_block_at(None, 0, &new("")).unwrap();
        doc.move_block(root, Some(above), 0).unwrap();
        root = above;
    }
    if hidden {
        doc.delete_block(root).unwrap();
        doc.append_block(BlockKind::Paragraph, "", "live").unwrap();
    }
    doc.commit();
    doc
}

fn metadata(doc: &Document) {
    assert!(doc.list_numbering().items.is_empty());
    assert!(!doc.has_block_semantics());
    assert!(!doc.has_marks());
    assert!(!doc.has_page_setup());
    assert!(!doc.has_annotations());
    assert!(!doc.has_text_formatting());
    assert!(!doc.has_text_emphasis());
    assert!(!doc.has_paragraph_styles());
    assert_eq!(
        reprise_format::features::table_features(doc),
        reprise_format::FeatureFlags::default()
    );
    // Guarding a deep edit must refuse before Loro's recursive codec.
    assert!(matches!(
        doc.check_edit_preflight(),
        Err(reprise_doc::DocError::EditPreflightDepth(64))
    ));
}

#[test]
fn deep_metadata_state_and_export_on_default_stack() {
    for (depth, hidden) in CASES {
        // A stack overflow aborts without a message: say how far each case got.
        let stage = |what: &str| eprintln!("deep {depth} hidden={hidden}: {what}");
        let doc = chain(depth, hidden);
        stage("built");
        metadata(&doc);
        stage("metadata");
        let bytes = reprise_format::Package::new(
            &doc,
            reprise_format::DocumentId([1; 16]),
            PersistenceMode::History,
        )
        .unwrap()
        .save()
        .unwrap();
        let mut session = Workspace::new()
            .open(
                &Payload::new(Open {
                    peer_id: "2".into(),
                }),
                &bytes,
            )
            .unwrap();
        stage("opened");
        assert!(!session.state().unwrap().data.blocks.is_empty());
        // Saving finishes a layout, which also walks a live chain's ancestry
        // per node: a hidden chain keeps that short.
        if hidden {
            assert!(!session.save().unwrap().data.bytes.is_empty());
        }
        assert!(
            !session
                .export_updates()
                .unwrap()
                .data
                .content
                .bytes
                .is_empty()
        );
        stage("state, save and export");
        if hidden && let Some(folder) = std::env::var_os("REPRISE_DEEP_FIXTURES") {
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(
                std::path::Path::new(&folder).join(format!("deep-{depth}.reprise")),
                &bytes,
            )
            .unwrap();
        }
    }
}

/// Loro's snapshot codec (export, validation and import) walks the whole tree;
/// it must complete on the default stack even past the State depth limit.
/// Shallow snapshots still build the nested tree value and are not covered.
#[test]
fn a_4096_deep_live_chain_survives_the_codec() {
    let doc = chain(4096, false);
    let bytes = doc.try_export(PersistenceMode::History).unwrap();
    let again = Document::import(&bytes, 2).unwrap();
    assert_eq!(again.revision(), doc.revision());
}
