//! Bounded typed expressions (decision 17), like CSS `calc()`.
//!
//! ```text
//! 1.2em                       a length relative to the font size
//! 50% * frame-width("main")   half of the frame called "main"
//! clamp(8pt, 2.5%, 14pt)      a percentage of the property's own basis
//! scale(12pt, ratio(3, 2))    a registered function
//! ```
//!
//! # Values and dimensions
//!
//! Every value has a [`Dim`]:
//!
//! | Dim | Written | Held as |
//! | --- | --- | --- |
//! | length | `12pt`, `1.5em`, `1lh`, `frame-width` | [`Length`] |
//! | number | `2.5` | [`Fixed`] (16.16) |
//! | percentage | `50%` | [`Fixed`] in percent points |
//! | ratio | `ratio(16, 9)` | a pair of [`Fixed`] |
//!
//! Percentage is a dimension of its own because it means nothing until it has a
//! basis. It becomes a length in exactly two ways:
//!
//! -   Multiplied by a length, it scales that length: `50% * 10pt` is `5pt`.
//! -   Used where a length is needed (added to a length, the root of the
//!     expression, a `min`/`max`/`clamp` argument next to a length, a function
//!     argument that takes a length), it is taken of the *declared basis* of the
//!     property (see [`Scope`] and `crate::context`).
//!
//! The rules, checked before evaluation by [`Expr::check`]:
//!
//! | Expression | Type |
//! | --- | --- |
//! | `a + b`, `a - b` | the same dimension; length with percentage gives a length |
//! | `number * number` | number |
//! | `length * number` | length |
//! | `percentage * number` | percentage |
//! | `percentage * length` | length (`length * length` is a type error) |
//! | `a / number` | the dimension of `a` |
//! | `length / length` | number |
//! | `-a` | the dimension of `a` (not a ratio) |
//! | `min`, `max`, `clamp` | their arguments' common dimension |
//!
//! Anything else is a type error, reported as `style.type-error`; ratios take
//! part only in registered functions.
//!
//! # Numeric policy (19)
//!
//! Evaluation is integer arithmetic only. Every operation saturates at the
//! limits of its type (reported once as `style.saturated`), every division
//! rounds half away from zero, and a division by zero saturates by the sign of
//! the numerator (reported as `style.divide-by-zero`). `clamp(lo, v, hi)` is
//! `max(lo, min(v, hi))`, so `lo` wins when `lo > hi`.
//!
//! A literal is kept as the decimal the author wrote (at most 18 digits, of
//! which at most 12 follow the point) and converted once, when the expression
//! is evaluated, so the text form round-trips exactly.
//!
//! # Bounds
//!
//! An [`Expr`] has at most [`MAX_NODES`] nodes, nests at most [`MAX_DEPTH`]
//! deep, and parses from at most [`MAX_TEXT`] bytes. Parsing and
//! [`Expr::new`] enforce all three before building anything deeper, so a
//! hostile expression is reported as `style.expr-limit` and never exhausts the
//! stack. Every pass over an `Expr` (check, fold, evaluate, print) recurses
//! at most [`MAX_DEPTH`] levels.
//!
//! # Text form
//!
//! The text form is canonical: `Expr::parse(&e.to_string())` is `e`, and
//! printing a parsed expression gives the canonical spelling (decimals without
//! trailing zeros, binary operators surrounded by spaces, parentheses only
//! where precedence needs them). `calc(...)` is accepted as plain
//! parentheses. Names are lower-case ASCII.
//!
//! # Stages
//!
//! [`Expr::compute`] turns an authored expression into a [`ComputedLength`] (08):
//! it binds the property's percentage basis, substitutes the font terms the
//! [`Scope`] knows (`em`, `lh`), and folds every subexpression that no longer
//! depends on a context into one literal. What remains mentions only geometric
//! bases ([`Basis`]) and, for properties relative to their own font size, a
//! free `em`. [`ComputedLength::used`] resolves that against a
//! [`ResolutionContext`].

use std::collections::BTreeSet;
use std::fmt;

use reprise_diag::Note;
use reprise_geom::{Fixed, Length, div_round};
use serde::{Deserialize, Serialize};

use crate::codes;
use crate::context::{Basis, BasisScope, Level, ResolutionContext, Resolved};
use crate::function::FunctionRegistry;

/// The most nodes an expression may have.
pub const MAX_NODES: usize = 256;
/// The deepest an expression may nest. A leaf has depth 1.
pub const MAX_DEPTH: usize = 32;
/// The longest text [`Expr::parse`] accepts, in bytes.
pub const MAX_TEXT: usize = 2048;
/// The longest function or frame name, in bytes.
pub const MAX_NAME: usize = 64;

const MAX_MANTISSA: u64 = 999_999_999_999_999_999;
const MAX_SCALE: u8 = 12;
/// Names the language itself uses; registered functions can't take them.
pub(crate) const RESERVED: [&str; 4] = ["calc", "min", "max", "clamp"];

// ---------------------------------------------------------------------------
// Decimals, units, dimensions

/// A non-negative decimal literal: `mantissa / 10^scale`, always in lowest terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Decimal {
    mantissa: u64,
    scale: u8,
}

impl Decimal {
    pub const ZERO: Decimal = Decimal {
        mantissa: 0,
        scale: 0,
    };

    /// `None` if the mantissa has more than 18 digits or the scale is above 12.
    pub fn new(mut mantissa: u64, mut scale: u8) -> Option<Decimal> {
        if mantissa > MAX_MANTISSA || scale > MAX_SCALE {
            return None;
        }
        while scale > 0 && mantissa.is_multiple_of(10) {
            mantissa /= 10;
            scale -= 1;
        }
        Some(Decimal { mantissa, scale })
    }

    pub fn int(n: u64) -> Option<Decimal> {
        Decimal::new(n, 0)
    }

    /// `self * k`, rounded half away from zero.
    fn times(self, k: i128) -> i128 {
        div_round_128(self.mantissa as i128 * k, 10i128.pow(self.scale as u32))
    }
}

impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let p = 10u64.pow(self.scale as u32);
        if self.scale == 0 {
            write!(f, "{}", self.mantissa)
        } else {
            write!(
                f,
                "{}.{:0width$}",
                self.mantissa / p,
                self.mantissa % p,
                width = self.scale as usize
            )
        }
    }
}

/// `num / den` for `den > 0`, rounding half away from zero.
fn div_round_128(num: i128, den: i128) -> i128 {
    let magnitude = (num.abs() + den / 2) / den;
    if num < 0 { -magnitude } else { magnitude }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Unit {
    /// No unit: a number.
    None,
    Pt,
    /// The font size ([`crate::style`] defines whose).
    Em,
    /// The line height.
    Lh,
    /// A percentage of the declared basis.
    Percent,
}

impl Unit {
    fn suffix(self) -> &'static str {
        match self {
            Unit::None => "",
            Unit::Pt => "pt",
            Unit::Em => "em",
            Unit::Lh => "lh",
            Unit::Percent => "%",
        }
    }

    fn dim(self) -> Dim {
        match self {
            Unit::None => Dim::Number,
            Unit::Pt | Unit::Em | Unit::Lh => Dim::Length,
            Unit::Percent => Dim::Percentage,
        }
    }
}

/// The dimension of a value (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Dim {
    Length,
    Number,
    Percentage,
    Ratio,
}

impl fmt::Display for Dim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Dim::Length => "length",
            Dim::Number => "number",
            Dim::Percentage => "percentage",
            Dim::Ratio => "ratio",
        })
    }
}

