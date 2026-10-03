//! Resolution contexts (decision 18): what a percentage, or a reference such
//! as `frame-width`, is relative to.
//!
//! # Levels
//!
//! Contexts are hierarchical, outermost first: [`Level::Medium`] (the output
//! medium, for example a screen or a sheet of paper), [`Level::Page`],
//! [`Level::Frame`], [`Level::Block`] and [`Level::Line`]. Medium and page are
//! *physical*: width runs left to right. Frame, block and line are *logical*
//! (20): width is the inline axis and height is the block axis of the frame's
//! own space, so rotating or mirroring a frame never changes what
//! `frame-width` means.
//!
//! # Basis states
//!
//! Every extent of every level is one of three [`Resolved`] states:
//!
//! -   **Definite**: a length.
//! -   **Indefinite**: the level exists but that extent depends on its own
//!     content, for example the height of an auto-height frame. A percentage of
//!     it would be circular.
//! -   **Unresolved**: there is no such level in this context. This is the
//!     state of everything in [`ResolutionContext::default`], and of a name that
//!     no frame has.
//!
//! An expression that needs a basis in either of the last two states is
//! *degraded*: the term evaluates to zero, the expression reports
//! `style.basis-indefinite` or `style.basis-unresolved` (both `Warning`: the
//! output differs from what the author asked for), and a style property whose
//! winning value is degraded keeps the value it inherited instead (see
//! [`crate::style`]).
//!
//! # Hierarchical defaults
//!
//! A reference such as `frame-width` is *exact*: it names one level. A
//! property that declares no basis of its own uses [`ResolutionContext::nearest`]:
//! the innermost level, at or outside the property's own, that is not
//! unresolved. An *indefinite* level stops the walk, so the percentage height
//! of an auto-height frame is indefinite and never silently becomes the page's.

use std::collections::BTreeMap;
use std::fmt;

use reprise_geom::Length;
use serde::{Deserialize, Serialize};

/// Where a basis sits in the hierarchy, outermost first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Level {
    Medium,
    Page,
    Frame,
    Block,
    Line,
}

impl Level {
    pub const ALL: [Level; 5] = [
        Level::Medium,
        Level::Page,
        Level::Frame,
        Level::Block,
        Level::Line,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Level::Medium => "medium",
            Level::Page => "page",
            Level::Frame => "frame",
            Level::Block => "block",
            Level::Line => "line",
        }
    }

    pub fn from_name(s: &str) -> Option<Level> {
        Level::ALL.into_iter().find(|l| l.name() == s)
    }

    /// The next level out, if any.
    fn outer(self) -> Option<Level> {
        match self {
            Level::Medium => None,
            Level::Page => Some(Level::Medium),
            Level::Frame => Some(Level::Page),
            Level::Block => Some(Level::Frame),
            Level::Line => Some(Level::Block),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Axis {
    Width,
    Height,
}

impl Axis {
    pub fn name(self) -> &'static str {
        match self {
            Axis::Width => "width",
            Axis::Height => "height",
        }
    }
}

/// What one extent of one level is in a context (see the module docs).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "length", rename_all = "kebab-case")]
pub enum Resolved {
    Definite(Length),
    Indefinite,
    #[default]
    Unresolved,
}

/// The two extents of a level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extent {
    pub width: Resolved,
    pub height: Resolved,
}

impl Extent {
    pub const UNRESOLVED: Extent = Extent {
        width: Resolved::Unresolved,
        height: Resolved::Unresolved,
    };

    pub fn definite(width: Length, height: Length) -> Extent {
        Extent {
            width: Resolved::Definite(width),
            height: Resolved::Definite(height),
        }
    }

    /// A box with a known width whose height follows its content.
    pub fn auto_height(width: Length) -> Extent {
        Extent {
            width: Resolved::Definite(width),
            height: Resolved::Indefinite,
        }
    }

    pub fn get(&self, axis: Axis) -> Resolved {
        match axis {
            Axis::Width => self.width,
            Axis::Height => self.height,
        }
    }
}

