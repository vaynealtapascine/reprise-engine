//! Styles (decisions 08, 17 and 18): named styles with inheritance, direct
//! overrides, and the values they resolve to.
//!
//! # The four value stages (08)
//!
//! A block's style goes through four stages. Each is a type, so a caller can
//! stop at any of them, and [`StyleResolution::stages`] explains all four for
//! every property (39).
//!
//! 1.  **Specified** ([`Specified`]): the layers as authored, outermost first:
//!     the engine defaults, the named-style chain (parents first), then the
//!     block's direct overrides. Problems in the chain itself are reported here.
//! 2.  **Inherited**: for each property, the value the previous layer left.
//!     `em` in `size` means the inherited size, so a style that says `1.5em`
//!     is always 1.5 times what its parent said.
//! 3.  **Computed** ([`Computed`]): each layer's value with everything that
//!     does not depend on a context worked out. A length with nothing symbolic
//!     in it is a number ([`ComputedLength::Absolute`]); one that mentions a
//!     frame, page or medium stays an expression, with its inherited `em`s
//!     already substituted. Computing needs no [`ResolutionContext`], so it can
//!     be cached per style and reused for every frame the block may land in.
//! 4.  **Used** ([`Computed::used`]): the computed values resolved against a
//!     [`ResolutionContext`] and clamped to be non-negative (they are what
//!     layout uses).
//!
//! # What `em`, `lh` and `%` mean
//!
//! -   **`em`** is the element's font size, except in `size` itself, where it is
//!     the *inherited* font size (otherwise a font size would be defined in
//!     terms of itself).
//! -   **`lh`** is the element's used line height. Neither `size` nor
//!     `line-height` may use it: `line-height` is resolved against the font
//!     size and `size` would then depend on it, and `line-height` in `lh` is
//!     its own definition. Both are a cycle: the layer's value is ignored, the
//!     property keeps its inherited value, and `style.cycle` is reported. `lh`
//!     is meant for properties that do not feed the line height, such as the
//!     parameters of relations.
//! -   **`%`** is a percentage of the property's *declared basis*. For `size`
//!     that is the inherited font size; for `line-height`, the element's font
//!     size. An expression can name any other basis explicitly by multiplying:
//!     `50% * frame-width("main")`. See [`crate::context`] for the references.
//!
//! # When a basis is unresolved or indefinite
//!
//! A reference to something that is not there (`frame-width("nowhere")`, or
//! any frame, page or medium in the default context) or whose size depends on
//! its content (the height of an auto-height frame) counts as zero while the
//! expression is evaluated, and the expression is *degraded*. A layer whose
//! value is degraded is skipped: the property keeps the value it inherited from
//! the layer before, as if the layer had not set it, and `explain` names the
//! layer that actually supplied the value. The note is `style.basis-unresolved`
//! or `style.basis-indefinite`, a `Warning`: the output differs from what the
//! author asked for, but every block is still laid out.
//!
//! The same rule covers every other way a layer's value can fail to be
//! computed: a type error, an unknown function, an unparsable stored value, an
//! expression over the limits, a cycle. Arithmetic that saturates, or divides by
//! zero, still produces its saturated value and a warning.
//!
//! # Where values are stored
//!
//! Each property is one string in the style's Loro map, in one of these forms:
//!
//! | Stored | Meaning |
//! | --- | --- |
//! | `pt:10240`, `em:1200` | a [`LengthExpr`], the original form |
//! | `expr1:<text>` | an expression (see [`crate::expr`]); the `1` versions the text form |
//! | anything else | kept as it is, and reported as `style.unparsed` |
//!
//! A value this engine can't read is never dropped: it is read as
//! [`Authored::Unparsed`] and written back exactly as it was, so a document
//! from a newer engine survives being opened and saved by an older one (34).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use loro::{LoroMap, LoroValue};
use reprise_diag::Note;
use reprise_geom::Length;
use serde::{Deserialize, Serialize};

use crate::codes;
use crate::context::ResolutionContext;
use crate::expr::{
    ComputedLength, Decimal, Dependency, Expr, ExprError, Node, Scope, Term, Unit, UsedEnv,
    length_node,
};
use crate::function::FunctionRegistry;
use crate::{Block, Document, get_str};