/// A value an expression produces and a registered function takes and returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value {
    Length(Length),
    Number(Fixed),
    /// In percent points: 50% is `Fixed::from_int(50)`.
    Percentage(Fixed),
    Ratio {
        num: Fixed,
        den: Fixed,
    },
}

impl Value {
    pub fn dim(&self) -> Dim {
        match self {
            Value::Length(_) => Dim::Length,
            Value::Number(_) => Dim::Number,
            Value::Percentage(_) => Dim::Percentage,
            Value::Ratio { .. } => Dim::Ratio,
        }
    }

    /// Zero of a dimension, the stand-in for a value that could not be computed.
    pub fn zero(dim: Dim) -> Value {
        match dim {
            Dim::Length => Value::Length(Length::ZERO),
            Dim::Number => Value::Number(Fixed::ZERO),
            Dim::Percentage => Value::Percentage(Fixed::ZERO),
            Dim::Ratio => Value::Ratio {
                num: Fixed::ZERO,
                den: Fixed::ONE,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// The tree

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
}

impl Op {
    fn prec(self) -> u8 {
        match self {
            Op::Add | Op::Sub => 1,
            Op::Mul | Op::Div => 2,
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Op::Add => "+",
            Op::Sub => "-",
            Op::Mul => "*",
            Op::Div => "/",
        }
    }
}

/// One node of an expression. Build an [`Expr`] from it with [`Expr::new`],
/// which checks the bounds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Quantity(Decimal, Unit),
    /// A geometric reference such as `frame-width`; always a length.
    Basis(Basis),
    Neg(Box<Node>),
    Binary(Op, Box<Node>, Box<Node>),
    Min(Vec<Node>),
    Max(Vec<Node>),
    Clamp(Box<Node>, Box<Node>, Box<Node>),
    /// A registered function. The name must not be one the language uses.
    Call(String, Vec<Node>),
}

impl Node {
    /// `n` as a number literal, if it fits.
    pub fn number(n: u64) -> Option<Node> {
        Some(Node::Quantity(Decimal::int(n)?, Unit::None))
    }

    pub fn binary(op: Op, a: Node, b: Node) -> Node {
        Node::Binary(op, Box::new(a), Box::new(b))
    }
}

/// A length as an exact literal: 1/1024 pt is `0.0009765625pt`.
pub(crate) fn length_node(l: Length) -> Node {
    let magnitude = l.0.unsigned_abs() as u64 * 9_765_625;
    let lit = Node::Quantity(
        Decimal::new(magnitude, 10).unwrap_or(Decimal::ZERO),
        Unit::Pt,
    );
    if l.0 < 0 {
        Node::Neg(Box::new(lit))
    } else {
        lit
    }
}

/// The length a node is, if it is a (possibly negated) `pt` literal that fits.
fn literal_length(n: &Node) -> Option<Length> {
    let exact = |d: &Decimal| i32::try_from(d.times(1024)).ok();
    match n {
        Node::Quantity(d, Unit::Pt) => exact(d).map(Length),
        Node::Neg(inner) => match &**inner {
            Node::Quantity(d, Unit::Pt) => exact(d).map(|v| Length(v.saturating_neg())),
            _ => None,
        },
        _ => None,
    }
}

/// What a parse or [`Expr::new`] can reject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExprError {
    Syntax {
        at: usize,
        expected: &'static str,
    },
    /// A number with too many digits.
    Literal {
        at: usize,
    },
    Limit(Limit),
    /// A tree that [`Expr::new`] can't print as text that parses back to it.
    Shape(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    Nodes,
    Depth,
    Text,
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExprError::Syntax { at, expected } => write!(f, "expected {expected} at byte {at}"),
            ExprError::Literal { at } => write!(f, "the number at byte {at} has too many digits"),
            ExprError::Limit(Limit::Nodes) => {
                write!(f, "more than {MAX_NODES} nodes in one expression")
            }
            ExprError::Limit(Limit::Depth) => {
                write!(f, "nested deeper than {MAX_DEPTH} levels")
            }
            ExprError::Limit(Limit::Text) => write!(f, "longer than {MAX_TEXT} bytes"),
            ExprError::Shape(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for ExprError {}

impl ExprError {
    /// `style.expr-limit` for the bounds, `style.unparsed` for everything else.
    pub fn code(&self) -> reprise_diag::Code {
        match self {
            ExprError::Limit(_) => codes::STYLE_EXPR_LIMIT,
            _ => codes::STYLE_UNPARSED,
        }
    }

    pub fn note(&self) -> Note {
        Note::warning(self.code(), self.to_string())
    }
}

/// A type or reference problem found before or while computing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckError {
    UnknownFunction(String),
    Type(String),
    /// Something the expression needs is not available: `em` with no font, a
    /// percentage with no basis.
    Unresolved(String),
    /// A term that refers back to the property being computed.
    Cycle(String),
    Limit(Limit),
}

impl CheckError {
    pub fn note(&self) -> Note {
        match self {
            CheckError::UnknownFunction(n) => Note::warning(
                codes::STYLE_UNKNOWN_FUNCTION,
                format!("no function called {n} is registered"),
            ),
            CheckError::Type(m) => Note::warning(codes::STYLE_TYPE, m.clone()),
            CheckError::Unresolved(m) => Note::warning(codes::STYLE_BASIS_UNRESOLVED, m.clone()),
            CheckError::Cycle(m) => Note::warning(codes::STYLE_CYCLE, m.clone()),
            CheckError::Limit(l) => ExprError::Limit(*l).note(),
        }
    }
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.note().message)
    }
}

/// What an expression's value depends on (27), in a fixed order so a set of
/// them can key a cache.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dependency {
    /// The font size.
    Em,
    /// The line height.
    Lh,
    /// The declared basis of a percentage that has not been bound yet.
    PercentBasis,
    /// A geometric basis.
    Basis(Basis),
    /// A registered function: its definition is part of the value.
    Function(String),
}

impl fmt::Display for Dependency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Dependency::Em => f.write_str("em"),
            Dependency::Lh => f.write_str("lh"),
            Dependency::PercentBasis => f.write_str("percentage basis"),
            Dependency::Basis(b) => write!(f, "{b}"),
            Dependency::Function(n) => write!(f, "function {n}"),
        }
    }
}

/// A bounded typed expression (see the module docs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expr {
    root: Node,
}

impl Expr {
    /// Checks the bounds (node count, depth) and that the tree has a text form
    /// that parses back to it. Never recurses deeper than the bounds allow.
    pub fn new(root: Node) -> Result<Expr, ExprError> {
        match validate(&root) {
            Ok(()) => Ok(Expr { root }),
            Err(e) => {
                dismantle(root);
                Err(e)
            }
        }
    }

    /// Parses the text form (see the module docs).
    pub fn parse(text: &str) -> Result<Expr, ExprError> {
        if text.len() > MAX_TEXT {
            return Err(ExprError::Limit(Limit::Text));
        }
        let mut p = Parser {
            src: text,
            pos: 0,
            nodes: 0,
        };
        let root = p.expr(1)?;
        p.skip_space();
        if p.pos < text.len() {
            return Err(p.syntax("the end of the expression"));
        }
        Expr::new(root)
    }

    /// For trees this crate builds itself that are small and valid by
    /// construction.
    pub(crate) fn trusted(root: Node) -> Expr {
        Expr { root }
    }

    pub fn node(&self) -> &Node {
        &self.root
    }