/// How a reference picks its level.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BasisScope {
    /// Exactly this level. For [`Level::Frame`] that is the current frame.
    Exact,
    /// The frame with this name, wherever it is. Only for [`Level::Frame`].
    Named(String),
    /// The nearest level at or outside this one that is not unresolved.
    Nearest,
}

/// A reference to a geometric basis: `frame-width`, `page-height`,
/// `frame-width("main")`, `nearest-width("block")`, and so on.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Basis {
    pub level: Level,
    pub axis: Axis,
    pub scope: BasisScope,
}

impl Basis {
    pub fn exact(level: Level, axis: Axis) -> Basis {
        Basis {
            level,
            axis,
            scope: BasisScope::Exact,
        }
    }

    pub fn frame(name: &str, axis: Axis) -> Basis {
        Basis {
            level: Level::Frame,
            axis,
            scope: BasisScope::Named(name.into()),
        }
    }

    pub fn nearest(level: Level, axis: Axis) -> Basis {
        Basis {
            level,
            axis,
            scope: BasisScope::Nearest,
        }
    }

    /// The bare identifier of the text form, without any argument.
    pub(crate) fn ident(&self) -> String {
        match self.scope {
            BasisScope::Nearest => format!("nearest-{}", self.axis.name()),
            _ => format!("{}-{}", self.level.name(), self.axis.name()),
        }
    }

    /// Recognises the identifiers `medium-width`, `frame-height`, `nearest-width`, and so on.
    /// `nearest-*` comes back with a placeholder level the parser replaces with its argument.
    pub(crate) fn from_ident(s: &str) -> Option<Basis> {
        let (head, axis) = s.rsplit_once('-')?;
        let axis = match axis {
            "width" => Axis::Width,
            "height" => Axis::Height,
            _ => return None,
        };
        if head == "nearest" {
            return Some(Basis::nearest(Level::Line, axis));
        }
        Some(Basis::exact(Level::from_name(head)?, axis))
    }
}

impl fmt::Display for Basis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.ident())?;
        match &self.scope {
            BasisScope::Exact => Ok(()),
            BasisScope::Named(n) => write!(f, "({})", quote(n)),
            BasisScope::Nearest => write!(f, "({})", quote(self.level.name())),
        }
    }
}

/// A string literal of the text form: `"` and `\` are escaped, nothing else.
pub(crate) fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// Everything a used value may be relative to besides the font (18).
///
/// Layout builds one per frame, block or line it resolves styles for. The
/// default context has nothing in it: every reference is unresolved, so a
/// style that only uses `pt` and `em` resolves the same in every context.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolutionContext {
    pub medium: Extent,
    pub page: Extent,
    /// The frame the block is being laid out in.
    pub frame: Extent,
    /// Frames by name, for `frame-width("name")`. Includes the current frame
    /// if it has a name ([`ResolutionContext::with_current_frame`]).
    pub frames: BTreeMap<String, Extent>,
    /// The block's own inline space: the width it may fill, before any line is formed.
    pub block: Extent,
    /// The line being formed. Width varies per line when the geometry is not a rectangle.
    pub line: Extent,
}

impl ResolutionContext {
    pub fn with_medium(mut self, extent: Extent) -> Self {
        self.medium = extent;
        self
    }

    pub fn with_page(mut self, extent: Extent) -> Self {
        self.page = extent;
        self
    }

    /// Makes `name` the current frame, and also available by that name.
    pub fn with_current_frame(mut self, name: &str, extent: Extent) -> Self {
        self.frame = extent;
        self.frames.insert(name.into(), extent);
        self
    }

    /// Makes a frame available by name without making it the current one.
    pub fn with_named_frame(mut self, name: &str, extent: Extent) -> Self {
        self.frames.insert(name.into(), extent);
        self
    }

    pub fn with_block(mut self, extent: Extent) -> Self {
        self.block = extent;
        self
    }

    pub fn with_line(mut self, extent: Extent) -> Self {
        self.line = extent;
        self
    }

