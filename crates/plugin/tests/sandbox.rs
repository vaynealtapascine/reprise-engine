mod common;
use std::collections::BTreeSet;

use reprise_doc::expr::{Dim, Value};
use reprise_doc::function::Signature;
use reprise_geom::Length;
use reprise_plugin::{
    CallContext, Capability, FunctionDeclaration, Limits, Manifest, Phase, Plugin, PluginFunction,
    abi,
};

use common::{bytes, load, load_limits};

fn error_code(module: &[u8]) -> String {
    Plugin::load(
        module,
        Manifest::new("test", "1", module),
        Phase::Layout,
        BTreeSet::new(),
        Limits::default(),
    )
    .unwrap_err()
    .code
    .to_string()
}

#[test]
fn pins_and_abi_are_validated_without_running() {
    let module = bytes("i32.const 0", "");
    let mut manifest = Manifest::new("test", "1", &module);
    manifest.identity.sha256[0] ^= 1;
    assert_eq!(
        Plugin::load(
            &module,
            manifest,
            Phase::Layout,
            BTreeSet::new(),
            Limits::default()
        )
        .unwrap_err()
        .code,
        "plugin.hash"
    );
    assert_eq!(error_code(&[]), "plugin.invalid");
    assert_eq!(error_code(&[0; 1_048_577]), "plugin.limit");
    assert_eq!(
        error_code(&wat::parse_str("(module)").unwrap()),
        "plugin.abi"
    );
    let start = bytes("i32.const 0", "(func $start unreachable) (start $start)");
    assert_eq!(error_code(&start), "plugin.abi");
}

#[test]
fn fuel_traps_recursion_and_growth_are_bounded_and_repeatable() {
    for (body, expected) in [
        ("(loop $forever br $forever) i32.const 0", "plugin.fuel"),
        ("unreachable", "plugin.trap"),
        (
            "i32.const 2147483647 memory.grow drop i32.const -3",
            "plugin.limit",
        ),
        (
            "local.get $op local.get $input local.get $len local.get $out local.get $cap call $call",
            "plugin.limit",
        ),
    ] {
        let plugin = load(&bytes(body, ""));
        let first = plugin.call(0, &[], &CallContext::default()).unwrap_err();
        assert_eq!(first.code, expected, "{body}");
        for _ in 0..4 {
            assert_eq!(
                plugin.call(0, &[], &CallContext::default()).unwrap_err(),
                first
            );
        }
    }
    let plugin = load_limits(
        &bytes("i32.const 0", ""),
        Limits {
            fuel: 0,
            ..Limits::default()
        },
    );
    assert_eq!(
        plugin
            .call(0, &[], &CallContext::default())
            .unwrap_err()
            .code,
        "plugin.fuel"
    );
}

#[test]
fn static_memory_tables_threads_and_simd_are_refused() {
    let module = bytes("i32.const 0", "");
    // Use source substitution so unexported resources are tested too.
    for source in [
        "(module (memory 65536 65536))",
        "(module (memory 1))",
        "(module (memory 1 1 shared))",
        "(module (table 100000 100000 funcref))",
        "(module (table 1 funcref))",
    ] {
        let rejected = wat::parse_str(source).unwrap();
        assert!(matches!(
            error_code(&rejected).as_str(),
            "plugin.limit" | "plugin.invalid"
        ));
    }
    assert_eq!(
        error_code(&bytes(
            "v128.const i32x4 0 0 0 0 v128.const i32x4 0 0 0 0 i8x16.relaxed_swizzle drop i32.const 0",
            ""
        )),
        "plugin.invalid"
    );
    assert_eq!(
        error_code(&bytes("v128.const i32x4 0 0 0 0 drop i32.const 0", "")),
        "plugin.invalid"
    );
    assert!(load(&module).envelope().runtime.contains("wasmi-2.0.0"));
    let table = bytes(
        "ref.null func i32.const 100000 table.grow drop i32.const -3",
        "(table 1 1024 funcref)",
    );
    assert_eq!(
        load(&table)
            .call(0, &[], &CallContext::default())
            .unwrap_err()
            .code,
        "plugin.limit"
    );
}

#[test]
fn ambient_and_undeclared_imports_do_not_exist() {
    for import in [
        "(import \"wasi_snapshot_preview1\" \"clock_time_get\" (func))",
        "(import \"reprise_v1\" \"random\" (func))",
        "(import \"reprise_v1\" \"read_text\" (func (param i32 i32 i32) (result i32)))",
    ] {
        // Imports must precede definitions.
        let module = bytes("i32.const 0", "");
        let mut source = format!("(module {import} (memory (export \"memory\") 2 16)");
        source.push_str("(func (export \"reprise_abi_version\") (result i32) i32.const 1) (func (export \"reprise_alloc\") (param i32) (result i32) i32.const 0) (func (export \"reprise_call\") (param i32 i32 i32 i32 i32) (result i32) i32.const 0))");
        assert_eq!(
            error_code(&wat::parse_str(source).unwrap()),
            "plugin.capability"
        );
        assert!(!module.is_empty());
    }
    let module = bytes("i32.const 0", "");
    let manifest = Manifest::new("test", "1", &module);
    assert_eq!(
        Plugin::load(
            &module,
            manifest,
            Phase::Layout,
            [Capability::InsertText].into(),
            Limits::default()
        )
        .unwrap_err()
        .code,
        "plugin.capability"
    );
}