    pub fn into_node(self) -> Node {
        self.root
    }

    pub fn node_count(&self) -> usize {
        fn count(n: &Node) -> usize {
            1 + match n {
                Node::Quantity(..) | Node::Basis(_) => 0,
                Node::Neg(x) => count(x),
                Node::Binary(_, a, b) => count(a) + count(b),
                Node::Min(v) | Node::Max(v) | Node::Call(_, v) => v.iter().map(count).sum(),
                Node::Clamp(a, b, c) => count(a) + count(b) + count(c),
            }
        }
        count(&self.root)
    }

    /// The type of the expression, or the first problem (in reading order).
    pub fn check(&self, functions: &FunctionRegistry) -> Result<Dim, CheckError> {
        Rewrite::new(functions, None).walk(&self.root).map(|r| r.1)
    }

    /// Everything the value depends on, as a set that iterates in a fixed
    /// order.
    ///
    /// A percentage that is not scaling a length depends on
    /// [`Dependency::PercentBasis`]. The root counts as needing a length. For
    /// an expression that doesn't type-check, the set is a conservative
    /// reading of its terms, so a cache never misses a dependency.
    pub fn dependencies(&self, functions: &FunctionRegistry) -> BTreeSet<Dependency> {
        let mut rw = Rewrite::new(functions, None);
        match rw.walk(&self.root) {
            Ok((node, Dim::Percentage)) => {
                rw.coerce(node);
                rw.deps
            }
            Ok(_) => rw.deps,
            Err(_) => {
                let mut deps = BTreeSet::new();
                structural_dependencies(&self.root, &mut deps);
                deps
            }
        }
    }

    /// Binds, substitutes and folds this expression as a length (see the
    /// module docs). The `Err` carries the notes that explain why not.
    pub fn compute(
        &self,
        scope: &Scope,
        functions: &FunctionRegistry,
    ) -> Result<ComputedLength, Vec<Note>> {
        let basis = match &scope.percent {
            Term::Bound(c) => Some(c.to_node()),
            _ => None,
        };
        let mut rw = Rewrite::new(functions, basis.as_ref());
        let (mut node, dim) = rw.walk(&self.root).map_err(|e| vec![e.note()])?;
        match dim {
            Dim::Length => {}
            Dim::Percentage => node = rw.coerce(node),
            other => {
                return Err(vec![
                    CheckError::Type(format!("expected a length, found a {other}")).note(),
                ]);
            }
        }
        if rw.unbound {
            return Err(vec![
                CheckError::Unresolved(
                    "a percentage has nothing to be a percentage of here".into(),
                )
                .note(),
            ]);
        }
        let node = substitute(node, scope).map_err(|e| vec![e.note()])?;
        let node = fold(node, functions);
        if let Some(l) = literal_length(&node) {
            return Ok(ComputedLength::Absolute(l));
        }
        Expr::new(node)
            .map(ComputedLength::Symbolic)
            .map_err(|e| vec![e.note()])
    }

    /// Evaluates the expression as written, against a context. Anything that
    /// can't be computed becomes zero of its dimension and marks the result
    /// `degraded`. Prefer [`Expr::compute`] followed by [`ComputedLength::used`]
    /// for lengths: this does not bind percentages.
    pub fn eval(&self, env: &UsedEnv<'_>) -> Evaluated {
        eval_root(&self.root, env)
    }
}

/// Checks the bounds and shape of a tree with an explicit stack, so a hostile
/// tree is rejected at the limit however deep it is.
fn validate(root: &Node) -> Result<(), ExprError> {
    let mut stack = vec![(root, 1usize)];
    let mut count = 0usize;
    while let Some((node, depth)) = stack.pop() {
        count += 1;
        if count > MAX_NODES {
            return Err(ExprError::Limit(Limit::Nodes));
        }
        if depth > MAX_DEPTH {
            return Err(ExprError::Limit(Limit::Depth));
        }
        match node {
            Node::Quantity(..) => {}
            Node::Basis(b) => match &b.scope {
                BasisScope::Named(n) if b.level != Level::Frame || n.len() > MAX_NAME => {
                    return Err(ExprError::Shape("only frames have names, and briefly"));
                }
                _ => {}
            },
            Node::Neg(x) => stack.push((x, depth + 1)),
            Node::Binary(_, a, b) => {
                stack.push((a, depth + 1));
                stack.push((b, depth + 1));
            }
            Node::Min(v) | Node::Max(v) => {
                if v.is_empty() {
                    return Err(ExprError::Shape("min and max need an argument"));
                }
                stack.extend(v.iter().map(|n| (n, depth + 1)));
            }
            Node::Clamp(a, b, c) => stack.extend([a, b, c].map(|n| (&**n, depth + 1))),
            Node::Call(name, args) => {
                if !valid_name(name) || RESERVED.contains(&name.as_str()) {
                    return Err(ExprError::Shape("a function name is lower-case words"));
                }
                if Basis::from_ident(name).is_some() {
                    return Err(ExprError::Shape("a function can't be named like a basis"));
                }
                stack.extend(args.iter().map(|n| (n, depth + 1)));
            }
        }
    }
    Ok(())
}

/// Takes a tree apart without recursing, so rejecting a hostile tree can't
/// overflow the stack in its destructor either.
fn dismantle(root: Node) {
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        match n {
            Node::Neg(x) => stack.push(*x),
            Node::Binary(_, a, b) => stack.extend([*a, *b]),
            Node::Min(v) | Node::Max(v) | Node::Call(_, v) => stack.extend(v),
            Node::Clamp(a, b, c) => stack.extend([*a, *b, *c]),
            Node::Quantity(..) | Node::Basis(_) => {}
        }
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = String::new();
        print(&self.root, &mut out);
        f.write_str(&out)
    }
}

impl Serialize for Expr {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Expr {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Expr::parse(&s).map_err(serde::de::Error::custom)
    }
}

pub(crate) fn valid_name(s: &str) -> bool {
    let mut chars = s.bytes();
    s.len() <= MAX_NAME
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

// ---------------------------------------------------------------------------
// Printing

fn print(n: &Node, out: &mut String) {
    match n {
        Node::Quantity(d, u) => {
            out.push_str(&d.to_string());
            out.push_str(u.suffix());
        }
        Node::Basis(b) => out.push_str(&b.to_string()),
        Node::Neg(x) => {
            out.push('-');
            if matches!(**x, Node::Binary(..) | Node::Neg(_)) {
                out.push('(');
                print(x, out);
                out.push(')');
            } else {
                print(x, out);
            }
        }
        Node::Binary(op, a, b) => {
            print_operand(a, op.prec(), false, out);
            out.push(' ');
            out.push_str(op.symbol());
            out.push(' ');
            print_operand(b, op.prec(), true, out);
        }
        Node::Min(v) => print_call("min", v, out),
        Node::Max(v) => print_call("max", v, out),
        Node::Clamp(a, b, c) => {
            out.push_str("clamp(");
            for (i, n) in [a, b, c].into_iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                print(n, out);
            }
            out.push(')');
        }
        Node::Call(name, v) => print_call(name, v, out),
    }
}

fn print_call(name: &str, args: &[Node], out: &mut String) {
    out.push_str(name);
    out.push('(');
    for (i, n) in args.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        print(n, out);
    }
    out.push(')');
}

