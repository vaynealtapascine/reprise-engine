use std::collections::BTreeMap;

use reprise_compose::Greedy;
use reprise_diag::Note;
use reprise_doc::expr::Dim;
use reprise_doc::function::Signature;
use reprise_doc::{
    Authored, BlockKind, Document, Expr, LengthExpr, NodeId, Property, Relation, SchemaRegistry,
    Style, Target,
};
use reprise_edit::{Command, Editor, Transaction};
use reprise_fixtures::{PEER, engine, hostile, plugins};
use reprise_geom::Length;
use reprise_layout::plugins::RelationBinding;
use reprise_layout::{RelationStatus, Resolution};
use reprise_plugin::GeometryComposer;
use reprise_plugin::{
    CallContext, Capability, EditKernel, FunctionDeclaration, Limits, Manifest, Phase, Plugin,
    TextInsertion,
};

#[test]
fn style_function_changes_used_size_and_line_breaks_end_to_end() {
    let doc = Document::new(PEER).unwrap();
    let mut style = Style {
        size: Some(LengthExpr::Pt(Length::from_pt(9))),
        ..Style::default()
    };
    style.set(
        Property::Size,
        Authored::Expr(Expr::parse("plugin-size(9pt)").unwrap()),
    );
    doc.define_style("plugin", &style).unwrap();
    let node = doc.append_block(BlockKind::Paragraph, "plugin", "Plugin type signatures are checked before any code runs. A larger font changes the line breaks in this paragraph.").unwrap();
    doc.commit();
    let mut engine = engine();
    let unknown = engine.layout(&doc);
    assert!(
        unknown
            .diagnostics_with("style.unknown-function")
            .next()
            .is_some()
    );
    engine
        .install_plugin_functions(&plugins::load(plugins::DEMO).unwrap())
        .unwrap();
    let snapshot = engine.layout(&doc);
    assert!(
        snapshot
            .diagnostics_with("style.unknown-function")
            .next()
            .is_none()
    );
    assert_eq!(
        snapshot.block(node).unwrap().style.size,
        Length::from_pt(18)
    );
    assert_ne!(
        unknown.block(node).unwrap().lines,
        snapshot.block(node).unwrap().lines
    );
    assert_eq!(snapshot.to_json(), engine.layout(&doc).to_json());
    assert_eq!(engine.plugins.envelope().modules.len(), 1);
}

#[test]
fn geometry_and_relation_are_active_in_the_source_defined_fixture() {
    let fixture = hostile::plugin_extensions().unwrap();
    let snapshot = fixture.engine.layout(&fixture.doc);
    let block = &snapshot.blocks[0];
    assert!(block.lines.len() > 3);
    assert_ne!(block.lines[0].rect.origin.x, block.lines[1].rect.origin.x);
    assert_eq!(block.style.size, Length::from_pt(18));
    assert!(snapshot.relations[0].applied);
    assert!(matches!(
        snapshot.relations[0].targets[0].resolved,
        Some(Resolution::Line(_))
    ));
    assert!(
        snapshot
            .diagnostics
            .iter()
            .all(|d| !d.code.as_str().starts_with("plugin."))
    );
}

#[test]
fn missing_geometry_preserves_the_frame_and_reports_fallback() {
    let doc = Document::new(PEER).unwrap();
    doc.append_block(BlockKind::Paragraph, "", "Text survives a missing shape.")
        .unwrap();
    doc.commit();
    let mut engine = engine();
    let base = engine.layout(&doc);
    engine.composer = Box::new(GeometryComposer {
        plugin: None,
        operation: 2,
        composer: Box::new(Greedy),
    });
    let missing = engine.layout(&doc);
    assert_eq!(missing.blocks, base.blocks);
    assert!(
        missing
            .diagnostics_with("plugin.unavailable")
            .next()
            .is_some()
    );
}

