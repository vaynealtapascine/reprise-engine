use reprise::*;
fn session() -> DocumentSession {
    Workspace::new()
        .create(&Payload::new(Create {
            document_id: "00112233445566778899aabbccddeeff".into(),
            peer_id: "1".into(),
        }))
        .unwrap()
}
fn paragraph(s: &mut DocumentSession, text: &str) -> String {
    s.apply(&Payload::new(Transaction {
        commands: vec![Command::InsertBlock {
            parent: None,
            index: 0,
            block_kind: BlockKind::Paragraph,
            text: text.into(),
            style: Style::default(),
        }],
    }))
    .unwrap()
    .data
    .blocks[0]
        .clone()
}
fn finish(s: &mut DocumentSession) {
    let mut job = s
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    for _ in 0..100 {
        if job.step(s, 8).unwrap().data.complete {
            return;
        }
    }
    panic!("explicit test job bound exceeded")
}
fn plugin(s: &DocumentSession) -> Plugin {
    let bytes = include_bytes!("../../plugin/test-plugins/demo.wasm");
    let hash = reprise_format::content_hash(bytes);
    s.load_plugin(
        &Payload::new(PluginSpec {
            name: "demo".into(),
            plugin_version: "1".into(),
            sha256: hash,
            phase: PluginPhase::Layout,
            imports: vec![],
            grants: vec![],
            functions: vec![PluginFunction {
                name: "demo-double".into(),
                operation: 1,
                params: vec![Dimension::Length],
                variadic: None,
                returns: Dimension::Length,
            }],
            fuel: 100_000,
            memory_pages: 16,
            table_elements: 1024,
            buffer_bytes: 65_536,
        }),
        bytes,
    )
    .unwrap()
}
#[test]
fn plugins_install_functions_geometry_and_schemas_without_losing_undo() {
    let mut s = session();
    let node = paragraph(&mut s, "plugin text");
    let p = plugin(&s);
    s.install_plugin(&p, &Payload::new(PluginInstall::Functions))
        .unwrap();
    s.apply(&Payload::new(Transaction {
        commands: vec![Command::SetStyle {
            node: node.clone(),
            style: Style {
                size: Some("demo-double(6pt)".into()),
                ..Style::default()
            },
        }],
    }))
    .unwrap();
    s.install_plugin(&p, &Payload::new(PluginInstall::Geometry { operation: 2 }))
        .unwrap();
    let schema = RelationSchema {
        id: "demo.edge".into(),
        version: 1,
        ownership: Ownership::Independent,
        roles: vec![RoleSpec {
            name: "target".into(),
            accepts: vec![TargetClass::Node],
            min: 1,
            max: Some(1),
        }],
        params: vec![],
        on_target_deleted: OnTargetDeleted::KeepMissing,
        on_copy: CopyPolicy {
            inside: CopyInside::Duplicate,
            crossing: CopyCrossing::Drop,
        },
    };
    s.install_plugin(
        &p,
        &Payload::new(PluginInstall::Relation {
            schema,
            operation: 3,
        }),
    )
    .unwrap();
    let mut targets = std::collections::BTreeMap::new();
    targets.insert("target".into(), vec![Target::Node(node)]);
    let result = s
        .apply(&Payload::new(Transaction {
            commands: vec![Command::AddRelation {
                relation: Relation {
                    schema: "demo.edge".into(),
                    owner: None,
                    targets,
                    params: std::collections::BTreeMap::new(),
                },
            }],
        }))
        .unwrap();
    assert_eq!(result.data.relations.len(), 1);
    finish(&mut s);
    assert!(
        !s.diagnostics()
            .data
            .iter()
            .any(|d| d.code == "relation.not-applied" || d.code == "style.function-failed")
    );
    assert!(s.undo().unwrap().data);
    assert!(s.undo().unwrap().data);
    assert!(s.undo().unwrap().data);
    assert!(s.state().unwrap().data.blocks.is_empty());
    assert!(s.redo().unwrap().data);
}
#[test]
fn opaque_assets_and_unknown_package_sections_survive_edit_and_save() {
    let mut s = session();
    let node = paragraph(&mut s, "original");
    let raw = [0, 255, 1, 2];
    s.register_asset(
        &Payload::new(AssetDeclaration {
            id: "image".into(),
            kind: AssetKind::Image,
        }),
        &raw,
    )
    .unwrap();
    assert_eq!(
        s.resource_bytes(&Payload::new("image".into()))
            .unwrap()
            .data
            .bytes,
        raw
    );
    let bytes = s.save().unwrap().data.bytes;
    let opened = reprise_format::Package::open(
        &bytes,
        1,
        reprise_format::Limits::default(),
        &reprise_format::MigrationRegistry::builtin(),
    )
    .unwrap();
    let mut package = opened.package().clone();
    package
        .set_unknown_section(
            42,
            reprise_format::Section {
                flags: 8,
                codec: 77,
                bytes: vec![0, 255, 12],
            },
        )
        .unwrap();
    package.set_extensions(vec![9, 0, 255]).unwrap();
    let bytes = package.save().unwrap();
    let mut opened = Workspace::new()
        .open(
            &Payload::new(Open {
                peer_id: "2".into(),
            }),
            &bytes,
        )
        .unwrap();
    opened
        .apply(&Payload::new(Transaction {
            commands: vec![Command::InsertText {
                node,
                at: 0,
                text: "changed ".into(),
            }],
        }))
        .unwrap();
    let bytes = opened.save().unwrap().data.bytes;
    let inspect = reprise_format::Package::open(
        &bytes,
        3,
        reprise_format::Limits::default(),
        &reprise_format::MigrationRegistry::builtin(),
    )
    .unwrap();
    let section = inspect.package().container().sections.get(&42).unwrap();
    assert_eq!(
        (section.flags, section.codec, section.bytes.as_slice()),
        (8, 77, [0, 255, 12].as_slice())
    );
    assert_eq!(inspect.package().extensions(), Some([9, 0, 255].as_slice()));
    assert_eq!(
        opened
            .resource_bytes(&Payload::new("image".into()))
            .unwrap()
            .data
            .bytes,
        raw
    );
    assert!(
        opened
            .resources()
            .unwrap()
            .data
            .iter()
            .any(|r| r.id == "image" && r.available)
    );
}
#[test]
fn malformed_packages_fonts_plugins_and_atomic_relation_refusal_are_typed() {
    let mut s = session();
    for bytes in [b"".as_slice(), b"not a package"] {
        assert!(
            Workspace::new()
                .open(
                    &Payload::new(Open {
                        peer_id: "1".into()
                    }),
                    bytes
                )
                .is_err()
        );
    }
    assert_eq!(
        Workspace::new()
            .create(&Payload::new(Create {
                document_id: "bad".into(),
                peer_id: "1".into()
            }))
            .err()
            .unwrap()
            .code(),
        "bindings.id"
    );
    assert_eq!(
        Workspace::new()
            .create(&Payload::new(Create {
                document_id: "00112233445566778899aabbccddeeff".into(),
                peer_id: "18446744073709551615".into()
            }))
            .err()
            .unwrap()
            .code(),
        "bindings.id"
    );
    assert!(
        s.declare_font(
            &Payload::new(FontDeclaration {
                family: "bad".into(),
                weight: 0,
                style: FontStyle::Normal,
                stretch: 0,
                face_index: u32::MAX
            }),
            b"broken"
        )
        .is_err()
    );
    let spec = PluginSpec {
        name: "x".into(),
        plugin_version: "1".into(),
        sha256: "0".repeat(64),
        phase: PluginPhase::Layout,
        imports: vec![],
        grants: vec![],
        functions: vec![],
        fuel: 100_000,
        memory_pages: 16,
        table_elements: 1024,
        buffer_bytes: 65_536,
    };
    assert_eq!(
        s.load_plugin(&Payload::new(spec), b"malformed")
            .err()
            .unwrap()
            .code(),
        "plugin.hash"
    );
    let node = paragraph(&mut s, "a");
    let before = s.state().unwrap();
    let commands = vec![
        Command::InsertText {
            node,
            at: 0,
            text: "would change".into(),
        },
        Command::RemoveRelation {
            id: "999@99".into(),
        },
    ];
    assert!(s.apply(&Payload::new(Transaction { commands })).is_err());
    assert_eq!(s.state().unwrap(), before);
}
