//! Registered pure functions for expressions (decisions 17 and 36).
//!
//! A [`FunctionRegistry`] is engine configuration, like
//! [`SchemaRegistry`](crate::SchemaRegistry): the document stores only a
//! function's *name* inside an expression, and the engine decides what that
//! name means. A document that calls a function the engine doesn't have keeps
//! the call; the expression is reported with `style.unknown-function` and the
//! property falls back to its inherited value (37).
//!
//! # What a function must be
//!
//! -   **Pure and deterministic (38).** No time, randomness, I/O or hidden
//!     state: the same arguments always give the same value, on every
//!     platform. Integer arithmetic only; saturate rather than wrap.
//! -   **Total.** It never panics. Arguments it can't accept give
//!     `Err(FunctionError)`, which the evaluator reports as
//!     `style.function-failed` and counts as zero.
//! -   **Honest about its types.** The [`Signature`] is checked before
//!     evaluation, and the evaluator checks the returned [`Value`] against
//!     `returns`: a function that returns another dimension is reported, not
//!     trusted.
//!
//! A [`Signature`] is plain data, so a sandboxed plugin (36) can declare its
//! functions without running any of its code at type-check time.
//!
//! A percentage argument is accepted where the signature says length: it
//! becomes a length of the property's declared basis first (see
//! [`crate::expr`]).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use reprise_geom::{Length, div_round};

use crate::context::Basis;
use crate::expr::{Dim, RESERVED, Value, valid_name};

/// The types of a function: its parameters, optionally a repeated last one,
/// and what it returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub params: Vec<Dim>,
    /// If set, any number of further arguments of this dimension.
    pub variadic: Option<Dim>,
    pub returns: Dim,
}

impl Signature {
    pub fn new(params: &[Dim], returns: Dim) -> Signature {
        Signature {
            params: params.to_vec(),
            variadic: None,
            returns,
        }
    }
}

/// A function refused its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionError(pub String);

impl fmt::Display for FunctionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FunctionError {}

/// A function expressions may call. See the module docs for what it promises.
pub trait PureFunction: Send + Sync {
    fn signature(&self) -> &Signature;

    /// Called only with arguments that match [`PureFunction::signature`].
    fn call(&self, args: &[Value]) -> Result<Value, FunctionError>;
}

/// A function that is a plain Rust function.
pub struct Builtin {
    signature: Signature,
    f: fn(&[Value]) -> Result<Value, FunctionError>,
}

impl Builtin {
    pub fn new(signature: Signature, f: fn(&[Value]) -> Result<Value, FunctionError>) -> Builtin {
        Builtin { signature, f }
    }
}

impl PureFunction for Builtin {
    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn call(&self, args: &[Value]) -> Result<Value, FunctionError> {
        (self.f)(args)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    #[error("{0:?} is not a valid function name: use lower-case words joined by hyphens")]
    BadName(String),
    #[error("{0:?} is a name the expression language uses itself")]
    Reserved(String),
    #[error("a function called {0:?} is already registered")]
    Duplicate(String),
}

/// The functions an engine offers to expressions.
#[derive(Clone, Default)]
pub struct FunctionRegistry {
    // A map so the registry's own listing is ordered; lookup is by name.
    functions: BTreeMap<String, Arc<dyn PureFunction>>,
}

impl fmt::Debug for FunctionRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.functions.keys()).finish()
    }
}

impl FunctionRegistry {
    /// No functions at all.
    pub fn empty() -> FunctionRegistry {
        FunctionRegistry::default()
    }

    /// The functions every engine has: [`builtin::ratio`], [`builtin::scale`]
    /// and [`builtin::round_to`].
    pub fn builtin() -> FunctionRegistry {
        let mut registry = FunctionRegistry::empty();
        for (name, f) in builtin::all() {
            // The built-in names are valid and distinct (tested).
            let _ = registry.register(name, f);
        }
        registry
    }

    pub fn register(
        &mut self,
        name: &str,
        function: impl PureFunction + 'static,
    ) -> Result<(), RegistryError> {
        if !valid_name(name) || Basis::from_ident(name).is_some() {
            return Err(RegistryError::BadName(name.into()));
        }
        if RESERVED.contains(&name) {
            return Err(RegistryError::Reserved(name.into()));
        }
        if self.functions.contains_key(name) {
            return Err(RegistryError::Duplicate(name.into()));
        }
        self.functions.insert(name.into(), Arc::new(function));
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&dyn PureFunction> {
        self.functions.get(name).map(|f| &**f)
    }

    /// Registered names in order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.functions.keys().map(String::as_str)
    }
}

/// The functions in [`FunctionRegistry::builtin`], proving the mechanism.
pub mod builtin {
    use super::*;

    pub(super) fn all() -> Vec<(&'static str, Builtin)> {
        vec![
            (
                "ratio",
                Builtin::new(
                    Signature::new(&[Dim::Number, Dim::Number], Dim::Ratio),
                    ratio,
                ),
            ),
            (
                "scale",
                Builtin::new(
                    Signature::new(&[Dim::Length, Dim::Ratio], Dim::Length),
                    scale,
                ),
            ),
            (
                "round-to",
                Builtin::new(
                    Signature::new(&[Dim::Length, Dim::Length], Dim::Length),
                    round_to,
                ),
            ),
        ]
    }

    fn bad_arguments() -> FunctionError {
        FunctionError("unexpected arguments".into())
    }

    /// `ratio(a, b)`: the ratio `a : b`. A zero `b` is refused.
    pub fn ratio(args: &[Value]) -> Result<Value, FunctionError> {
        match args {
            [Value::Number(num), Value::Number(den)] if den.0 != 0 => Ok(Value::Ratio {
                num: *num,
                den: *den,
            }),
            [Value::Number(_), Value::Number(_)] => {
                Err(FunctionError("the second term of a ratio is zero".into()))
            }
            _ => Err(bad_arguments()),
        }
    }

    /// `scale(length, ratio)`: `length * num / den`, rounded half away from
    /// zero and saturating.
    pub fn scale(args: &[Value]) -> Result<Value, FunctionError> {
        match args {
            [Value::Length(l), Value::Ratio { num, den }] if den.0 != 0 => {
                let q = div_round(l.0 as i64 * num.0 as i64, den.0 as i64);
                Ok(Value::Length(Length(
                    q.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
                )))
            }
            [Value::Length(_), Value::Ratio { .. }] => {
                Err(FunctionError("the ratio's second term is zero".into()))
            }
            _ => Err(bad_arguments()),
        }
    }

    /// `round-to(length, step)`: the multiple of `|step|` nearest `length`,
    /// halves away from zero. A zero step is refused.
    pub fn round_to(args: &[Value]) -> Result<Value, FunctionError> {
        match args {
            [Value::Length(l), Value::Length(step)] if step.0 != 0 => {
                let step = (step.0 as i64).abs();
                let multiples = div_round(l.0 as i64, step);
                let v = multiples.saturating_mul(step);
                Ok(Value::Length(Length(
                    v.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
                )))
            }
            [Value::Length(_), Value::Length(_)] => Err(FunctionError("the step is zero".into())),
            _ => Err(bad_arguments()),
        }
    }
}