#[test]
fn missing_and_failing_relations_still_resolve_and_report() {
    for plugin in [None, Some(plugins::load(plugins::LOOP).unwrap())] {
        let doc = Document::new(PEER).unwrap();
        let node = doc
            .append_block(
                BlockKind::Paragraph,
                "",
                "Resolve before calling the sandbox.",
            )
            .unwrap();
        let mut engine = engine();
        engine
            .install_plugin_relation(
                plugins::schema(),
                RelationBinding {
                    plugin,
                    operation: 3,
                },
            )
            .unwrap();
        let id = doc
            .add_relation(
                &engine.schemas,
                &Relation::new(plugins::REPORT).target("to", Target::Node(node)),
            )
            .unwrap();
        doc.commit();
        let result = engine.layout(&doc);
        let relation = result.relation(id).unwrap();
        assert!(!relation.applied);
        assert_eq!(relation.status, RelationStatus::Valid);
        assert_eq!(relation.targets[0].resolved, Some(Resolution::Node(node)));
        assert!(
            result
                .diagnostics_with("relation.not-applied")
                .next()
                .is_some()
        );
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| matches!(d.code.as_str(), "plugin.fuel" | "plugin.unavailable"))
        );
    }
}

#[test]
fn missing_targets_cannot_be_acknowledged_and_delete_policy_skips_execution() {
    for delete in [false, true] {
        let doc = Document::new(PEER).unwrap();
        let node = doc
            .append_block(BlockKind::Paragraph, "", "Delete this target.")
            .unwrap();
        let mut engine = engine();
        let mut schema = plugins::schema();
        if delete {
            schema.on_target_deleted = reprise_doc::relation::OnTargetDeleted::Delete;
        }
        let plugin = plugins::load(if delete { plugins::LOOP } else { plugins::DEMO }).unwrap();
        engine
            .install_plugin_relation(
                schema,
                RelationBinding {
                    plugin: Some(plugin),
                    operation: 3,
                },
            )
            .unwrap();
        let id = doc
            .add_relation(
                &engine.schemas,
                &Relation::new(plugins::REPORT).target("to", Target::Node(node)),
            )
            .unwrap();
        doc.delete_block(node).unwrap();
        doc.commit();
        let result = engine.layout(&doc);
        assert!(!result.relation(id).unwrap().applied);
        assert_eq!(
            result.relation(id).unwrap().status,
            if delete {
                RelationStatus::Deleted
            } else {
                RelationStatus::Missing
            }
        );
        assert!(result.diagnostics_with("plugin.fuel").next().is_none());
    }
}

#[test]
fn envelope_distinguishes_two_budgets_for_the_same_content() {
    let function = plugins::load(plugins::DEMO).unwrap();
    let mut manifest = function.manifest().clone();
    manifest.functions.clear();
    let geometry = Plugin::load(
        plugins::DEMO,
        manifest,
        Phase::Layout,
        Default::default(),
        Limits {
            fuel: 12345,
            ..Limits::default()
        },
    )
    .unwrap();
    let mut engine = engine();
    engine.install_plugin_functions(&function).unwrap();
    engine
        .install_plugin_geometry(geometry, 2, Box::new(Greedy))
        .unwrap();
    let pin = engine.plugins.envelope();
    assert_eq!(pin.modules.len(), 2);
    assert_ne!(pin.modules[0].limits.fuel, pin.modules[1].limits.fuel);
    assert_eq!(pin.geometry.unwrap().0.limits.fuel, 12345);
}

struct Kernel<'a> {
    editor: &'a mut Editor,
    handles: BTreeMap<u32, NodeId>,
}
impl EditKernel for Kernel<'_> {
    fn apply(&mut self, insertions: &[TextInsertion]) -> Result<(), Note> {
        let mut transaction = Transaction::new();
        for insertion in insertions {
            let node = self
                .handles
                .get(&insertion.handle)
                .copied()
                .ok_or_else(|| {
                    Note::warning(reprise_plugin::codes::CAPABILITY, "unknown edit handle")
                })?;
            transaction.push(Command::InsertText {
                node,
                at: insertion.at as usize,
                text: insertion.text.clone(),
            });
        }
        self.editor
            .apply(&transaction)
            .map(|_| ())
            .map_err(|e| e.note())
    }
}