/// Operators are left-associative: a right operand of equal precedence needs
/// parentheses, a left one doesn't.
fn print_operand(n: &Node, parent: u8, right: bool, out: &mut String) {
    let wrap = matches!(n, Node::Binary(op, ..) if if right { op.prec() <= parent } else { op.prec() < parent });
    if wrap {
        out.push('(');
        print(n, out);
        out.push(')');
    } else {
        print(n, out);
    }
}

// ---------------------------------------------------------------------------
// Parsing

struct Parser<'a> {
    src: &'a str,
    pos: usize,
    nodes: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.src.as_bytes().get(self.pos).copied()
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    fn syntax(&self, expected: &'static str) -> ExprError {
        ExprError::Syntax {
            at: self.pos,
            expected,
        }
    }

    /// Counts a node as soon as it exists, so the bound stops a hostile
    /// expression early.
    fn count(&mut self) -> Result<(), ExprError> {
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            Err(ExprError::Limit(Limit::Nodes))
        } else {
            Ok(())
        }
    }

    fn expect(&mut self, c: u8, what: &'static str) -> Result<(), ExprError> {
        self.skip_space();
        if self.peek() == Some(c) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.syntax(what))
        }
    }

    fn enter(depth: usize) -> Result<(), ExprError> {
        if depth > MAX_DEPTH {
            Err(ExprError::Limit(Limit::Depth))
        } else {
            Ok(())
        }
    }

    fn expr(&mut self, depth: usize) -> Result<Node, ExprError> {
        Self::enter(depth)?;
        let mut left = self.term(depth)?;
        loop {
            self.skip_space();
            let op = match self.peek() {
                Some(b'+') => Op::Add,
                Some(b'-') => Op::Sub,
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.term(depth)?;
            self.count()?;
            left = Node::binary(op, left, right);
        }
    }

    fn term(&mut self, depth: usize) -> Result<Node, ExprError> {
        let mut left = self.unary(depth)?;
        loop {
            self.skip_space();
            let op = match self.peek() {
                Some(b'*') => Op::Mul,
                Some(b'/') => Op::Div,
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.unary(depth)?;
            self.count()?;
            left = Node::binary(op, left, right);
        }
    }

    fn unary(&mut self, depth: usize) -> Result<Node, ExprError> {
        Self::enter(depth)?;
        self.skip_space();
        match self.peek() {
            Some(b'-') => {
                self.pos += 1;
                let inner = self.unary(depth + 1)?;
                self.count()?;
                Ok(Node::Neg(Box::new(inner)))
            }
            Some(b'+') => {
                self.pos += 1;
                self.unary(depth + 1)
            }
            _ => self.primary(depth),
        }
    }

    fn primary(&mut self, depth: usize) -> Result<Node, ExprError> {
        self.skip_space();
        match self.peek() {
            Some(b'(') => {
                self.pos += 1;
                let inner = self.expr(depth + 1)?;
                self.expect(b')', "a closing parenthesis")?;
                Ok(inner)
            }
            Some(c) if c.is_ascii_digit() || c == b'.' => self.quantity(),
            Some(c) if c.is_ascii_lowercase() => self.word(depth),
            _ => Err(self.syntax("a value")),
        }
    }

    fn quantity(&mut self) -> Result<Node, ExprError> {
        let start = self.pos;
        let mut mantissa = 0u64;
        let mut scale = 0usize;
        let mut digits = 0usize;
        let mut in_fraction = false;
        loop {
            match self.peek() {
                Some(c) if c.is_ascii_digit() => {
                    mantissa = mantissa
                        .checked_mul(10)
                        .and_then(|m| m.checked_add((c - b'0') as u64))
                        .ok_or(ExprError::Literal { at: start })?;
                    digits += 1;
                    if in_fraction {
                        scale += 1;
                    }
                }
                Some(b'.') if !in_fraction => in_fraction = true,
                _ => break,
            }
            self.pos += 1;
        }
        if digits == 0 || (in_fraction && scale == 0) {
            return Err(ExprError::Syntax {
                at: start,
                expected: "digits",
            });
        }
        let value = u8::try_from(scale)
            .ok()
            .and_then(|s| Decimal::new(mantissa, s))
            .ok_or(ExprError::Literal { at: start })?;
        let unit = if self.peek() == Some(b'%') {
            self.pos += 1;
            Unit::Percent
        } else {
            let from = self.pos;
            while self.peek().is_some_and(|c| c.is_ascii_lowercase()) {
                self.pos += 1;
            }
            match &self.src[from..self.pos] {
                "" => Unit::None,
                "pt" => Unit::Pt,
                "em" => Unit::Em,
                "lh" => Unit::Lh,
                _ => {
                    self.pos = from;
                    return Err(self.syntax("a unit: pt, em, lh or %"));
                }
            }
        };
        self.count()?;
        Ok(Node::Quantity(value, unit))
    }

    fn word(&mut self, depth: usize) -> Result<Node, ExprError> {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        {
            self.pos += 1;
        }
        let word = &self.src[start..self.pos];
        if word.len() > MAX_NAME {
            return Err(ExprError::Syntax {
                at: start,
                expected: "a shorter name",
            });
        }
        if let Some(basis) = Basis::from_ident(word) {
            return self.basis(basis, start);
        }
        if self.peek() != Some(b'(') {
            self.pos = start;
            return Err(self.syntax("a value: a number, a basis or a function call"));
        }
        self.pos += 1;
        let mut args = self.args(depth + 1)?;
        if word == "calc" {
            // A group, not a node.
            return match (args.pop(), args.is_empty()) {
                (Some(inner), true) => Ok(inner),
                _ => Err(self.syntax("one expression inside calc()")),
            };
        }
        self.count()?;
        match (word, args.len()) {
            ("min", 1..) => Ok(Node::Min(args)),
            ("max", 1..) => Ok(Node::Max(args)),
            ("min" | "max", _) => Err(self.syntax("at least one argument")),
            ("clamp", 3) => {
                let mut it = args.into_iter();
                match (it.next(), it.next(), it.next()) {
                    (Some(a), Some(b), Some(c)) => {
                        Ok(Node::Clamp(Box::new(a), Box::new(b), Box::new(c)))
                    }
                    _ => Err(self.syntax("three arguments")),
                }
            }
            ("clamp", _) => Err(self.syntax("clamp(lowest, value, highest)")),
            (name, _) => Ok(Node::Call(name.to_string(), args)),
        }
    }

    /// A comma-separated list up to the closing parenthesis.
    fn args(&mut self, depth: usize) -> Result<Vec<Node>, ExprError> {
        let mut args = Vec::new();
        self.skip_space();
        if self.peek() == Some(b')') {
            self.pos += 1;
            return Ok(args);
        }
        loop {
            args.push(self.expr(depth)?);
            self.skip_space();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b')') => {
                    self.pos += 1;
                    return Ok(args);
                }
                _ => return Err(self.syntax("a comma or a closing parenthesis")),
            }
        }
    }

    /// A basis, with the optional string argument that names a frame or
    /// (for `nearest-*`) the level to start from.
    fn basis(&mut self, mut basis: Basis, start: usize) -> Result<Node, ExprError> {
        let takes_argument = basis.scope == BasisScope::Nearest || basis.level == Level::Frame;
        if self.peek() == Some(b'(') {
            if !takes_argument {
                return Err(self.syntax("no argument: only frames have names"));
            }
            self.pos += 1;
            let name = self.string()?;
            self.expect(b')', "a closing parenthesis")?;
            if name.len() > MAX_NAME {
                self.pos = start;
                return Err(self.syntax("a shorter name"));
            }
            if basis.scope == BasisScope::Nearest {
                basis.level = Level::from_name(&name).ok_or_else(|| self.syntax("a level name"))?;
            } else {
                basis.scope = BasisScope::Named(name);
            }
        } else if basis.scope == BasisScope::Nearest {
            return Err(self.syntax("the level to start from, as in nearest-width(\"block\")"));
        }
        self.count()?;
        Ok(Node::Basis(basis))
    }

    fn string(&mut self) -> Result<String, ExprError> {
        self.skip_space();
        if self.peek() != Some(b'"') {
            return Err(self.syntax("a quoted name"));
        }
        self.pos += 1;
        let mut out = String::new();
        let mut chars = self.src[self.pos..].char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => {
                    self.pos += i + 1;
                    return Ok(out);
                }
                '\\' => match chars.next() {
                    Some((_, e @ ('"' | '\\'))) => out.push(e),
                    _ => return Err(self.syntax("\\\" or \\\\")),
                },
                c => out.push(c),
            }
            if out.len() > MAX_NAME {
                return Err(self.syntax("a shorter name"));
            }
        }
        Err(self.syntax("a closing quote"))
    }
}

