//! Diagnostic codes the document crate reports as [`Note`](reprise_diag::Note)s
//! (decisions 37 and 39). All are `style.*`: they come from resolving styles
//! and expressions. Layout attaches the subject and publishes them.
//!
//! Every one is a `Warning`. The author asked for something and the output
//! differs: the value was ignored, the term counted as zero, or the style
//! chain was cut short. None is an `Error`, because the block is always laid
//! out. None is `Info`, because the output is not what the author wrote.

use reprise_diag::Code;

/// A stored value this engine can't read: a syntax error, or a newer version
/// of the format. The text is kept in the document unchanged.
pub const STYLE_UNPARSED: Code = Code::new("style.unparsed");
/// An expression has too many nodes, nests too deep or is too long.
pub const STYLE_EXPR_LIMIT: Code = Code::new("style.expr-limit");
/// A dimension error such as `length * length`, or a wrong argument to a function.
pub const STYLE_TYPE: Code = Code::new("style.type-error");
/// An expression calls a function the engine doesn't have registered.
pub const STYLE_UNKNOWN_FUNCTION: Code = Code::new("style.unknown-function");
/// A registered function refused its arguments or returned the wrong type.
pub const STYLE_FUNCTION_FAILED: Code = Code::new("style.function-failed");
/// A basis does not exist in the context (an unknown frame, no page yet).
pub const STYLE_BASIS_UNRESOLVED: Code = Code::new("style.basis-unresolved");
/// A basis exists but depends on its own content, such as an auto-height frame's height.
pub const STYLE_BASIS_INDEFINITE: Code = Code::new("style.basis-indefinite");
/// A value refers to itself: `lh` in `size` or `line-height`.
pub const STYLE_CYCLE: Code = Code::new("style.cycle");
/// Arithmetic hit the end of its range and saturated.
pub const STYLE_SATURATED: Code = Code::new("style.saturated");
/// An expression divided by zero.
pub const STYLE_DIVIDE_BY_ZERO: Code = Code::new("style.divide-by-zero");
/// A named style's parent chain loops back on itself.
pub const STYLE_PARENT_CYCLE: Code = Code::new("style.parent-cycle");
/// A block or named style refers to a style that is not defined.
pub const STYLE_PARENT_MISSING: Code = Code::new("style.parent-missing");
/// A named-style chain is longer than the limit.
pub const STYLE_CHAIN_TOO_LONG: Code = Code::new("style.chain-too-long");