/// A length as authored: a literal or relative to the font size (17).
///
/// This is the original, simple form. It stays `Copy` and keeps the meaning it
/// always had; richer values are an [`Expr`], stored beside it in a [`Style`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LengthExpr {
    Pt(Length),
    /// Thousandths of an em.
    Em(i32),
}

impl LengthExpr {
    pub fn resolve(self, em: Length) -> Length {
        match self {
            LengthExpr::Pt(l) => l,
            LengthExpr::Em(permille) => em.mul_ratio(permille, 1000),
        }
    }

    /// The same length as an expression. It evaluates to the same value
    /// unless the literal has to saturate.
    pub fn to_expr(self) -> Expr {
        let node = match self {
            LengthExpr::Pt(l) => length_node(l),
            LengthExpr::Em(p) => {
                let lit = Node::Quantity(
                    Decimal::new(p.unsigned_abs() as u64, 3).unwrap_or(Decimal::ZERO),
                    Unit::Em,
                );
                if p < 0 { Node::Neg(Box::new(lit)) } else { lit }
            }
        };
        Expr::trusted(node)
    }

    fn to_loro(self) -> LoroValue {
        match self {
            LengthExpr::Pt(l) => format!("pt:{}", l.0).into(),
            LengthExpr::Em(p) => format!("em:{p}").into(),
        }
    }

    fn from_loro(v: &str) -> Option<LengthExpr> {
        let (unit, n) = v.split_once(':')?;
        let n: i32 = n.parse().ok()?;
        match unit {
            "pt" => Some(LengthExpr::Pt(Length(n))),
            "em" => Some(LengthExpr::Em(n)),
            _ => None,
        }
    }
}

/// A style property that holds a length.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Property {
    Size,
    LineHeight,
}

impl Property {
    pub const ALL: [Property; 2] = [Property::Size, Property::LineHeight];

    /// The name used in the document, in `explain` and in diagnostics.
    pub fn name(self) -> &'static str {
        match self {
            Property::Size => "size",
            Property::LineHeight => "line-height",
        }
    }

    /// What a percentage is of, if the expression names no other basis.
    pub fn percent_basis(self) -> &'static str {
        match self {
            Property::Size => "the inherited font size",
            Property::LineHeight => "the element's font size",
        }
    }

    /// The name of `em` in this property, for explanations.
    fn font_term(self) -> &'static str {
        match self {
            Property::Size => "inherited font size",
            Property::LineHeight => "font size",
        }
    }

    /// What the terms of an expression refer to when a layer is taken on its
    /// own: `em` is left for the used stage, which supplies the inherited size
    /// for `size` and the final size for `line-height`, and a percentage is of
    /// that same size.
    fn own_scope(self) -> Scope {
        Scope {
            em: Term::Free,
            lh: Term::Cyclic,
            percent: Term::Bound(ComputedLength::EmRelative(1000)),
        }
    }
}

/// One property's value as it is stored (see the module docs).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Authored {
    Legacy(LengthExpr),
    Expr(Expr),
    /// Stored text this engine can't read. Kept verbatim.
    Unparsed(String),
}

/// The stored prefix of an expression. The number is the version of the text
/// form; an engine that doesn't know a version keeps the value unread.
const EXPR_PREFIX: &str = "expr1:";

impl Authored {
    /// What is written to the document.
    pub fn to_stored(&self) -> String {
        match self {
            Authored::Legacy(l) => match l.to_loro() {
                LoroValue::String(s) => s.to_string(),
                _ => String::new(),
            },
            Authored::Expr(e) => format!("{EXPR_PREFIX}{e}"),
            Authored::Unparsed(t) => t.clone(),
        }
    }

    /// Reads a stored string. Never fails: text that can't be read is kept as
    /// [`Authored::Unparsed`].
    pub fn from_stored(text: &str) -> Authored {
        if let Some(l) = LengthExpr::from_loro(text) {
            return Authored::Legacy(l);
        }
        if let Some(body) = text.strip_prefix(EXPR_PREFIX)
            && let Ok(e) = Expr::parse(body)
        {
            return Authored::Expr(e);
        }
        Authored::Unparsed(text.into())
    }

