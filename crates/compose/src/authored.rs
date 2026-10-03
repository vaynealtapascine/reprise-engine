//! The authored-break composer, for verse (decisions 11 and 23).
//!
//! Lines end where the author ended them: at forced breaks, which stand for
//! authored line ends. An authored line too long for its interval *turns
//! over*: it wraps at its break opportunities, and each continuation line is
//! moved in by [`AuthoredBreak::turnover_indent`], the hanging turnover of
//! verse typesetting.
//!
//! # Which authored line a fragment belongs to
//!
//! The document model has no verse-line entities yet (decision 11 calls for
//! them), so authored lines are recovered from what each [`LineFragment`]
//! already carries:
//!
//! -   With this composer, a fragment *ends* an authored line exactly when its
//!     `explanation.reason` is [`BreakReason::Forced`] or [`BreakReason::End`];
//!     every other fragment is followed by a turnover. So the authored line of
//!     fragment `i` is the number of earlier fragments that end one, and a
//!     fragment is a turnover when the fragment before it doesn't end one.
//!     (An overflowing line that ends at a forced break still says `Forced`
//!     here; [`Greedy`](crate::Greedy) says `Overflow` for it.)
//! -   For any composer, the same answer comes from source ranges: a fragment
//!     ends an authored line when its `text.end` is the end of the text or the
//!     `at` of a [`BreakKind::Forced`](crate::BreakKind::Forced) break in the
//!     request's `breaks`.
//!
//! [`LineFragment`]: crate::LineFragment
//! [`BreakReason::Forced`]: crate::BreakReason::Forced
//! [`BreakReason::End`]: crate::BreakReason::End

use reprise_geom::Length;

use crate::greedy::{FirstFit, first_fit};
use crate::para::Para;
use crate::walk::Walk;
use crate::{ComposeRequest, Composer, Composition, Optimal, optimal};

/// How an authored line that doesn't fit is turned over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turnover {
    /// Choose the turnover breaks with the optimal composer and these
    /// settings, over the whole text at once. Every path goes through every
    /// forced break, so this is optimal within each authored line too.
    Optimal(Optimal),
    /// Fill each line as far as it goes, like [`Greedy`](crate::Greedy).
    /// Lines have no score.
    FirstFit,
}

/// The authored-break composer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthoredBreak {
    /// How far a turnover's first interval on its line starts in: its inline
    /// start moves in by this much, never past the interval's end. The first
    /// fragment of an authored line, and fragments in a line's later
    /// intervals, are not moved. Negative values count as zero.
    pub turnover_indent: Length,
    pub turnover: Turnover,
}

impl Default for AuthoredBreak {
    /// An 18 pt hanging turnover, broken optimally and ragged.
    fn default() -> AuthoredBreak {
        AuthoredBreak {
            turnover_indent: Length::from_pt(18),
            turnover: Turnover::Optimal(Optimal::default()),
        }
    }
}

impl Composer for AuthoredBreak {
    fn name(&self) -> &'static str {
        match self.turnover {
            Turnover::Optimal(_) => "authored-break",
            Turnover::FirstFit => "authored-break-first-fit",
        }
    }

    fn compose(&self, request: &ComposeRequest<'_>) -> Composition {
        let indent = self.turnover_indent.max(Length::ZERO);
        match &self.turnover {
            Turnover::Optimal(cfg) => optimal::compose(cfg, request, indent),
            Turnover::FirstFit => {
                let para = Para::new(request);
                let mut out = Composition::default();
                let mut walk = Walk::new(request.block_start);
                let style = FirstFit {
                    indent,
                    forced_reason_wins: true,
                };
                first_fit(&para, &mut walk, para.start, &mut out, style);
                out
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Shaped;
    use crate::{BreakReason, Measure};

    const VERSE: &str = "Whose woods these are I think I know.\n\
                         His house is in the village though;\n\
                         He will not see me stopping here\n\
                         To watch his woods fill up with snow.";

    fn pt(v: i32) -> Length {
        Length::from_pt(v)
    }

    #[test]
    fn authored_lines_turn_over_with_a_hanging_indent() {
        let shaped = Shaped::new(VERSE);
        for turnover in [Turnover::Optimal(Optimal::default()), Turnover::FirstFit] {
            let composer = AuthoredBreak {
                turnover_indent: pt(15),
                turnover,
            };
            let c = shaped.compose(&composer, &Measure(pt(110)));
            assert!(c.notes.is_empty(), "{:?}", c.notes);
            assert_eq!(shaped.joined(&c), VERSE);
            assert!(c.lines.len() > 4, "something turns over");
            let mut starts_line = true;
            for l in &c.lines {
                let expected = if starts_line { pt(0) } else { pt(15) };
                assert_eq!(l.available.start, expected, "{:?}", &VERSE[l.text.clone()]);
                assert_eq!(l.available.end, pt(110));
                assert!(l.width <= l.available.width());
                starts_line =
                    matches!(l.explanation.reason, BreakReason::Forced | BreakReason::End);
            }
            let scored = c.lines.iter().all(|l| l.explanation.score.is_some());
            assert_eq!(scored, matches!(turnover, Turnover::Optimal(_)));
        }
    }

    #[test]
    fn names_say_how_turnovers_break() {
        assert_eq!(AuthoredBreak::default().name(), "authored-break");
        let first_fit = AuthoredBreak {
            turnover: Turnover::FirstFit,
            ..AuthoredBreak::default()
        };
        assert_eq!(first_fit.name(), "authored-break-first-fit");
    }

    #[test]
    fn negative_and_huge_indents_are_harmless() {
        let shaped = Shaped::new(VERSE);
        let negative = AuthoredBreak {
            turnover_indent: pt(-40),
            ..AuthoredBreak::default()
        };
        let c = shaped.compose(&negative, &Measure(pt(110)));
        assert!(c.lines.iter().all(|l| l.available.start == Length::ZERO));
        // An indent past the measure leaves turnovers no room: each
        // overflows with one word, reported, and nothing panics.
        let huge = AuthoredBreak {
            turnover_indent: Length::MAX,
            ..AuthoredBreak::default()
        };
        let c = shaped.compose(&huge, &Measure(pt(110)));
        assert_eq!(shaped.joined(&c), VERSE);
        assert!(c.notes.iter().any(|n| n.code == crate::codes::OVERFLOW));
    }
}