// ---------------------------------------------------------------------------
// Type checking and percentage binding

struct Rewrite<'a> {
    functions: &'a FunctionRegistry,
    /// What a percentage that needs a length is a percentage of.
    basis: Option<&'a Node>,
    deps: BTreeSet<Dependency>,
    /// A percentage needed a basis and there was none.
    unbound: bool,
}

impl<'a> Rewrite<'a> {
    fn new(functions: &'a FunctionRegistry, basis: Option<&'a Node>) -> Self {
        Rewrite {
            functions,
            basis,
            deps: BTreeSet::new(),
            unbound: false,
        }
    }

    /// Turns a percentage into the length it is of the declared basis.
    fn coerce(&mut self, n: Node) -> Node {
        self.deps.insert(Dependency::PercentBasis);
        match self.basis {
            Some(b) => Node::binary(Op::Mul, n, b.clone()),
            None => {
                self.unbound = true;
                n
            }
        }
    }

    fn mismatch(op: &str, a: Dim, b: Dim) -> CheckError {
        CheckError::Type(format!("cannot {op} a {a} and a {b}"))
    }

    fn walk(&mut self, n: &Node) -> Result<(Node, Dim), CheckError> {
        match n {
            Node::Quantity(_, unit) => {
                match unit {
                    Unit::Em => {
                        self.deps.insert(Dependency::Em);
                    }
                    Unit::Lh => {
                        self.deps.insert(Dependency::Lh);
                    }
                    _ => {}
                }
                Ok((n.clone(), unit.dim()))
            }
            Node::Basis(b) => {
                self.deps.insert(Dependency::Basis(b.clone()));
                Ok((n.clone(), Dim::Length))
            }
            Node::Neg(x) => {
                let (x, d) = self.walk(x)?;
                if d == Dim::Ratio {
                    return Err(CheckError::Type("cannot negate a ratio".into()));
                }
                Ok((Node::Neg(Box::new(x)), d))
            }
            Node::Binary(op, a, b) => {
                let (a, ad) = self.walk(a)?;
                let (b, bd) = self.walk(b)?;
                self.binary(*op, a, ad, b, bd)
            }
            Node::Min(v) | Node::Max(v) => {
                let items = v.iter().map(|n| self.walk(n)).collect::<Result<_, _>>()?;
                let (args, dim) = self.unify(items)?;
                Ok((
                    if matches!(n, Node::Min(_)) {
                        Node::Min(args)
                    } else {
                        Node::Max(args)
                    },
                    dim,
                ))
            }
            Node::Clamp(a, b, c) => {
                let items = [a, b, c]
                    .into_iter()
                    .map(|n| self.walk(n))
                    .collect::<Result<_, _>>()?;
                let (args, dim) = self.unify(items)?;
                let mut it = args.into_iter();
                match (it.next(), it.next(), it.next()) {
                    (Some(a), Some(b), Some(c)) => {
                        Ok((Node::Clamp(Box::new(a), Box::new(b), Box::new(c)), dim))
                    }
                    _ => Err(CheckError::Type("clamp takes three arguments".into())),
                }
            }
            Node::Call(name, args) => {
                self.deps.insert(Dependency::Function(name.clone()));
                let f = self
                    .functions
                    .get(name)
                    .ok_or_else(|| CheckError::UnknownFunction(name.clone()))?;
                let sig = f.signature();
                let arity_ok = match sig.variadic {
                    None => args.len() == sig.params.len(),
                    Some(_) => args.len() >= sig.params.len(),
                };
                if !arity_ok {
                    return Err(CheckError::Type(format!(
                        "{name} takes {} arguments, not {}",
                        sig.params.len(),
                        args.len()
                    )));
                }
                let mut out = Vec::with_capacity(args.len());
                for (i, arg) in args.iter().enumerate() {
                    let (node, dim) = self.walk(arg)?;
                    let want = sig.params.get(i).copied().or(sig.variadic);
                    match want {
                        Some(w) if w == dim => out.push(node),
                        Some(Dim::Length) if dim == Dim::Percentage => out.push(self.coerce(node)),
                        Some(w) => {
                            return Err(CheckError::Type(format!(
                                "argument {} of {name} is a {w}, found a {dim}",
                                i + 1
                            )));
                        }
                        None => {
                            return Err(CheckError::Type(format!("too many arguments to {name}")));
                        }
                    }
                }
                Ok((Node::Call(name.clone(), out), sig.returns))
            }
        }
    }

    fn binary(
        &mut self,
        op: Op,
        a: Node,
        ad: Dim,
        b: Node,
        bd: Dim,
    ) -> Result<(Node, Dim), CheckError> {
        use Dim::*;
        let (mut a, mut b) = (a, b);
        let dim = match (op, ad, bd) {
            (Op::Add | Op::Sub, x, y) if x == y && x != Ratio => x,
            (Op::Add | Op::Sub, Length, Percentage) => {
                b = self.coerce(b);
                Length
            }
            (Op::Add | Op::Sub, Percentage, Length) => {
                a = self.coerce(a);
                Length
            }
            (Op::Mul, Number, Number) => Number,
            (Op::Mul, Length, Number) | (Op::Mul, Number, Length) => Length,
            (Op::Mul, Percentage, Number) | (Op::Mul, Number, Percentage) => Percentage,
            (Op::Mul, Percentage, Length) | (Op::Mul, Length, Percentage) => Length,
            (Op::Div, Number, Number) => Number,
            (Op::Div, Length, Number) => Length,
            (Op::Div, Percentage, Number) => Percentage,
            (Op::Div, Length, Length) => Number,
            (op, x, y) => {
                let verb = match op {
                    Op::Add => "add",
                    Op::Sub => "subtract",
                    Op::Mul => "multiply",
                    Op::Div => "divide",
                };
                return Err(Self::mismatch(verb, x, y));
            }
        };
        Ok((Node::binary(op, a, b), dim))
    }

