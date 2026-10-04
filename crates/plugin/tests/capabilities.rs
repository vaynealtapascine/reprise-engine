use reprise_diag::Note;
use reprise_plugin::{
    CallContext, Capability, EditKernel, Limits, Manifest, Phase, Plugin, TextInsertion, abi,
};
use std::collections::BTreeSet;

fn module(import: &str, body: &str) -> Vec<u8> {
    wat::parse_str(format!(r#"(module {import}
      (memory (export "memory") 2 16)
      (data (i32.const 64) "hello")
      (global $heap (mut i32) (i32.const 1024))
      (func (export "reprise_abi_version") (result i32) i32.const 1)
      (func (export "reprise_alloc") (param $len i32) (result i32) (local $ptr i32)
        global.get $heap local.tee $ptr local.get $len i32.add global.set $heap local.get $ptr)
      (func (export "reprise_call") (param i32 i32 i32) (param $out i32) (param i32) (result i32) {body}))"#)).unwrap()
}

#[derive(Default)]
struct Kernel {
    calls: usize,
    insertions: Vec<TextInsertion>,
}
impl EditKernel for Kernel {
    fn apply(&mut self, edits: &[TextInsertion]) -> Result<(), Note> {
        self.calls += 1;
        self.insertions.extend_from_slice(edits);
        Ok(())
    }
}

#[test]
fn immutable_text_import_has_only_call_scoped_handles() {
    let bytes = module(
        "(import \"reprise_v1\" \"read_text\" (func $read (param i32 i32 i32) (result i32)))",
        "i32.const 7 local.get $out i32.const 100 call $read",
    );
    let mut manifest = Manifest::new("read", "1", &bytes);
    manifest.imports.insert(Capability::ReadText);
    assert_eq!(
        Plugin::load(
            &bytes,
            manifest.clone(),
            Phase::Layout,
            BTreeSet::new(),
            Limits::default()
        )
        .unwrap_err()
        .code,
        "plugin.capability"
    );
    let plugin = Plugin::load(
        &bytes,
        manifest,
        Phase::Layout,
        [Capability::ReadText].into(),
        Limits::default(),
    )
    .unwrap();
    let context = CallContext {
        texts: [(7, "é UTF-8".into())].into(),
        ..CallContext::default()
    };
    assert_eq!(
        plugin.call(0, &[], &context).unwrap().bytes,
        "é UTF-8".as_bytes()
    );
    assert_eq!(
        plugin
            .call(0, &[], &CallContext::default())
            .unwrap_err()
            .code,
        "plugin.result"
    );
    let huge = CallContext {
        texts: [(7, "x".repeat(65537))].into(),
        ..CallContext::default()
    };
    assert_eq!(plugin.call(0, &[], &huge).unwrap_err().code, "plugin.limit");
}

#[test]
fn edits_are_staged_discarded_on_failure_and_committed_once() {
    let import = "(import \"reprise_v1\" \"insert_text\" (func $insert (param i32 i32 i32 i32) (result i32)))";
    let insert = "i32.const 7 i32.const 0 i32.const 64 i32.const 5 call $insert drop";
    for (tail, expected_calls) in [
        ("i32.const 0", 1),
        ("unreachable", 0),
        ("i32.const -1", 0),
        ("local.get $out i32.const 1 i32.store i32.const 4", 0),
    ] {
        let bytes = module(import, &format!("{insert} {tail}"));
        let mut manifest = Manifest::new("edit", "1", &bytes);
        manifest.imports.insert(Capability::InsertText);
        assert_eq!(
            Plugin::load(
                &bytes,
                manifest.clone(),
                Phase::Layout,
                [Capability::InsertText].into(),
                Limits::default()
            )
            .unwrap_err()
            .code,
            "plugin.capability"
        );
        let plugin = Plugin::load(
            &bytes,
            manifest,
            Phase::Editing,
            [Capability::InsertText].into(),
            Limits::default(),
        )
        .unwrap();
        let mut kernel = Kernel::default();
        let context = CallContext {
            editable: [7].into(),
            ..CallContext::default()
        };
        let result = plugin.edit(0, &[], &context, &mut kernel);
        assert_eq!(kernel.calls, expected_calls);
        assert_eq!(result.is_ok(), expected_calls == 1);
        if expected_calls == 1 {
            assert_eq!(
                kernel.insertions,
                vec![TextInsertion {
                    handle: 7,
                    at: 0,
                    text: "hello".into()
                }]
            );
        }
        assert_eq!(
            plugin
                .edit(0, &[], &CallContext::default(), &mut Kernel::default())
                .unwrap_err()
                .code,
            "plugin.capability"
        );
    }
}

#[test]
fn host_import_copy_cost_is_charged_as_fuel() {
    let bytes = module(
        "(import \"reprise_v1\" \"read_text\" (func $read (param i32 i32 i32) (result i32)))",
        "i32.const 7 local.get $out i32.const 65536 call $read",
    );
    let mut manifest = Manifest::new("read", "1", &bytes);
    manifest.imports.insert(Capability::ReadText);
    let plugin = Plugin::load(
        &bytes,
        manifest,
        Phase::Layout,
        [Capability::ReadText].into(),
        Limits {
            fuel: 100,
            ..Limits::default()
        },
    )
    .unwrap();
    let context = CallContext {
        texts: [(7, "x".repeat(1000))].into(),
        ..CallContext::default()
    };
    assert_eq!(
        plugin.call(0, &[], &context).unwrap_err().code,
        "plugin.fuel"
    );
    assert_eq!(abi::read_i32(&[], 0).unwrap_err().code, "plugin.result");
}