    /// For [`Authored::Unparsed`], why it can't be read.
    pub fn problem(&self) -> Option<ExprError> {
        let Authored::Unparsed(text) = self else {
            return None;
        };
        match text.strip_prefix(EXPR_PREFIX) {
            Some(body) => Expr::parse(body).err(),
            None => Some(ExprError::Shape(
                "not a length this engine knows (a newer format, or damaged)",
            )),
        }
    }

    /// The value in the expression syntax, or the raw text if unreadable.
    pub fn describe(&self) -> String {
        match self {
            Authored::Legacy(l) => l.to_expr().to_string(),
            Authored::Expr(e) => e.to_string(),
            Authored::Unparsed(t) => t.clone(),
        }
    }
}

/// Style properties as authored, in a named style or as direct overrides.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Style {
    pub parent: Option<String>,
    pub family: Option<String>,
    /// Explicit CSS-like chain. This replaces `family` in the same layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub families: Option<Vec<String>>,
    /// Unknown versioned chain text is preserved and reported, never overwritten.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unparsed_families: Option<String>,
    pub size: Option<LengthExpr>,
    pub line_height: Option<LengthExpr>,
    /// Values that are not a plain [`LengthExpr`]: expressions, and stored text
    /// this engine can't read. An entry here replaces the simple field for the
    /// same property in the same style. [`Style::set`] keeps the two apart.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<Property, Authored>,
}

impl Style {
    /// The authored value of a property, wherever it is held.
    pub fn get(&self, p: Property) -> Option<Authored> {
        self.values.get(&p).cloned().or_else(|| {
            let simple = match p {
                Property::Size => self.size,
                Property::LineHeight => self.line_height,
            };
            simple.map(Authored::Legacy)
        })
    }

    /// Sets a property, in the simple field if it fits there.
    pub fn set(&mut self, p: Property, value: Authored) {
        let slot = match p {
            Property::Size => &mut self.size,
            Property::LineHeight => &mut self.line_height,
        };
        match value {
            Authored::Legacy(l) => {
                *slot = Some(l);
                self.values.remove(&p);
            }
            other => {
                *slot = None;
                self.values.insert(p, other);
            }
        }
    }

    /// Sets a property to the expression written as `text`.
    pub fn with_expr(mut self, p: Property, text: &str) -> Result<Style, ExprError> {
        self.set(p, Authored::Expr(Expr::parse(text)?));
        Ok(self)
    }

    pub(crate) fn write(&self, map: &LoroMap) -> loro::LoroResult<()> {
        if let Some(p) = &self.parent {
            map.insert("parent", p.as_str())?;
        }
        if let Some(f) = &self.family {
            map.insert("family", f.as_str())?;
        }
        if let Some(raw) = &self.unparsed_families {
            map.insert("families", raw.as_str())?;
        } else if let Some(chain) = &self.families {
            // JSON encoding Vec<String> is infallible; no document-dependent unwrap.
            if let Ok(encoded) = serde_json::to_string(chain) {
                map.insert("families", format!("families1:{encoded}").as_str())?;
            }
        }
        for p in Property::ALL {
            if let Some(v) = self.get(p) {
                map.insert(p.name(), v.to_stored().as_str())?;
            }
        }
        Ok(())
    }

    pub(crate) fn read(map: &LoroMap) -> Style {
        let mut style = Style {
            parent: get_str(map, "parent"),
            family: get_str(map, "family"),
            ..Style::default()
        };
        if let Some(raw) = get_str(map, "families") {
            match raw
                .strip_prefix("families1:")
                .and_then(|json| serde_json::from_str(json).ok())
            {
                Some(chain) => style.families = Some(chain),
                None => style.unparsed_families = Some(raw),
            }
        }
        for p in Property::ALL {
            if let Some(text) = get_str(map, p.name()) {
                style.set(p, Authored::from_stored(&text));
            }
        }
        style
    }
}

