//! Root metadata queries must not enter Loro's recursive tree value builder.
use reprise::*;
use reprise_doc::{Document, NewBlock, PersistenceMode};

#[test]
fn deep_metadata_state_and_export_on_default_stack() {
    for depth in [1024, 4096] {
        let doc = Document::new(1).unwrap();
        let mut parent = None;
        for _ in 0..depth {
            parent = Some(
                doc.insert_block_at(
                    parent,
                    0,
                    &NewBlock {
                        kind: reprise_doc::BlockKind::Paragraph,
                        style: String::new(),
                        text: String::new(),
                        overrides: Default::default(),
                    },
                )
                .unwrap(),
            );
        }
        doc.commit();
        assert!(doc.list_numbering().items.is_empty());
        assert!(!doc.has_block_semantics());
        assert!(!doc.has_marks());
        assert!(!doc.has_page_setup());
        assert!(!doc.has_annotations());
        assert!(!doc.has_text_formatting());
        assert!(!doc.has_text_emphasis());
        assert!(!doc.has_paragraph_styles());
        assert_eq!(
            reprise_format::features::table_features(&doc),
            reprise_format::FeatureFlags::default()
        );
        // Guarding a deep edit must refuse before Loro's recursive codec.
        assert!(matches!(
            doc.check_edit_preflight(),
            Err(reprise_doc::DocError::EditPreflightDepth(64))
        ));
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
        assert!(!session.state().unwrap().data.blocks.is_empty());
        assert!(!session.save().unwrap().data.bytes.is_empty());
        assert!(!session.export_updates().unwrap().data.bytes.is_empty());
        if let Some(folder) = std::env::var_os("REPRISE_DEEP_FIXTURES") {
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(
                std::path::Path::new(&folder).join(format!("deep-{depth}.reprise")),
                &bytes,
            )
            .unwrap();
        }
    }
}