fn editing_module(tail: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module
      (import "reprise_v1" "insert_text" (func $insert (param i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 2 16) (data (i32.const 64) "Hi ")
      (global $heap (mut i32) (i32.const 1024))
      (func (export "reprise_abi_version") (result i32) i32.const 1)
      (func (export "reprise_alloc") (param $len i32) (result i32) (local $ptr i32)
        global.get $heap local.tee $ptr local.get $len i32.add global.set $heap local.get $ptr)
      (func (export "reprise_call") (param i32 i32 i32 i32 i32) (result i32)
        i32.const 7 i32.const 0 i32.const 64 i32.const 3 call $insert drop
        {tail} i32.const 0))"#
    ))
    .unwrap()
}

#[test]
fn editing_capability_uses_real_kernel_validation_atomicity_and_undo() {
    for tail in [
        "",
        "i32.const 7 i32.const 4 i32.const 64 i32.const 3 call $insert drop",
        "unreachable",
    ] {
        let doc = Document::new(PEER).unwrap();
        let node = doc.append_block(BlockKind::Paragraph, "", "é").unwrap();
        doc.commit();
        let mut editor = Editor::new(doc, SchemaRegistry::builtin());
        let revision = editor.document().revision();
        let bytes = editing_module(tail);
        let mut manifest = Manifest::new("edit", "1", &bytes);
        manifest.imports.insert(Capability::InsertText);
        let plugin = Plugin::load(
            &bytes,
            manifest,
            Phase::Editing,
            [Capability::InsertText].into(),
            Limits::default(),
        )
        .unwrap();
        let result = plugin.edit(
            0,
            &[],
            &CallContext {
                editable: [7].into(),
                ..CallContext::default()
            },
            &mut Kernel {
                editor: &mut editor,
                handles: [(7, node)].into(),
            },
        );
        if tail.is_empty() {
            result.unwrap();
            assert_eq!(
                editor.document().block(node).unwrap().text.to_string(),
                "Hi é"
            );
            assert!(editor.undo().unwrap());
            assert_eq!(editor.document().block(node).unwrap().text.to_string(), "é");
            assert!(editor.redo().unwrap());
            assert_eq!(
                editor.document().block(node).unwrap().text.to_string(),
                "Hi é"
            );
        } else {
            assert!(result.is_err());
            assert_eq!(editor.document().block(node).unwrap().text.to_string(), "é");
            assert_eq!(editor.document().revision(), revision);
            assert_eq!(editor.undo_count(), 0);
        }
    }
}

#[test]
fn failed_plugin_style_skips_the_layer_and_preserves_authored_expression() {
    let fixture = hostile::plugin_fuel().unwrap();
    let snapshot = fixture.engine.layout(&fixture.doc);
    let failure = snapshot
        .diagnostics_with("style.function-failed")
        .next()
        .unwrap();
    assert!(failure.message.contains("plugin.fuel"));
    assert!(
        fixture
            .doc
            .style("plugin-loop")
            .unwrap()
            .values
            .contains_key(&Property::Size)
    );
    assert_eq!(snapshot.blocks[0].style.size, Length::from_pt(10));
    assert!(!snapshot.relations[0].applied);
}

#[test]
fn incompatible_function_result_degrades_with_the_frozen_style_diagnostic() {
    let bytes = wat::parse_str(include_str!("../../plugin/test-plugins/demo.wat")).unwrap();
    let mut manifest = Manifest::new("bad-signature", "1", &bytes);
    manifest.functions.insert(
        "plugin-size".into(),
        FunctionDeclaration {
            operation: 3,
            signature: Signature::new(&[Dim::Length], Dim::Length),
        },
    );
    let plugin = Plugin::load(
        &bytes,
        manifest,
        Phase::Layout,
        Default::default(),
        Limits::default(),
    )
    .unwrap();
    let fixture = hostile::plugin_fuel().unwrap();
    let mut engine = engine();
    engine.install_plugin_functions(&plugin).unwrap();
    let snapshot = engine.layout(&fixture.doc);
    let failure = snapshot
        .diagnostics_with("style.function-failed")
        .next()
        .unwrap();
    assert!(failure.message.contains("plugin.result"));
}