/// Used values after inheritance and overrides (08), with where each came from (39).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputedStyle {
    pub family: String,
    /// Empty for legacy single-family documents, preserving their stored/output form.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub families: Vec<String>,
    pub size: Length,
    pub line_height: Length,
    pub explain: BTreeMap<String, String>,
    /// Properties whose resolved value was negative and was clamped to zero.
    /// Layout reports each one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clamped: Vec<String>,
    /// For each property whose value was relative to a percentage basis or a
    /// frame, page or medium: what it was resolved against (18). Empty for
    /// styles that use only `pt` and `em`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bases: BTreeMap<String, String>,
    /// What went wrong resolving the style, as `style.*` notes. Layout
    /// publishes them with the block as their subject.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
}

/// Engine defaults, the bottom of every style chain.
pub fn default_style() -> Style {
    Style {
        parent: None,
        families: None,
        unparsed_families: None,
        family: Some("Source Serif Pro".into()),
        size: Some(LengthExpr::Pt(Length::from_pt(10))),
        line_height: Some(LengthExpr::Em(1200)),
        values: BTreeMap::new(),
    }
}

// ---------------------------------------------------------------------------
// Stage 1: specified

/// One layer of a block's style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer {
    /// `default`, `style <name>` or `direct`, as `explain` names it.
    pub name: String,
    pub style: Style,
}

/// The longest named-style chain followed.
const MAX_CHAIN: usize = 32;

/// Stage 1 (08): a block's layers, outermost first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Specified {
    pub layers: Vec<Layer>,
    /// Problems with the chain itself: a missing parent, a cycle, a chain over
    /// the limit. The chain is cut where the problem is, and keeps what it has.
    pub notes: Vec<Note>,
}

pub(crate) fn specify(doc: &Document, block: &Block) -> Specified {
    let mut notes = Vec::new();
    let mut chain: Vec<(String, Style)> = Vec::new();
    let mut next = block.style.clone();
    let mut from = "the block".to_string();
    while let Some(name) = next.take() {
        if chain.iter().any(|(n, _)| n == &name) {
            notes.push(Note::warning(
                codes::STYLE_PARENT_CYCLE,
                format!(
                    "style {name} is its own ancestor (reached from {from}); \
                     the chain stops before it repeats"
                ),
            ));
            break;
        }
        if chain.len() > MAX_CHAIN {
            notes.push(Note::warning(
                codes::STYLE_CHAIN_TOO_LONG,
                format!(
                    "more than {MAX_CHAIN} styles in one chain; \
                     {from} refers to {name}, which is ignored"
                ),
            ));
            break;
        }
        let Some(style) = doc.style(&name) else {
            notes.push(Note::warning(
                codes::STYLE_PARENT_MISSING,
                format!("{from} refers to style {name}, which is not defined"),
            ));
            break;
        };
        next = style.parent.clone();
        from = format!("style {name}");
        chain.push((name, style));
    }
    let mut layers = vec![Layer {
        name: "default".into(),
        style: default_style(),
    }];
    layers.extend(chain.into_iter().rev().map(|(n, s)| Layer {
        name: format!("style {n}"),
        style: s,
    }));
    layers.push(Layer {
        name: "direct".into(),
        style: block.overrides.clone(),
    });
    Specified { layers, notes }
}

// ---------------------------------------------------------------------------
// Stages 2 and 3: inherited and computed

/// One layer's contribution to a property.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub layer: String,
    /// The value as authored, in the expression syntax.
    pub specified: String,
    /// The value inherited from the layer before, or empty for the first.
    pub inherited: String,
    /// The computed value (08). For `size` it has the inherited size worked
    /// in, so a style made of `pt` and `em` is already a number here.
    pub value: ComputedLength,
    /// The layer's value on its own, relative to the font size the used stage
    /// supplies: the previous layers' for `size`, the final one for
    /// `line-height`. This is what the used stage evaluates, so that a layer
    /// that has to be skipped leaves the layers after it applying to the value
    /// that survived.
    own: ComputedLength,
    /// What a percentage in the value is of, if it has one.
    pub basis: Option<String>,
    font_term: &'static str,
}

/// Stage 3 (08) for one property: the steps of the layers that set it,
/// outermost first. The last step is the winner; the ones before it are what
/// it falls back to if its value can't be used.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PropertyChain {
    pub steps: Vec<Step>,
}

