use reprise_diag::Note;
use reprise_doc::FunctionRegistry;
use reprise_doc::expr::Value;
use reprise_doc::function::{FunctionError, PureFunction, RegistryError, Signature};

use crate::{CallContext, Phase, Plugin, abi, codes};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionDeclaration {
    pub operation: u32,
    pub signature: Signature,
}

/// No mutable state or diagnostic side channel: errors travel with the call.
#[derive(Clone, Debug)]
pub struct PluginFunction {
    plugin: Plugin,
    declaration: FunctionDeclaration,
}

impl PluginFunction {
    pub fn new(plugin: Plugin, declaration: FunctionDeclaration) -> Result<Self, Note> {
        if plugin.envelope().phase != Phase::Layout {
            return Err(Note::warning(
                codes::CAPABILITY,
                "an editing plugin cannot be a pure function",
            ));
        }
        Ok(Self {
            plugin,
            declaration,
        })
    }

    pub fn evaluate(&self, args: &[Value]) -> Result<Value, Note> {
        let sig = &self.declaration.signature;
        if args.len() < sig.params.len()
            || (sig.variadic.is_none() && args.len() != sig.params.len())
            || args
                .iter()
                .enumerate()
                .any(|(i, v)| sig.params.get(i).copied().or(sig.variadic) != Some(v.dim()))
        {
            return Err(abi::bad());
        }
        let result = self.plugin.call(
            self.declaration.operation,
            &abi::encode_arguments(args)?,
            &CallContext::default(),
        )?;
        abi::decode_value(&result.bytes, sig.returns)
    }
}

impl PureFunction for PluginFunction {
    fn signature(&self) -> &Signature {
        &self.declaration.signature
    }
    fn call(&self, args: &[Value]) -> Result<Value, FunctionError> {
        self.evaluate(args)
            .map_err(|n| FunctionError(format!("{}: {}", n.code, n.message)))
    }
}

impl Plugin {
    /// Register atomically; a refused/missing module leaves its functions unknown.
    pub fn register_functions(&self, registry: &mut FunctionRegistry) -> Result<(), RegistryError> {
        let mut next = registry.clone();
        for (name, declaration) in &self.manifest().functions {
            let f = PluginFunction::new(self.clone(), declaration.clone())
                .map_err(|_| RegistryError::BadName(name.clone()))?;
            next.register(name, f)?;
        }
        *registry = next;
        Ok(())
    }
}