    /// The common dimension of the arguments of `min`, `max` and `clamp`.
    fn unify(&mut self, items: Vec<(Node, Dim)>) -> Result<(Vec<Node>, Dim), CheckError> {
        let first = items.first().map_or(Dim::Length, |i| i.1);
        let all_same = items.iter().all(|i| i.1 == first);
        let lengths_and_percentages = items
            .iter()
            .all(|i| matches!(i.1, Dim::Length | Dim::Percentage));
        if first == Dim::Ratio || !(all_same || lengths_and_percentages) {
            let names: Vec<_> = items.iter().map(|i| i.1.to_string()).collect();
            return Err(CheckError::Type(format!(
                "cannot compare a {}",
                names.join(" with a ")
            )));
        }
        if all_same {
            return Ok((items.into_iter().map(|i| i.0).collect(), first));
        }
        let args = items
            .into_iter()
            .map(|(n, d)| {
                if d == Dim::Percentage {
                    self.coerce(n)
                } else {
                    n
                }
            })
            .collect();
        Ok((args, Dim::Length))
    }
}

fn structural_dependencies(n: &Node, deps: &mut BTreeSet<Dependency>) {
    match n {
        Node::Quantity(_, Unit::Em) => {
            deps.insert(Dependency::Em);
        }
        Node::Quantity(_, Unit::Lh) => {
            deps.insert(Dependency::Lh);
        }
        Node::Quantity(_, Unit::Percent) => {
            deps.insert(Dependency::PercentBasis);
        }
        Node::Quantity(..) => {}
        Node::Basis(b) => {
            deps.insert(Dependency::Basis(b.clone()));
        }
        Node::Neg(x) => structural_dependencies(x, deps),
        Node::Binary(_, a, b) => {
            structural_dependencies(a, deps);
            structural_dependencies(b, deps);
        }
        Node::Min(v) | Node::Max(v) => v.iter().for_each(|n| structural_dependencies(n, deps)),
        Node::Clamp(a, b, c) => [a, b, c]
            .into_iter()
            .for_each(|n| structural_dependencies(n, deps)),
        Node::Call(name, v) => {
            deps.insert(Dependency::Function(name.clone()));
            v.iter().for_each(|n| structural_dependencies(n, deps));
        }
    }
}

// ---------------------------------------------------------------------------
// Computed values

/// What a font term (`em`, `lh`) or the percentage basis is while computing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Term {
    /// Known: substitute it.
    Bound(ComputedLength),
    /// Not known yet: leave the term, to be supplied at the used stage
    /// ([`UsedEnv`]).
    Free,
    /// The term refers back to the property being computed (`style.cycle`).
    Cyclic,
    /// There is nothing for it to be (`style.basis-unresolved`).
    Unavailable,
}

/// What the terms of an expression refer to while it is computed (18).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    pub em: Term,
    pub lh: Term,
    /// What a percentage that needs a length is a percentage of.
    pub percent: Term,
}

impl Scope {
    /// A scope where the font terms and the percentage basis are all known
    /// lengths, as for the parameter of a relation. `percent: None` leaves a
    /// bare percentage unresolved.
    pub fn known(em: Length, lh: Length, percent: Option<Length>) -> Scope {
        Scope {
            em: Term::Bound(ComputedLength::Absolute(em)),
            lh: Term::Bound(ComputedLength::Absolute(lh)),
            percent: percent.map_or(Term::Unavailable, |l| {
                Term::Bound(ComputedLength::Absolute(l))
            }),
        }
    }
}

/// A length at the computed stage (08): a number where nothing depends on a
/// context, otherwise an expression that mentions only what the used stage
/// supplies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComputedLength {
    Absolute(Length),
    /// Thousandths of the element's own font size, resolved with
    /// [`Length::mul_ratio`] exactly as [`crate::LengthExpr::Em`] always was.
    EmRelative(i32),
    Symbolic(Expr),
}

impl ComputedLength {
    pub(crate) fn to_node(&self) -> Node {
        match self {
            ComputedLength::Absolute(l) => length_node(*l),
            ComputedLength::EmRelative(1000) => {
                Node::Quantity(Decimal::new(1, 0).unwrap_or(Decimal::ZERO), Unit::Em)
            }
            ComputedLength::EmRelative(p) => {
                let scaled = Node::binary(
                    Op::Mul,
                    Node::Quantity(
                        Decimal::new(p.unsigned_abs() as u64, 3).unwrap_or(Decimal::ZERO),
                        Unit::None,
                    ),
                    Node::Quantity(Decimal::new(1, 0).unwrap_or(Decimal::ZERO), Unit::Em),
                );
                if *p < 0 {
                    Node::Neg(Box::new(scaled))
                } else {
                    scaled
                }
            }
            ComputedLength::Symbolic(e) => e.root.clone(),
        }
    }

    /// What this value depends on. A symbolic value with no free terms
    /// depends only on its bases.
    pub fn dependencies(&self, functions: &FunctionRegistry) -> BTreeSet<Dependency> {
        match self {
            ComputedLength::Absolute(_) => BTreeSet::new(),
            ComputedLength::EmRelative(_) => BTreeSet::from([Dependency::Em]),
            ComputedLength::Symbolic(e) => e.dependencies(functions),
        }
    }

    /// The text form of the value, for explanations.
    pub fn describe(&self) -> String {
        match self {
            ComputedLength::EmRelative(_) | ComputedLength::Symbolic(_) => {
                let mut out = String::new();
                print(&self.to_node(), &mut out);
                out
            }
            ComputedLength::Absolute(l) => {
                let mut out = String::new();
                print(&length_node(*l), &mut out);
                out
            }
        }
    }

    /// Resolves the value against a context (08, 18).
    pub fn used(&self, env: &UsedEnv<'_>) -> UsedLength {
        match self {
            ComputedLength::Absolute(l) => UsedLength::clean(*l),
            ComputedLength::EmRelative(p) => match env.em {
                Some(em) => UsedLength::clean(em.mul_ratio(*p, 1000)),
                None => {
                    let mut acc = Acc::default();
                    acc.unresolved("em", "the font size");
                    acc.finish_length(Length::ZERO)
                }
            },
            ComputedLength::Symbolic(e) => {
                let ev = eval_root(&e.root, env);
                match ev.value {
                    Value::Length(l) => UsedLength {
                        value: l,
                        notes: ev.notes,
                        degraded: ev.degraded,
                        bases: ev.bases,
                    },
                    other => {
                        let mut notes = ev.notes;
                        notes.push(
                            CheckError::Type(format!("expected a length, found a {}", other.dim()))
                                .note(),
                        );
                        UsedLength {
                            value: Length::ZERO,
                            notes,
                            degraded: true,
                            bases: ev.bases,
                        }
                    }
                }
            }
        }
    }
}

/// A length at the used stage, and how it came about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsedLength {
    pub value: Length,
    pub notes: Vec<Note>,
    /// Some part could not be computed and counted as zero. A caller with a
    /// better value to use instead (a property's inherited value) should.
    pub degraded: bool,
    /// The bases that were read, as `frame-width("main") = 400pt`, sorted.
    pub bases: Vec<String>,
}

impl UsedLength {
    fn clean(value: Length) -> UsedLength {
        UsedLength {
            value,
            notes: Vec::new(),
            degraded: false,
            bases: Vec::new(),
        }
    }
}

/// What the used stage may read.
pub struct UsedEnv<'a> {
    pub functions: &'a FunctionRegistry,
    pub context: &'a ResolutionContext,
    /// The element's font size, for an `em` the computed stage left free.
    pub em: Option<Length>,
    /// The element's line height, for an `lh` the computed stage left free.
    pub lh: Option<Length>,
}