/// Stage 3 (08): the style with everything context-free worked out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Computed {
    pub family: String,
    pub families: Vec<String>,
    family_layer: Option<String>,
    pub size: PropertyChain,
    pub line_height: PropertyChain,
    /// Problems found so far: the chain's, and any layer's value that could not
    /// be computed. A layer that failed is not in the chain.
    pub notes: Vec<Note>,
}

impl Specified {
    /// Stages 2 and 3: inherit and compute each property through the layers.
    pub fn compute(&self, functions: &FunctionRegistry) -> Computed {
        let mut notes = self.notes.clone();
        let mut family = String::new();
        let mut families = Vec::new();
        let mut family_layer = None;
        for layer in &self.layers {
            if let Some(f) = &layer.style.family {
                family = f.clone();
                families.clear();
                family_layer = Some(layer.name.clone());
            }
            if let Some(raw) = &layer.style.unparsed_families {
                notes.push(Note::warning(
                    crate::codes::STYLE_UNPARSED,
                    format!("unreadable font chain in {}: {raw}", layer.name),
                ));
            } else if let Some(chain) = &layer.style.families {
                families = chain.clone();
                if let Some(first) = chain.first() {
                    family = first.clone();
                }
                family_layer = Some(layer.name.clone());
                // Even an empty explicit chain opts into generic serif fallback.
                if families.is_empty() {
                    families.push("serif".into());
                }
            }
        }
        let mut chain = |p: Property| compute_property(p, &self.layers, functions, &mut notes);
        let size = chain(Property::Size);
        let line_height = chain(Property::LineHeight);
        Computed {
            family,
            families,
            family_layer,
            size,
            line_height,
            notes,
        }
    }
}

fn prefix(p: Property, layer: &str, note: Note) -> Note {
    Note {
        message: format!("{} in {layer}: {}", p.name(), note.message),
        ..note
    }
}

fn compute_property(
    p: Property,
    layers: &[Layer],
    functions: &FunctionRegistry,
    notes: &mut Vec<Note>,
) -> PropertyChain {
    let mut steps: Vec<Step> = Vec::new();
    for layer in layers {
        let Some(authored) = layer.style.get(p) else {
            continue;
        };
        let inherited = steps.last().map(|s| s.value.clone());
        let (own, value, basis) = match &authored {
            Authored::Unparsed(text) => {
                let problem = authored.problem();
                let why = problem.as_ref().map_or(String::new(), |e| format!(": {e}"));
                let code = problem.map_or(codes::STYLE_UNPARSED, |e| e.code());
                notes.push(prefix(
                    p,
                    &layer.name,
                    Note::warning(
                        code,
                        format!(
                            "the stored value {text:?} can't be read{why}; it is kept, and ignored"
                        ),
                    ),
                ));
                continue;
            }
            // The original forms follow the original rules, with no expression
            // machinery and so no notes.
            Authored::Legacy(LengthExpr::Pt(l)) => (
                ComputedLength::Absolute(*l),
                ComputedLength::Absolute(*l),
                None,
            ),
            Authored::Legacy(LengthExpr::Em(m)) => {
                let own = ComputedLength::EmRelative(*m);
                let value = match (p, &inherited) {
                    (Property::LineHeight, _) => own.clone(),
                    (Property::Size, None) => {
                        ComputedLength::Absolute(Length::ZERO.mul_ratio(*m, 1000))
                    }
                    (Property::Size, Some(ComputedLength::Absolute(e))) => {
                        ComputedLength::Absolute(e.mul_ratio(*m, 1000))
                    }
                    (Property::Size, Some(_)) => {
                        bake(&LengthExpr::Em(*m).to_expr(), inherited.as_ref(), functions)
                            .unwrap_or_else(|| own.clone())
                    }
                };
                (own, value, None)
            }
            Authored::Expr(e) => match e.compute(&p.own_scope(), functions) {
                Ok(own) => {
                    let value = match p {
                        Property::Size => bake(e, inherited.as_ref(), functions),
                        Property::LineHeight => None,
                    }
                    .unwrap_or_else(|| own.clone());
                    let basis = e
                        .dependencies(functions)
                        .contains(&Dependency::PercentBasis)
                        .then(|| format!("percentages are of {}", p.percent_basis()));
                    (own, value, basis)
                }
                Err(failed) => {
                    notes.extend(failed.into_iter().map(|n| {
                        prefix(
                            p,
                            &layer.name,
                            Note {
                                message: format!("{}; this layer's value is ignored", n.message),
                                ..n
                            },
                        )
                    }));
                    continue;
                }
            },
        };
        steps.push(Step {
            layer: layer.name.clone(),
            specified: authored.describe(),
            inherited: inherited
                .as_ref()
                .map_or(String::new(), ComputedLength::describe),
            value,
            own,
            basis,
            font_term: p.font_term(),
        });
    }
    PropertyChain { steps }
}

