//! Lists and inline objects through the facade (`docs/inline.md`,
//! `docs/roles-and-lists.md`): a list item's `list` and `inline` listings sit
//! on the same block, a save and a copy carry both, and the package declares
//! both required features.
use reprise::*;

const DOC: &str = "00112233445566778899aabbccddeeff";
const COPY: &str = "ffeeddccbbaa99887766554433221100";
const ASSET: &str = "ab5f7c0d3e1a9b2c4d6e8f0a1b2c3d4e5f60718293a4b5c6d7e8f9a0b1c2d3e4";

fn session() -> DocumentSession {
    Workspace::new()
        .create(&Payload::new(Create {
            document_id: DOC.into(),
            peer_id: "1".into(),
        }))
        .unwrap()
}
fn apply(s: &mut DocumentSession, commands: Vec<Command>) -> Applied {
    s.apply(&Payload::new(Transaction { commands }))
        .unwrap()
        .data
}
fn block(text: &str, index: u32) -> Command {
    Command::InsertBlock {
        parent: None,
        index,
        block_kind: BlockKind::Paragraph,
        text: text.into(),
        style: Style::default(),
    }
}
fn field(function: &str) -> InlineObject {
    InlineObject {
        kind: "field".into(),
        params: [("field".to_string(), function.to_string())].into(),
        ..InlineObject::default()
    }
}
fn image(alt: &str) -> InlineObject {
    InlineObject {
        kind: "image".into(),
        params: [
            ("asset".to_string(), ASSET.to_string()),
            ("alt".to_string(), alt.to_string()),
            ("width".to_string(), "pt:20480".to_string()),
        ]
        .into(),
        ..InlineObject::default()
    }
}
fn insert(node: &str, at: u32, object: InlineObject) -> Command {
    Command::InsertInline {
        node: node.into(),
        at,
        object,
    }
}
fn state(s: &DocumentSession) -> State {
    s.state().unwrap().data
}

/// Two numbered items with objects and a body paragraph with one.
fn document() -> DocumentSession {
    let mut s = session();
    let ids = apply(
        &mut s,
        vec![
            block("see  here", 0),
            block("page  of", 1),
            block("plain  end", 2),
        ],
    )
    .blocks;
    apply(
        &mut s,
        vec![
            insert(&ids[0], 4, image("a cat")),
            insert(&ids[1], 5, field("page-number")),
            insert(&ids[2], 6, image("body picture")),
        ],
    );
    let mut commands = vec![Command::CreateList {
        definition: ListDefinition {
            base: None,
            levels: vec![ListLevel {
                level: 0,
                format: Some("decimal".into()),
                start: Some("1".into()),
                ..Default::default()
            }],
        },
    }];
    commands.extend(ids[..2].iter().map(|node| Command::SetListItem {
        node: node.clone(),
        list: ListRef::New { index: 0 },
        level: 0,
    }));
    apply(&mut s, commands);
    s
}

#[test]
fn an_item_lists_its_marker_and_its_objects_on_one_block() {
    let s = document();
    let blocks = state(&s).blocks;
    let markers: Vec<_> = blocks
        .iter()
        .map(|b| b.list.as_ref().map(|i| i.marker.as_str()))
        .collect();
    assert_eq!(markers, [Some("1."), Some("2."), None]);
    let kinds: Vec<Vec<&str>> = blocks
        .iter()
        .map(|b| b.inline.iter().map(|e| e.object.kind.as_str()).collect())
        .collect();
    assert_eq!(kinds, [["image"], ["field"], ["image"]]);
    // The objects stay at their characters while an item is added above.
    let before: Vec<u32> = blocks.iter().map(|b| b.inline[0].at).collect();
    assert_eq!(before, [4, 5, 6]);
}

#[test]
fn a_save_and_a_copy_keep_the_list_and_the_objects_with_new_identities() {
    let mut s = document();
    let source = state(&s).blocks;
    let bytes = s.save().unwrap().data.bytes;

    // The package declares both features: lists (bit 14), objects (bit 15).
    let container =
        reprise_format::Container::decode(&bytes, reprise_format::Limits::default()).unwrap();
    let required = container.0.header.features.required;
    assert_ne!(
        required & reprise_format::features::REQUIRED_BLOCK_SEMANTICS,
        0
    );
    assert_ne!(
        required & reprise_format::features::REQUIRED_INLINE_OBJECTS,
        0
    );
    assert_eq!(reprise_format::features::REQUIRED_BLOCK_SEMANTICS, 1 << 14);
    assert_eq!(reprise_format::features::REQUIRED_INLINE_OBJECTS, 1 << 15);

    let copy = Workspace::new()
        .import_copy(
            &Payload::new(ImportCopy {
                document_id: COPY.into(),
                peer_id: "2".into(),
            }),
            &bytes,
        )
        .unwrap();
    let copied = state(&copy).blocks;
    assert_eq!(copied.len(), source.len());
    for (a, b) in source.iter().zip(&copied) {
        assert_eq!(a.text, b.text);
        assert_eq!(
            a.list.as_ref().map(|i| (i.level, &i.marker)),
            b.list.as_ref().map(|i| (i.level, &i.marker)),
            "the list is carried"
        );
        assert_eq!(a.inline.len(), b.inline.len());
        for (x, y) in a.inline.iter().zip(&b.inline) {
            assert_eq!((x.at, &x.object), (y.at, &y.object));
            assert_ne!(x.id, y.id, "an object in a copy is new");
        }
    }
}

#[test]
fn undo_takes_back_the_list_and_keeps_the_objects() {
    let mut s = document();
    assert!(s.undo().unwrap().data, "the list is one step");
    let blocks = state(&s).blocks;
    assert!(blocks.iter().all(|b| b.list.is_none()));
    assert_eq!(
        blocks.iter().map(|b| b.inline.len()).collect::<Vec<_>>(),
        [1, 1, 1]
    );
}