/// Replaces `em` and `lh` as the scope says.
fn substitute(n: Node, scope: &Scope) -> Result<Node, CheckError> {
    fn term(
        t: &Term,
        d: Decimal,
        unit: Unit,
        what: &str,
        original: Node,
    ) -> Result<Node, CheckError> {
        match t {
            Term::Bound(c) => {
                let value = c.to_node();
                if d == Decimal::new(1, 0).unwrap_or(Decimal::ZERO) {
                    Ok(value)
                } else {
                    Ok(Node::binary(Op::Mul, Node::Quantity(d, Unit::None), value))
                }
            }
            Term::Free => Ok(original),
            Term::Cyclic => Err(CheckError::Cycle(format!(
                "{what} depends on the value being computed"
            ))),
            Term::Unavailable => Err(CheckError::Unresolved(format!(
                "there is no {what} to use for {}",
                unit.suffix()
            ))),
        }
    }
    Ok(match n {
        Node::Quantity(d, Unit::Em) => term(
            &scope.em,
            d,
            Unit::Em,
            "font size",
            Node::Quantity(d, Unit::Em),
        )?,
        Node::Quantity(d, Unit::Lh) => term(
            &scope.lh,
            d,
            Unit::Lh,
            "line height",
            Node::Quantity(d, Unit::Lh),
        )?,
        Node::Neg(x) => Node::Neg(Box::new(substitute(*x, scope)?)),
        Node::Binary(op, a, b) => Node::binary(op, substitute(*a, scope)?, substitute(*b, scope)?),
        Node::Min(v) => Node::Min(subst_all(v, scope)?),
        Node::Max(v) => Node::Max(subst_all(v, scope)?),
        Node::Clamp(a, b, c) => Node::Clamp(
            Box::new(substitute(*a, scope)?),
            Box::new(substitute(*b, scope)?),
            Box::new(substitute(*c, scope)?),
        ),
        Node::Call(name, v) => Node::Call(name, subst_all(v, scope)?),
        leaf => leaf,
    })
}

fn subst_all(v: Vec<Node>, scope: &Scope) -> Result<Vec<Node>, CheckError> {
    v.into_iter().map(|n| substitute(n, scope)).collect()
}

fn is_constant(n: &Node) -> bool {
    match n {
        Node::Quantity(_, u) => !matches!(u, Unit::Em | Unit::Lh),
        Node::Basis(_) => false,
        Node::Neg(x) => is_constant(x),
        Node::Binary(_, a, b) => is_constant(a) && is_constant(b),
        Node::Min(v) | Node::Max(v) | Node::Call(_, v) => v.iter().all(is_constant),
        Node::Clamp(a, b, c) => is_constant(a) && is_constant(b) && is_constant(c),
    }
}

/// Replaces every maximal constant length subexpression with its value. A
/// subexpression whose evaluation reports anything (saturation, division by
/// zero, a failing function) is left alone, so the used stage reports it.
fn fold(n: Node, functions: &FunctionRegistry) -> Node {
    if is_constant(&n) {
        if matches!(n, Node::Quantity(_, Unit::Pt)) {
            return n;
        }
        let context = ResolutionContext::default();
        let env = UsedEnv {
            functions,
            context: &context,
            em: None,
            lh: None,
        };
        let ev = eval_root(&n, &env);
        if let (Value::Length(l), true, false) = (ev.value, ev.notes.is_empty(), ev.degraded) {
            return length_node(l);
        }
        return n;
    }
    match n {
        Node::Neg(x) => Node::Neg(Box::new(fold(*x, functions))),
        Node::Binary(op, a, b) => Node::binary(op, fold(*a, functions), fold(*b, functions)),
        Node::Min(v) => Node::Min(fold_all(v, functions)),
        Node::Max(v) => Node::Max(fold_all(v, functions)),
        Node::Clamp(a, b, c) => Node::Clamp(
            Box::new(fold(*a, functions)),
            Box::new(fold(*b, functions)),
            Box::new(fold(*c, functions)),
        ),
        Node::Call(name, v) => Node::Call(name, fold_all(v, functions)),
        leaf => leaf,
    }
}

fn fold_all(v: Vec<Node>, functions: &FunctionRegistry) -> Vec<Node> {
    v.into_iter().map(|n| fold(n, functions)).collect()
}

// ---------------------------------------------------------------------------
// Evaluation

/// The result of [`Expr::eval`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evaluated {
    pub value: Value,
    pub notes: Vec<Note>,
    pub degraded: bool,
    pub bases: Vec<String>,
}

/// What evaluation collects on the way.
#[derive(Default)]
struct Acc {
    notes: Vec<Note>,
    degraded: bool,
    saturated: bool,
    divided_by_zero: bool,
    bases: Vec<String>,
}

impl Acc {
    fn note(&mut self, note: Note) {
        if !self.notes.iter().any(|n| n == &note) {
            self.notes.push(note);
        }
    }

    fn degrade(&mut self, note: Note) {
        self.degraded = true;
        self.note(note);
    }

    fn unresolved(&mut self, what: &str, of: &str) {
        self.degrade(Note::warning(
            codes::STYLE_BASIS_UNRESOLVED,
            format!("{what} has no value here: there is no {of}; counted as zero"),
        ));
    }

    /// Narrows to `i32`, noting if it had to saturate.
    fn sat(&mut self, v: i128) -> i32 {
        match i32::try_from(v) {
            Ok(v) => v,
            Err(_) => {
                self.saturated = true;
                if v < 0 { i32::MIN } else { i32::MAX }
            }
        }
    }

    /// `num / den` rounded half away from zero and narrowed. A zero
    /// denominator saturates by the sign of the numerator (19).
    fn divide(&mut self, num: i64, den: i64) -> i32 {
        if den == 0 {
            self.divided_by_zero = true;
            return match num.signum() {
                1 => i32::MAX,
                -1 => i32::MIN,
                _ => 0,
            };
        }
        let q = div_round(num, den);
        self.sat(q as i128)
    }

    fn finish(mut self) -> (Vec<Note>, bool, Vec<String>) {
        if self.saturated {
            self.notes.push(Note::warning(
                codes::STYLE_SATURATED,
                "arithmetic saturated at the limits of its range",
            ));
        }
        if self.divided_by_zero {
            self.notes.push(Note::warning(
                codes::STYLE_DIVIDE_BY_ZERO,
                "division by zero saturates by the sign of the numerator",
            ));
        }
        self.bases.sort();
        self.bases.dedup();
        let degraded = self.degraded;
        (self.notes, degraded, self.bases)
    }

    fn finish_length(self, value: Length) -> UsedLength {
        let (notes, degraded, bases) = self.finish();
        UsedLength {
            value,
            notes,
            degraded,
            bases,
        }
    }
}

fn eval_root(root: &Node, env: &UsedEnv<'_>) -> Evaluated {
    let mut acc = Acc::default();
    let value = eval(root, env, &mut acc);
    let (notes, degraded, bases) = acc.finish();
    Evaluated {
        value,
        notes,
        degraded,
        bases,
    }
}

fn mismatch(acc: &mut Acc, what: &str) -> Value {
    acc.degrade(Note::warning(
        codes::STYLE_TYPE,
        format!("{what}; counted as zero"),
    ));
    Value::Length(Length::ZERO)
}