/// A size layer's value with the inherited size worked in. `None` if that
/// can't be done, in which case the layer is shown as it is on its own.
fn bake(
    e: &Expr,
    inherited: Option<&ComputedLength>,
    functions: &FunctionRegistry,
) -> Option<ComputedLength> {
    let inherited = inherited
        .cloned()
        .unwrap_or(ComputedLength::Absolute(Length::ZERO));
    let scope = Scope {
        em: Term::Bound(inherited.clone()),
        lh: Term::Cyclic,
        percent: Term::Bound(inherited),
    };
    e.compute(&scope, functions).ok()
}

impl Computed {
    /// What the computed style depends on, per property, as sets that
    /// iterate in a fixed order (27). The union over every step: a layer that
    /// can't be used falls back to the one before, so all of them matter.
    pub fn dependencies(
        &self,
        functions: &FunctionRegistry,
    ) -> BTreeMap<String, BTreeSet<Dependency>> {
        let mut out = BTreeMap::new();
        for (p, chain) in [
            (Property::Size, &self.size),
            (Property::LineHeight, &self.line_height),
        ] {
            let deps: BTreeSet<Dependency> = chain
                .steps
                .iter()
                .flat_map(|s| s.value.dependencies(functions))
                .collect();
            out.insert(p.name().to_string(), deps);
        }
        out
    }

    /// Stage 4 (08): resolves against a context, falling back along each
    /// property's chain for values that can't be used, and clamping to non-negative.
    pub fn used(
        &self,
        context: &ResolutionContext,
        functions: &FunctionRegistry,
    ) -> StyleResolution {
        let mut notes = self.notes.clone();
        let mut explain = BTreeMap::new();
        let mut bases = BTreeMap::new();
        let mut stages = BTreeMap::new();
        if let Some(layer) = &self.family_layer {
            explain.insert("family".to_string(), layer.clone());
        }

        let size = self.resolve(
            Property::Size,
            &self.size,
            functions,
            context,
            None,
            &mut notes,
        );
        // Line height resolves against the size before clamping, as it always
        // did: a negative size makes a negative line height, and both clamp.
        let line_height = self.resolve(
            Property::LineHeight,
            &self.line_height,
            functions,
            context,
            Some(size.value),
            &mut notes,
        );

        let mut clamped = Vec::new();
        let mut used = |name: &str, value: Length| {
            if value < Length::ZERO {
                clamped.push(name.to_string());
                Length::ZERO
            } else {
                value
            }
        };
        let (size_used, line_used) = (
            used("size", size.value),
            used("line-height", line_height.value),
        );
        for (p, r, value) in [
            (Property::Size, &size, size_used),
            (Property::LineHeight, &line_height, line_used),
        ] {
            let name = p.name().to_string();
            if let Some(layer) = &r.layer {
                explain.insert(name.clone(), layer.clone());
            }
            if !r.bases.is_empty() {
                bases.insert(name.clone(), r.bases.join("; "));
            }
            stages.insert(
                name,
                StageExplanation {
                    layer: r.layer.clone(),
                    specified: r.specified.clone(),
                    inherited: r.inherited.clone(),
                    computed: r.computed.clone(),
                    used: value,
                    basis: r.bases.clone(),
                },
            );
        }
        StyleResolution {
            style: ComputedStyle {
                family: self.family.clone(),
                families: self.families.clone(),
                size: size_used,
                line_height: line_used,
                explain,
                clamped,
                bases,
                notes,
            },
            stages,
        }
    }