#[test]
fn global_and_memory_state_never_leak_and_fuel_counts_are_pinned() {
    let plugin = load(&bytes(
        "global.get $counter i32.const 1 i32.add global.set $counter local.get $out global.get $counter i32.store i32.const 4",
        "(global $counter (mut i32) (i32.const 0))",
    ));
    let first = plugin.call(0, &[], &CallContext::default()).unwrap();
    assert_eq!(abi::read_i32(&first.bytes, 0).unwrap(), 1);
    for _ in 0..10 {
        assert_eq!(plugin.call(0, &[], &CallContext::default()).unwrap(), first);
    }
    // Tight budgets bracket an actual metered call; this is independent of wall-clock speed.
    let module = bytes("i32.const 0", "");
    let used = load(&module)
        .call(0, &[], &CallContext::default())
        .unwrap()
        .fuel_used;
    assert!(used > 0);
    assert!(
        load_limits(
            &module,
            Limits {
                fuel: used,
                ..Limits::default()
            }
        )
        .call(0, &[], &CallContext::default())
        .is_ok()
    );
    assert_eq!(
        load_limits(
            &module,
            Limits {
                fuel: used - 1,
                ..Limits::default()
            }
        )
        .call(0, &[], &CallContext::default())
        .unwrap_err()
        .code,
        "plugin.fuel"
    );
}

#[test]
fn nan_arithmetic_is_canonical_and_nan_cannot_be_an_integer_value() {
    let module = bytes(
        "local.get $out f32.const nan:0x12345 f32.const 0 f32.add i32.reinterpret_f32 i32.store i32.const 4",
        "",
    );
    let result = load(&module).call(0, &[], &CallContext::default()).unwrap();
    assert_eq!(abi::read_i32(&result.bytes, 0).unwrap(), 0x7fc00000);
    assert!(abi::decode_value(&result.bytes, Dim::Length).is_err());
    let nan = bytes(
        "local.get $out i32.const 5 i32.store local.get $out f64.const nan f64.store offset=4 i32.const 12",
        "",
    );
    let f = PluginFunction::new(
        load(&nan),
        FunctionDeclaration {
            operation: 0,
            signature: Signature::new(&[], Dim::Length),
        },
    )
    .unwrap();
    assert_eq!(f.evaluate(&[]).unwrap_err().code, "plugin.result");
}

#[test]
fn malformed_results_pointers_and_huge_requests_are_refused() {
    for body in ["i32.const -1", "i32.const 65537"] {
        assert_eq!(
            load(&bytes(body, ""))
                .call(0, &[], &CallContext::default())
                .unwrap_err()
                .code,
            "plugin.result"
        );
    }
    let plugin = load(&bytes("i32.const 0", ""));
    assert_eq!(
        plugin
            .call(0, &[0; 65537], &CallContext::default())
            .unwrap_err()
            .code,
        "plugin.limit"
    );
    for value in [
        Value::Length(Length::MIN),
        Value::Length(Length::MAX),
        Value::Length(Length::ZERO),
        Value::Length(Length(-1)),
    ] {
        assert_eq!(
            abi::decode_value(&abi::encode_value(value), Dim::Length).unwrap(),
            value
        );
    }
    assert_eq!(
        abi::read_i32(&[], usize::MAX).unwrap_err().code,
        "plugin.result"
    );
    assert_eq!(
        abi::encode_arguments(&vec![Value::Length(Length::ZERO); 257])
            .unwrap_err()
            .code,
        "plugin.limit"
    );
}

#[test]
fn signatures_and_registrations_are_atomic() {
    let module = bytes(
        "local.get $out local.get $input i32.load offset=4 i32.store local.get $out local.get $input i32.load offset=8 i32.store offset=4 i32.const 12",
        "",
    );
    let mut manifest = Manifest::new("identity", "1", &module);
    let declaration = FunctionDeclaration {
        operation: 0,
        signature: Signature::new(&[Dim::Length], Dim::Length),
    };
    manifest
        .functions
        .insert("identity".into(), declaration.clone());
    manifest
        .functions
        .insert("scale".into(), declaration.clone());
    let plugin = Plugin::load(
        &module,
        manifest,
        Phase::Layout,
        BTreeSet::new(),
        Limits::default(),
    )
    .unwrap();
    let mut registry = reprise_doc::FunctionRegistry::builtin();
    assert!(plugin.register_functions(&mut registry).is_err());
    assert!(registry.get("identity").is_none());
    let f = PluginFunction::new(plugin, declaration).unwrap();
    assert!(f.evaluate(&[]).is_err());
    assert!(
        f.evaluate(&[Value::Number(reprise_geom::Fixed::ZERO)])
            .is_err()
    );
    assert_eq!(
        f.evaluate(&[Value::Length(Length::MIN)]).unwrap(),
        Value::Length(Length::MIN)
    );
}

#[test]
fn checked_in_binaries_match_the_pinned_wat_sources() {
    for (source, binary) in [
        (
            include_str!("../test-plugins/demo.wat"),
            include_bytes!("../test-plugins/demo.wasm").as_slice(),
        ),
        (
            include_str!("../test-plugins/loop.wat"),
            include_bytes!("../test-plugins/loop.wasm").as_slice(),
        ),
    ] {
        assert_eq!(wat::parse_str(source).unwrap(), binary);
    }
}