fn eval(n: &Node, env: &UsedEnv<'_>, acc: &mut Acc) -> Value {
    match n {
        Node::Quantity(d, unit) => match unit {
            Unit::None => Value::Number(Fixed(acc.sat(d.times(65536)))),
            Unit::Percent => Value::Percentage(Fixed(acc.sat(d.times(65536)))),
            Unit::Pt => Value::Length(Length(acc.sat(d.times(1024)))),
            Unit::Em | Unit::Lh => {
                let (known, what, of) = if *unit == Unit::Em {
                    (env.em, "em", "font size")
                } else {
                    (env.lh, "lh", "line height")
                };
                match known {
                    Some(l) => Value::Length(Length(acc.sat(d.times(l.0 as i128)))),
                    None => {
                        acc.unresolved(what, of);
                        Value::Length(Length::ZERO)
                    }
                }
            }
        },
        Node::Basis(b) => match env.context.basis(b) {
            Resolved::Definite(l) => {
                acc.bases.push(format!("{b} = {l:?}"));
                Value::Length(l)
            }
            Resolved::Indefinite => {
                acc.degrade(Note::warning(
                    codes::STYLE_BASIS_INDEFINITE,
                    format!("{b} depends on its own content here; counted as zero"),
                ));
                Value::Length(Length::ZERO)
            }
            Resolved::Unresolved => {
                acc.degrade(Note::warning(
                    codes::STYLE_BASIS_UNRESOLVED,
                    format!("{b} does not exist in this context; counted as zero"),
                ));
                Value::Length(Length::ZERO)
            }
        },
        Node::Neg(x) => match eval(x, env, acc) {
            Value::Length(l) => Value::Length(Length(acc.sat(-(l.0 as i128)))),
            Value::Number(v) => Value::Number(Fixed(acc.sat(-(v.0 as i128)))),
            Value::Percentage(v) => Value::Percentage(Fixed(acc.sat(-(v.0 as i128)))),
            Value::Ratio { .. } => mismatch(acc, "cannot negate a ratio"),
        },
        Node::Binary(op, a, b) => {
            let a = eval(a, env, acc);
            let b = eval(b, env, acc);
            binary(*op, a, b, acc)
        }
        Node::Min(v) => extremum(v, env, acc, i32::min),
        Node::Max(v) => extremum(v, env, acc, i32::max),
        Node::Clamp(lo, v, hi) => {
            let lo = eval(lo, env, acc);
            let v = eval(v, env, acc);
            let hi = eval(hi, env, acc);
            match (lo, v, hi) {
                (Value::Length(lo), Value::Length(v), Value::Length(hi)) => {
                    Value::Length(Length(lo.0.max(v.0.min(hi.0))))
                }
                (Value::Number(lo), Value::Number(v), Value::Number(hi)) => {
                    Value::Number(Fixed(lo.0.max(v.0.min(hi.0))))
                }
                (Value::Percentage(lo), Value::Percentage(v), Value::Percentage(hi)) => {
                    Value::Percentage(Fixed(lo.0.max(v.0.min(hi.0))))
                }
                _ => mismatch(acc, "clamp needs three values of one dimension"),
            }
        }
        Node::Call(name, args) => {
            let values: Vec<Value> = args.iter().map(|a| eval(a, env, acc)).collect();
            let Some(f) = env.functions.get(name) else {
                acc.degrade(CheckError::UnknownFunction(name.clone()).note());
                return Value::Length(Length::ZERO);
            };
            let returns = f.signature().returns;
            match f.call(&values) {
                Ok(v) if v.dim() == returns => v,
                Ok(v) => {
                    acc.degrade(Note::warning(
                        codes::STYLE_FUNCTION_FAILED,
                        format!(
                            "{name} returned a {} but is declared to return a {returns}",
                            v.dim()
                        ),
                    ));
                    Value::zero(returns)
                }
                Err(e) => {
                    acc.degrade(Note::warning(
                        codes::STYLE_FUNCTION_FAILED,
                        format!("{name} failed: {e}"),
                    ));
                    Value::zero(returns)
                }
            }
        }
    }
}

/// `min` or `max` of values of one dimension.
fn extremum(v: &[Node], env: &UsedEnv<'_>, acc: &mut Acc, pick: fn(i32, i32) -> i32) -> Value {
    let values: Vec<Value> = v.iter().map(|n| eval(n, env, acc)).collect();
    let raw = |v: &Value| match v {
        Value::Length(l) => Some(l.0),
        Value::Number(f) | Value::Percentage(f) => Some(f.0),
        Value::Ratio { .. } => None,
    };
    let Some(first) = values.first() else {
        return mismatch(acc, "min and max need an argument");
    };
    let dim = first.dim();
    let mut best = raw(first);
    for value in &values {
        best = match (best, raw(value)) {
            (Some(a), Some(b)) if value.dim() == dim => Some(pick(a, b)),
            _ => None,
        };
    }
    match (best, dim) {
        (Some(b), Dim::Length) => Value::Length(Length(b)),
        (Some(b), Dim::Number) => Value::Number(Fixed(b)),
        (Some(b), Dim::Percentage) => Value::Percentage(Fixed(b)),
        _ => mismatch(acc, "min and max need values of one dimension"),
    }
}

fn binary(op: Op, a: Value, b: Value, acc: &mut Acc) -> Value {
    use Value::*;
    match (op, a, b) {
        (Op::Add, Length(x), Length(y)) => Length(self::Length(acc.sat(x.0 as i128 + y.0 as i128))),
        (Op::Sub, Length(x), Length(y)) => Length(self::Length(acc.sat(x.0 as i128 - y.0 as i128))),
        (Op::Add, Number(x), Number(y)) => Number(Fixed(acc.sat(x.0 as i128 + y.0 as i128))),
        (Op::Sub, Number(x), Number(y)) => Number(Fixed(acc.sat(x.0 as i128 - y.0 as i128))),
        (Op::Add, Percentage(x), Percentage(y)) => {
            Percentage(Fixed(acc.sat(x.0 as i128 + y.0 as i128)))
        }
        (Op::Sub, Percentage(x), Percentage(y)) => {
            Percentage(Fixed(acc.sat(x.0 as i128 - y.0 as i128)))
        }
        (Op::Mul, Number(x), Number(y)) => {
            Number(Fixed(acc.divide(x.0 as i64 * y.0 as i64, 1 << 16)))
        }
        (Op::Mul, Length(l), Number(n)) | (Op::Mul, Number(n), Length(l)) => {
            Length(self::Length(acc.divide(l.0 as i64 * n.0 as i64, 1 << 16)))
        }
        (Op::Mul, Percentage(p), Number(n)) | (Op::Mul, Number(n), Percentage(p)) => {
            Percentage(Fixed(acc.divide(p.0 as i64 * n.0 as i64, 1 << 16)))
        }
        (Op::Mul, Percentage(p), Length(l)) | (Op::Mul, Length(l), Percentage(p)) => Length(
            self::Length(acc.divide(l.0 as i64 * p.0 as i64, (1 << 16) * 100)),
        ),
        (Op::Div, Length(l), Number(n)) => {
            Length(self::Length(acc.divide((l.0 as i64) << 16, n.0 as i64)))
        }
        (Op::Div, Number(x), Number(y)) => {
            Number(Fixed(acc.divide((x.0 as i64) << 16, y.0 as i64)))
        }
        (Op::Div, Percentage(x), Number(y)) => {
            Percentage(Fixed(acc.divide((x.0 as i64) << 16, y.0 as i64)))
        }
        (Op::Div, Length(x), Length(y)) => {
            Number(Fixed(acc.divide((x.0 as i64) << 16, y.0 as i64)))
        }
        (op, x, y) => mismatch(
            acc,
            &format!(
                "cannot apply {} to a {} and a {}",
                op.symbol(),
                x.dim(),
                y.dim()
            ),
        ),
    }
}

#[cfg(test)]
mod tests;