    /// Resolves one property's chain. `size` runs forward, each layer's `em`
    /// being what the layers before it left; a layer that can't be used is
    /// skipped, so a later layer is applied to the value that survived.
    /// `line-height` takes the last layer that can be used, against the final
    /// size (`line_em`).
    fn resolve(
        &self,
        p: Property,
        chain: &PropertyChain,
        functions: &FunctionRegistry,
        context: &ResolutionContext,
        line_em: Option<Length>,
        notes: &mut Vec<Note>,
    ) -> Chosen {
        let env = |em| UsedEnv {
            functions,
            context,
            em,
            lh: None,
        };
        let mut chosen = Chosen::default();
        match p {
            Property::Size => {
                let mut running = Length::ZERO;
                for (i, step) in chain.steps.iter().enumerate() {
                    let u = step.own.used(&env(Some(running)));
                    let skip = u.degraded && i > 0;
                    report(p, step, u.notes, skip, notes);
                    if skip {
                        continue;
                    }
                    let inherited = if i == 0 {
                        String::new()
                    } else {
                        ComputedLength::Absolute(running).describe()
                    };
                    chosen = Chosen::from_step(step, u.value, inherited, running, u.bases);
                    running = u.value;
                }
            }
            Property::LineHeight => {
                for (i, step) in chain.steps.iter().enumerate().rev() {
                    let u = step.own.used(&env(line_em));
                    let skip = u.degraded && i > 0;
                    report(p, step, u.notes, skip, notes);
                    if skip {
                        continue;
                    }
                    let em = line_em.unwrap_or(Length::ZERO);
                    return Chosen::from_step(step, u.value, step.inherited.clone(), em, u.bases);
                }
            }
        }
        chosen
    }
}

/// Publishes the notes from using one step's value.
fn report(p: Property, step: &Step, found: Vec<Note>, skipped: bool, notes: &mut Vec<Note>) {
    notes.extend(found.into_iter().map(|n| {
        let message = if skipped {
            format!(
                "{}; this layer's value is ignored and the value inherited from before is used",
                n.message
            )
        } else {
            n.message.clone()
        };
        prefix(p, &step.layer, Note { message, ..n })
    }));
}

/// A property resolved at the used stage, before clamping.
#[derive(Default)]
struct Chosen {
    value: Length,
    layer: Option<String>,
    specified: String,
    inherited: String,
    computed: String,
    bases: Vec<String>,
}

impl Chosen {
    /// `font` is the font size the value was relative to, for the explanation.
    fn from_step(
        step: &Step,
        value: Length,
        inherited: String,
        font: Length,
        used_bases: Vec<String>,
    ) -> Chosen {
        let mut bases = Vec::new();
        if let Some(basis) = &step.basis {
            bases.push(format!("{basis}; {} = {font:?}", step.font_term));
        }
        bases.extend(used_bases);
        Chosen {
            value,
            layer: Some(step.layer.clone()),
            specified: step.specified.clone(),
            inherited,
            computed: step.value.describe(),
            bases,
        }
    }
}

// ---------------------------------------------------------------------------
// Stage 4: used, with the whole story

/// All four stages for one property (39).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageExplanation {
    /// The layer whose value was used; `None` if the property had no usable value.
    pub layer: Option<String>,
    /// Stage 1: that layer's value as authored.
    pub specified: String,
    /// Stage 2: the value it inherited from the layer before; empty for the first layer.
    pub inherited: String,
    /// Stage 3: the computed value, in the expression syntax.
    pub computed: String,
    /// Stage 4: the used value, after clamping.
    pub used: Length,
    /// What it was resolved against: percentage bases, frames, pages.
    pub basis: Vec<String>,
}

/// The used style, and an explanation of how each property got there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyleResolution {
    pub style: ComputedStyle,
    /// By property name.
    pub stages: BTreeMap<String, StageExplanation>,
}

impl fmt::Display for StageExplanation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "specified {} | inherited {} | computed {} | used {:?}",
            self.specified,
            if self.inherited.is_empty() {
                "nothing"
            } else {
                &self.inherited
            },
            self.computed,
            self.used
        )
    }
}

#[cfg(test)]
mod tests;
