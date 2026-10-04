//! Deterministic, fuel-metered core-WASM extensions (04, 36–38).
//! See `docs/plugins.md` for the language-independent ABI.

pub mod abi;
pub mod codes;
mod function;
mod geometry;
mod runtime;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use reprise_diag::Note;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use function::{FunctionDeclaration, PluginFunction};
pub use geometry::{GeometryComposer, PluginGeometry};
use runtime::{Runtime, WasmiRuntime};

pub const ABI_VERSION: u32 = 1;
pub const RUNTIME_VERSION: &str = "wasmi-2.0.0/reprise-abi1";

/// No implicit authority: imports must be declared and separately granted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Capability {
    /// Read only the immutable UTF-8 texts supplied in this call's context.
    ReadText,
    /// Stage insertions for an editing kernel; unavailable in layout mode.
    InsertText,
}

impl Capability {
    pub(crate) fn import(self) -> &'static str {
        match self {
            Self::ReadText => "read_text",
            Self::InsertText => "insert_text",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Phase {
    Layout,
    Editing,
}

/// Complete sandbox settings are reproducibility inputs, including fuel.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Limits {
    pub fuel: u64,
    pub memory_pages: u32,
    pub table_elements: u32,
    pub buffer_bytes: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            fuel: 100_000,
            memory_pages: 16,
            table_elements: 1024,
            buffer_bytes: 65_536,
        }
    }
}

/// A hash is the authority; the name and version are descriptive declarations.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Identity {
    pub name: String,
    pub version: String,
    pub sha256: [u8; 32],
}

pub fn content_hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub identity: Identity,
    pub abi_version: u32,
    pub imports: BTreeSet<Capability>,
    pub functions: BTreeMap<String, FunctionDeclaration>,
}

impl Manifest {
    pub fn new(name: &str, version: &str, bytes: &[u8]) -> Self {
        Self {
            identity: Identity {
                name: name.into(),
                version: version.into(),
                sha256: content_hash(bytes),
            },
            abi_version: ABI_VERSION,
            imports: BTreeSet::new(),
            functions: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Envelope {
    pub identity: Identity,
    pub runtime: String,
    pub phase: Phase,
    pub grants: BTreeSet<Capability>,
    pub imports: BTreeSet<Capability>,
    pub limits: Limits,
    pub functions: Vec<FunctionPin>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FunctionPin {
    pub name: String,
    pub operation: u32,
    pub params: Vec<String>,
    pub variadic: Option<String>,
    pub returns: String,
}

/// Handles are assigned by the caller, never raw engine IDs or ambient resources.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CallContext {
    pub texts: BTreeMap<u32, String>,
    pub editable: BTreeSet<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextInsertion {
    pub handle: u32,
    /// UTF-8 byte offset, checked by the kernel before any mutation.
    pub at: u32,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallResult {
    pub bytes: Vec<u8>,
    pub fuel_used: u64,
    pub insertions: Vec<TextInsertion>,
}

/// The application implements this using one validated editing-kernel transaction.
/// It must commit all insertions atomically or reject all of them.
pub trait EditKernel {
    fn apply(&mut self, insertions: &[TextInsertion]) -> Result<(), Note>;
}

#[derive(Clone)]
pub struct Plugin {
    manifest: Manifest,
    envelope: Envelope,
    runtime: Arc<dyn Runtime>,
}

impl std::fmt::Debug for Plugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plugin")
            .field("envelope", &self.envelope)
            .finish()
    }
}

impl Plugin {
    /// Validates bytes, proposals, imports, ABI types and declared resource maxima.
    /// Does not execute plugin code, including start functions (which are refused).
    pub fn load(
        bytes: &[u8],
        manifest: Manifest,
        phase: Phase,
        grants: BTreeSet<Capability>,
        limits: Limits,
    ) -> Result<Self, Note> {
        if bytes.len() > 1_048_576 {
            return Err(Note::warning(
                codes::LIMIT,
                "plugin module exceeds its byte limit",
            ));
        }
        if content_hash(bytes) != manifest.identity.sha256 {
            return Err(Note::warning(
                codes::HASH,
                "plugin bytes differ from their content pin",
            ));
        }
        if manifest.abi_version != ABI_VERSION
            || manifest.identity.name.is_empty()
            || manifest.identity.name.len() > 256
            || manifest.identity.version.is_empty()
            || manifest.identity.version.len() > 256
        {
            return Err(Note::warning(
                codes::ABI,
                "invalid plugin identity or ABI version",
            ));
        }
        if limits.memory_pages == 0
            || limits.memory_pages > 256
            || limits.table_elements > 65_536
            || limits.buffer_bytes == 0
            || limits.buffer_bytes > 1_048_576
            || limits.fuel > 100_000_000
        {
            return Err(Note::warning(
                codes::LIMIT,
                "sandbox settings exceed hard bounds",
            ));
        }
        if !manifest.imports.is_subset(&grants)
            || (phase == Phase::Layout
                && (grants.contains(&Capability::InsertText)
                    || manifest.imports.contains(&Capability::InsertText)))
        {
            return Err(Note::warning(
                codes::CAPABILITY,
                "plugin imports were not granted for this phase",
            ));
        }
        if manifest.functions.len() > 256
            || manifest
                .functions
                .values()
                .any(|f| f.signature.params.len() > 256)
        {
            return Err(Note::warning(
                codes::LIMIT,
                "too many plugin function declarations or arguments",
            ));
        }
        let runtime = Arc::new(WasmiRuntime::load(bytes, &manifest, &limits)?);
        let functions = manifest
            .functions
            .iter()
            .map(|(name, f)| FunctionPin {
                name: name.clone(),
                operation: f.operation,
                params: f.signature.params.iter().map(ToString::to_string).collect(),
                variadic: f.signature.variadic.map(|d| d.to_string()),
                returns: f.signature.returns.to_string(),
            })
            .collect();
        let envelope = Envelope {
            identity: manifest.identity.clone(),
            runtime: RUNTIME_VERSION.into(),
            phase,
            grants,
            imports: manifest.imports.clone(),
            limits,
            functions,
        };
        Ok(Self {
            manifest,
            envelope,
            runtime,
        })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn envelope(&self) -> &Envelope {
        &self.envelope
    }

    /// A fresh instance for every invocation, with one fuel budget covering all exports.
    /// Layout calls cannot return or apply edits.
    pub fn call(
        &self,
        operation: u32,
        input: &[u8],
        context: &CallContext,
    ) -> Result<CallResult, Note> {
        self.runtime.call(operation, input, context, &self.envelope)
    }

    /// Apply staged commands only after successful execution and result validation.
    pub fn edit(
        &self,
        operation: u32,
        input: &[u8],
        context: &CallContext,
        kernel: &mut dyn EditKernel,
    ) -> Result<CallResult, Note> {
        if self.envelope.phase != Phase::Editing
            || !self.envelope.grants.contains(&Capability::InsertText)
        {
            return Err(Note::warning(
                codes::CAPABILITY,
                "editing authority is absent",
            ));
        }
        let result = self.call(operation, input, context)?;
        if !result.bytes.is_empty() {
            return Err(Note::warning(
                codes::RESULT,
                "editing operations must return an empty result",
            ));
        }
        kernel.apply(&result.insertions)?;
        Ok(result)
    }
}