    pub fn level(&self, level: Level) -> &Extent {
        match level {
            Level::Medium => &self.medium,
            Level::Page => &self.page,
            Level::Frame => &self.frame,
            Level::Block => &self.block,
            Level::Line => &self.line,
        }
    }

    /// The innermost level at or outside `level` whose `axis` is not
    /// unresolved, with its state. An indefinite level ends the walk.
    /// Everything unresolved gives `(Level::Medium, Unresolved)`.
    pub fn nearest(&self, level: Level, axis: Axis) -> (Level, Resolved) {
        let mut at = level;
        loop {
            let r = self.level(at).get(axis);
            if r != Resolved::Unresolved {
                return (at, r);
            }
            match at.outer() {
                Some(outer) => at = outer,
                None => return (Level::Medium, Resolved::Unresolved),
            }
        }
    }

    /// The state of one reference in this context.
    pub fn basis(&self, basis: &Basis) -> Resolved {
        match &basis.scope {
            BasisScope::Exact => self.level(basis.level).get(basis.axis),
            BasisScope::Named(name) => self
                .frames
                .get(name)
                .map_or(Resolved::Unresolved, |e| e.get(basis.axis)),
            BasisScope::Nearest => self.nearest(basis.level, basis.axis).1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(n: i32) -> Length {
        Length::from_pt(n)
    }

    #[test]
    fn the_default_context_resolves_nothing() {
        let ctx = ResolutionContext::default();
        for level in Level::ALL {
            for axis in [Axis::Width, Axis::Height] {
                assert_eq!(ctx.basis(&Basis::exact(level, axis)), Resolved::Unresolved);
                assert_eq!(
                    ctx.basis(&Basis::nearest(level, axis)),
                    Resolved::Unresolved
                );
            }
        }
    }

    #[test]
    fn nearest_walks_outward_past_unresolved_levels() {
        let ctx = ResolutionContext::default()
            .with_page(Extent::definite(pt(600), pt(800)))
            .with_current_frame("main", Extent::auto_height(pt(400)));
        // The block and line levels are unresolved: the frame answers.
        assert_eq!(
            ctx.nearest(Level::Line, Axis::Width),
            (Level::Frame, Resolved::Definite(pt(400)))
        );
        // An indefinite height stops the walk; it never becomes the page's.
        assert_eq!(
            ctx.nearest(Level::Line, Axis::Height),
            (Level::Frame, Resolved::Indefinite)
        );
        assert_eq!(
            ctx.nearest(Level::Page, Axis::Height),
            (Level::Page, Resolved::Definite(pt(800)))
        );
        // Medium is unresolved and has nothing outside it.
        assert_eq!(
            ctx.nearest(Level::Medium, Axis::Width),
            (Level::Medium, Resolved::Unresolved)
        );
    }

    #[test]
    fn named_frames_are_looked_up_by_name() {
        let ctx = ResolutionContext::default()
            .with_current_frame("main", Extent::definite(pt(400), pt(700)))
            .with_named_frame("margin", Extent::auto_height(pt(100)));
        assert_eq!(
            ctx.basis(&Basis::frame("margin", Axis::Width)),
            Resolved::Definite(pt(100))
        );
        assert_eq!(
            ctx.basis(&Basis::frame("margin", Axis::Height)),
            Resolved::Indefinite
        );
        assert_eq!(
            ctx.basis(&Basis::frame("nowhere", Axis::Width)),
            Resolved::Unresolved
        );
        assert_eq!(
            ctx.basis(&Basis::exact(Level::Frame, Axis::Width)),
            Resolved::Definite(pt(400))
        );
    }

    #[test]
    fn identifiers_round_trip() {
        for level in Level::ALL {
            for axis in [Axis::Width, Axis::Height] {
                let b = Basis::exact(level, axis);
                assert_eq!(Basis::from_ident(&b.ident()), Some(b));
            }
        }
        assert_eq!(Basis::from_ident("frame-depth"), None);
        assert_eq!(Basis::from_ident("width"), None);
        assert_eq!(Basis::from_ident("galaxy-width"), None);
    }
}
