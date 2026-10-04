//! Pinned sandbox sources, also usable by the WASM fixture build without a WAT parser.
use reprise_doc::expr::Dim;
use reprise_doc::function::Signature;
use reprise_doc::relation::{
    CopyCrossing, CopyInside, CopyPolicy, OnTargetDeleted, Ownership, RoleSpec,
};
use reprise_doc::{DocError, RelationSchema, SchemaId, TargetClass};
use reprise_plugin::{FunctionDeclaration, Limits, Manifest, Phase, Plugin};

pub const DEMO: &[u8] = include_bytes!("../../plugin/test-plugins/demo.wasm");
pub const LOOP: &[u8] = include_bytes!("../../plugin/test-plugins/loop.wasm");
pub const REPORT: SchemaId = SchemaId::new("fixture.plugin-report");

pub fn load(bytes: &[u8]) -> Result<Plugin, DocError> {
    let mut manifest = Manifest::new("fixture.plugin", "1", bytes);
    manifest.functions.insert(
        "plugin-size".into(),
        FunctionDeclaration {
            operation: 1,
            signature: Signature::new(&[Dim::Length], Dim::Length),
        },
    );
    Plugin::load(
        bytes,
        manifest,
        Phase::Layout,
        Default::default(),
        Limits::default(),
    )
    .map_err(|n| DocError::Store(format!("{}: {}", n.code, n.message)))
}

pub fn schema() -> RelationSchema {
    RelationSchema {
        id: REPORT,
        version: 1,
        ownership: Ownership::Independent,
        roles: vec![RoleSpec {
            name: "to".into(),
            accepts: vec![
                TargetClass::Node,
                TargetClass::Range,
                TargetClass::Structural,
                TargetClass::Layout,
                TargetClass::Snapshot,
            ]
            .into(),
            min: 1,
            max: Some(1),
        }],
        params: Vec::new(),
        on_target_deleted: OnTargetDeleted::KeepMissing,
        on_copy: CopyPolicy {
            inside: CopyInside::Duplicate,
            crossing: CopyCrossing::KeepOutside,
        },
    }
}
