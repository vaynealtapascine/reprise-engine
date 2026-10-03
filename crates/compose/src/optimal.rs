//! The optimal composer: Knuth–Plass total-fit line breaking over variable
//! geometry (decision 23), in integer arithmetic (19, 38).
//!
//! # Demerits
//!
//! Every line gets *demerits*, a dimensionless integer; lower is better. They
//! are TeX's, computed exactly:
//!
//! ```text
//! demerits = (line_penalty + badness)²  ± penalty²  [+ fitness_demerits]
//! ```
//!
//! -   `badness` is `min(10000, round(100 · r³))`, where `r` is the line's
//!     adjustment ratio rounded to 16.16 fixed point: `r = round(gap · 2¹⁶ /
//!     scale) / 2¹⁶`, `gap = |available width − natural width|` in sub-units
//!     (1/1024 pt) and `scale` is how far the line may stretch or shrink, also
//!     in sub-units (see [`Mode`]). Ratios at or above 4.6416 (`r16 ≥ 304192`)
//!     give 10000 directly, so the cube never overflows.
//! -   `penalty` is the break's [`Break::penalty`](crate::Break::penalty),
//!     clamped to ±10000: added squared when positive, subtracted squared when
//!     negative. Forced breaks and the end of the text add nothing.
//! -   `fitness_demerits` is added when the line's fitness class (very loose,
//!     loose, decent, tight: badness ≥ 100 / ≥ 13 stretching, < 13, ≥ 13
//!     shrinking) is more than one class away from the previous line's.
//!     The first line follows a decent one.
//! -   A line that ends at a forced break or the end of the text is set at its
//!     natural width, like TeX's `\parfillskip`: badness 0 when it fits. In
//!     justified mode it may still shrink.
//!
//! Each fragment's [`Explanation::score`](crate::Explanation::score) is its
//! line's demerits. Every term is bounded (at most about 6 · 10⁸ per line),
//! and totals add with saturation in `i64`.
//!
//! The composer minimises, in order: the number of overflowing lines, then
//! total demerits. An overflowing line holds one unbreakable run that fits
//! nowhere (reported with [`codes::OVERFLOW`]); its badness counts as 10000
//! and its fitness as decent. Skipping an interval on a line with several
//! (a *pass*) costs `(line_penalty + 10000)²`, like an empty line at the
//! worst badness. Ties go to the path found first: the one whose earlier
//! lines break earlier, then the one using fewer lines.
//!
//! # Geometry, and when the result is optimal
//!
//! Composition is a shortest path over states `(break, slot, fitness)`. A
//! *slot* is one interval of one line of the region, in the order the
//! geometry provider gives them; the slots form a *ladder* that the composer
//! reads by walking the provider exactly as [`Greedy`](crate::Greedy) does
//! (skips, `End`, [`MAX_CONSECUTIVE_SKIPS`](crate::MAX_CONSECUTIVE_SKIPS)).
//! Because each slot carries its own line index and block offset, widths may
//! depend on both, and on `Skip`s, `End` and several intervals per line:
//!
//! -   **Line index, block offset, `Skip` and multiple intervals:** optimal.
//!     The ladder is read once, up to as many lines as there are breaks (no
//!     paragraph can need more), and the search runs over it exactly.
//! -   **`End`:** the region holds fewer lines than the text needs. The
//!     composer then maximises how much text it places (so `rest` is as late
//!     as possible), then minimises overflows and demerits. That is optimal
//!     for this region; the next region is composed separately from `rest`.
//! -   **Geometry that depends on `previous`:** the ladder is read with
//!     `previous` empty, then every answer is checked again, with the real
//!     previous fragments, while the lines are placed. If an answer differs,
//!     the lines before it stay as planned, and the rest are set first-fit
//!     ([`Greedy`](crate::Greedy)'s algorithm, from that exact point), with
//!     `score: None`, and a [`codes::FALLBACK`] note. Keeping a state per
//!     `(line, offset, previous)` instead would make the state space depend
//!     on every earlier line, which is not bounded.
//!
//! When the ladder's lines are all alike from some line on (a plain
//! [`Measure`](crate::Measure), or a shape above a rectangular tail), states
//! in that tail that differ only in their line index are merged, keeping the
//! cheapest. That is exact, because their futures are identical, and it is
//! what keeps a long paragraph's search linear (TeX's "easy lines").
//!
//! # Limits
//!
//! The search stops at the [`Limits`]: states waiting to be expanded, and
//! line candidates evaluated in total (reading the ladder counts too). When a
//! limit is hit, the whole paragraph is set first-fit instead, with no
//! scores, and a [`codes::FALLBACK`] note (Info) says which limit. Falling
//! back is deterministic, and keeps every composer guarantee; pruning the
//! search instead would give lines whose scores claim an optimality they
//! don't have. Time is O(`max_candidates` · log `max_active`) plus a linear
//! first-fit pass; memory is O(`max_candidates`).

use std::collections::BTreeMap;

use reprise_diag::Note;
use reprise_geom::{Length, div_round};

use crate::greedy::{FirstFit, first_fit};
use crate::para::{Para, indented};
use crate::walk::{Step, Walk, stalled};
use crate::{
    Adjustment, Available, Break, BreakKind, BreakReason, ComposeRequest, Composer, Composition,
    Interval, codes,
};

/// Badness of a line stretched or shrunk as far as it goes, and the most
/// any line gets.
pub const INF_BAD: i64 = 10_000;
/// The largest penalty, line penalty or fitness demerit taken into account.
pub const MAX_PENALTY: i64 = 10_000;

/// How lines are fitted to their intervals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Lines keep their natural spacing and minimise raggedness. A line's
    /// `scale` is [`Optimal::ragged_stretch`] per mille of its interval's
    /// width, so a line short by a quarter of the measure has badness 100 by
    /// default. No [`Adjustment`] is set.
    #[default]
    Ragged,
    /// Inter-word spaces stretch (by [`Optimal::stretch`] per mille of their
    /// natural width) or shrink (by [`Optimal::shrink`]) so lines fill their
    /// interval. [`Adjustment::word_spacing`] is set to `gap / spaces`,
    /// rounded half away from zero, so a line fills its interval to within
    /// half a sub-unit per space. Lines with no spaces can't stretch.
    Justified,
}

/// Bounds on the search, so huge paragraphs stay fast (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The most states waiting to be expanded at once.
    pub max_active: usize,
    /// The most line candidates evaluated, plus geometry answers read.
    pub max_candidates: u64,
}

impl Default for Limits {
    /// 16 384 active states and 2 097 152 candidates: enough for a paragraph
    /// of about 30 000 words in a plain measure, or a page-sized shape.
    fn default() -> Limits {
        Limits {
            max_active: 1 << 14,
            max_candidates: 1 << 21,
        }
    }
}

/// The optimal (Knuth–Plass) composer. See the module docs for the demerits,
/// what is optimal, and the limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Optimal {
    pub mode: Mode,
    /// Added to every line's badness before squaring, so fewer lines are
    /// preferred. TeX's `\linepenalty`; clamped to `0..=10000`.
    pub line_penalty: i64,
    /// Added when consecutive lines are more than one fitness class apart.
    /// TeX's `\adjdemerits`; clamped to `0..=10000²`.
    pub fitness_demerits: i64,
    /// [`Mode::Ragged`]: how far a line may fall short, per mille of its
    /// interval's width, before its badness reaches 100. At least 1.
    pub ragged_stretch: u32,
    /// [`Mode::Justified`]: stretch per mille of the natural space width.
    pub stretch: u32,
    /// [`Mode::Justified`]: shrink per mille of the natural space width, at
    /// most 1000.
    pub shrink: u32,
    pub limits: Limits,
}

impl Default for Optimal {
    fn default() -> Optimal {
        Optimal {
            mode: Mode::Ragged,
            line_penalty: 10,
            fitness_demerits: 10_000,
            ragged_stretch: 250,
            stretch: 500,
            shrink: 333,
            limits: Limits::default(),
        }
    }
}

impl Optimal {
    /// The default settings, justified.
    pub fn justified() -> Optimal {
        Optimal {
            mode: Mode::Justified,
            ..Optimal::default()
        }
    }

    fn line_penalty(&self) -> i64 {
        self.line_penalty.clamp(0, MAX_PENALTY)
    }

    fn fitness_demerits(&self) -> i64 {
        self.fitness_demerits.clamp(0, MAX_PENALTY * MAX_PENALTY)
    }

    /// The demerits of a line with this badness and fitness, ending at `b`.
    fn demerits(&self, badness: i64, fitness: u8, previous: u8, b: &Break, end: bool) -> i64 {
        let base = self.line_penalty() + badness.clamp(0, INF_BAD);
        let mut d = base * base;
        if b.kind != BreakKind::Forced && !end {
            let p = (b.penalty as i64).clamp(-MAX_PENALTY, MAX_PENALTY);
            d += p.signum() * p * p;
        }
        if fitness.abs_diff(previous) > 1 {
            d += self.fitness_demerits();
        }
        d
    }

    fn pass_demerits(&self) -> i64 {
        let base = self.line_penalty() + INF_BAD;
        base * base
    }

    /// How a line of natural width `natural` fits `room`, or `None` when it is
    /// too long. `last` is set for lines ending at a forced break or the end.
    fn fit(&self, natural: i64, room: i64, last: bool, spaces: (i64, u32)) -> Option<Fit> {
        let gap = room - natural;
        let (space_width, count) = spaces;
        let per_space = |gap: i64| match count {
            0 => Length::ZERO,
            n => Length(div_round(gap, n as i64).clamp(i32::MIN as i64, i32::MAX as i64) as i32),
        };
        let permille = |w: i64, p: u32| div_round(w.saturating_mul(p.min(1_000_000) as i64), 1000);
        if gap >= 0 && last {
            return Some(Fit::NATURAL);
        }
        match self.mode {
            Mode::Ragged => {
                if gap < 0 {
                    return None;
                }
                let scale = permille(room, self.ragged_stretch.max(1)).max(1);
                let badness = badness(gap, scale);
                Some(Fit {
                    badness,
                    fitness: stretching(badness),
                    word_spacing: Length::ZERO,
                })
            }
            Mode::Justified if gap >= 0 => {
                let stretch = permille(space_width, self.stretch);
                let badness = match (gap, stretch) {
                    (0, _) => 0,
                    (_, s) if s <= 0 => INF_BAD,
                    (g, s) => badness(g, s),
                };
                Some(Fit {
                    badness,
                    fitness: stretching(badness),
                    word_spacing: per_space(gap),
                })
            }
            Mode::Justified => {
                let shrink = permille(space_width, self.shrink.min(1000));
                if shrink <= 0 || -gap > shrink {
                    return None;
                }
                let badness = badness(-gap, shrink);
                Some(Fit {
                    badness,
                    fitness: if badness >= 13 { TIGHT } else { DECENT },
                    word_spacing: per_space(gap),
                })
            }
        }
    }
}

impl Composer for Optimal {
    fn name(&self) -> &'static str {
        match self.mode {
            Mode::Ragged => "optimal",
            Mode::Justified => "optimal-justified",
        }
    }

    fn compose(&self, request: &ComposeRequest<'_>) -> Composition {
        compose(self, request, Length::ZERO)
    }
}

const VERY_LOOSE: u8 = 0;
const LOOSE: u8 = 1;
const DECENT: u8 = 2;
const TIGHT: u8 = 3;

/// `min(INF_BAD, round(100 · r³))` for `r = gap / scale` in 16.16 fixed point.
fn badness(gap: i64, scale: i64) -> i64 {
    if gap <= 0 {
        return 0;
    }
    if scale <= 0 {
        return INF_BAD;
    }
    // 100 · r³ reaches 10000 at r = 4.6416, that is r16 = 304192.
    let r = div_round(gap.saturating_mul(1 << 16), scale);
    if r >= 304_192 {
        return INF_BAD;
    }
    div_round(100 * r * r * r, 1 << 48).min(INF_BAD)
}

fn stretching(badness: i64) -> u8 {
    match badness {
        100.. => VERY_LOOSE,
        13.. => LOOSE,
        _ => DECENT,
    }
}

#[derive(Clone, Copy, Debug)]
struct Fit {
    badness: i64,
    fitness: u8,
    word_spacing: Length,
}

impl Fit {
    const NATURAL: Fit = Fit {
        badness: 0,
        fitness: DECENT,
        word_spacing: Length::ZERO,
    };
}

/// Overflows first, then demerits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct Cost {
    overflows: u32,
    demerits: i64,
}

#[derive(Clone, Copy, Debug)]
enum Edge {
    Root,
    Pass,
    Line {
        demerits: i64,
        word_spacing: Length,
        overflow: bool,
    },
}

/// A state: the text up to `pos` is set, and the next fragment goes in `slot`.
#[derive(Clone, Copy, Debug)]
struct Node {
    pos: usize,
    /// 0 for the start, `i + 1` after `breaks[i]`.
    after: usize,
    slot: usize,
    fitness: u8,
    cost: Cost,
    parent: usize,
    edge: Edge,
}

/// One interval of one ladder line.
struct Slot {
    line: usize,
    /// Its index among the line's intervals.
    k: usize,
    interval: Interval,
    last_on_line: bool,
}

/// A line of the ladder: its walk position, and which answer gave it room.
struct Rung {
    line: u32,
    y: Length,
    answer: usize,
    first_slot: usize,
    slots: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Terminal {
    /// Read as many lines as the text could need.
    Horizon,
    End,
    Stalled,
}

/// The region as geometry answered with no previous lines.
struct Ladder {
    answers: Vec<Available>,
    rungs: Vec<Rung>,
    slots: Vec<Slot>,
    terminal: Terminal,
    /// Lines from here on are alike to the horizon.
    tail: usize,
}

#[derive(Clone, Copy, Debug)]
enum Abort {
    Active,
    Candidates,
}

struct Search<'p, 'r, 'a> {
    cfg: &'p Optimal,
    para: &'p Para<'r, 'a>,
    ladder: &'p Ladder,
    indent: Length,
    nodes: Vec<Node>,
    frontier: BTreeMap<(usize, usize, usize, u8), usize>,
    complete: Option<usize>,
    stopped: Option<usize>,
    work: u64,
}

impl Search<'_, '_, '_> {
    fn spend(&mut self, n: u64) -> Result<(), Abort> {
        self.work = self.work.saturating_add(n);
        if self.work > self.cfg.limits.max_candidates {
            return Err(Abort::Candidates);
        }
        Ok(())
    }

    /// Lines in the uniform tail share one class.
    fn key(&self, node: &Node) -> (usize, usize, usize, u8) {
        let slot = &self.ladder.slots[node.slot];
        let class = if slot.line >= self.ladder.tail {
            usize::MAX
        } else {
            slot.line
        };
        (node.after, class, slot.k, node.fitness)
    }

    fn line_of(&self, node: &Node) -> usize {
        self.ladder
            .slots
            .get(node.slot)
            .map_or(usize::MAX, |s| s.line)
    }

    fn offer(&mut self, node: Node) -> Result<(), Abort> {
        if node.pos >= self.para.len {
            if self.complete.is_none_or(|c| node.cost < self.nodes[c].cost) {
                self.complete = Some(self.push(node));
            }
            return Ok(());
        }
        if node.slot >= self.ladder.slots.len() {
            let better =
                |old: &Node| node.pos > old.pos || (node.pos == old.pos && node.cost < old.cost);
            if self.stopped.is_none_or(|s| better(&self.nodes[s])) {
                self.stopped = Some(self.push(node));
            }
            return Ok(());
        }
        let key = self.key(&node);
        match self.frontier.get(&key) {
            Some(&old) => {
                let held = &self.nodes[old];
                if (node.cost, self.line_of(&node)) < (held.cost, self.line_of(held)) {
                    self.nodes[old] = node;
                }
            }
            None => {
                let id = self.push(node);
                self.frontier.insert(key, id);
                if self.frontier.len() > self.cfg.limits.max_active {
                    return Err(Abort::Active);
                }
            }
        }
        Ok(())
    }

    fn push(&mut self, node: Node) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    fn expand(&mut self, id: usize) -> Result<(), Abort> {
        let node = self.nodes[id];
        let para = self.para;
        let slot = &self.ladder.slots[node.slot];
        let rung = &self.ladder.rungs[slot.line];
        let interval = if slot.k == 0 && !para.begins_line(node.pos) {
            indented(slot.interval, self.indent)
        } else {
            slot.interval
        };
        let room = interval.width().0 as i64;
        let next_line = rung.first_slot + rung.slots;
        let last_on_line = slot.last_on_line;
        let first = para.first_break_after(node.pos);
        for (j, b) in para.breaks.iter().enumerate().skip(first) {
            self.spend(1)?;
            let forced = b.kind == BreakKind::Forced;
            let end = b.at == para.len;
            let natural = para.line_width(node.pos, j).0 as i64;
            let next = if forced { next_line } else { node.slot + 1 };
            let fit = self
                .cfg
                .fit(natural, room, forced || end, para.spaces(node.pos, j));
            let Some(fit) = fit else {
                if j == first {
                    let demerits = self.cfg.demerits(INF_BAD, DECENT, node.fitness, b, end);
                    self.offer(Node {
                        pos: b.at,
                        after: j + 1,
                        slot: next,
                        fitness: DECENT,
                        cost: Cost {
                            overflows: node.cost.overflows.saturating_add(1),
                            demerits: node.cost.demerits.saturating_add(demerits),
                        },
                        parent: id,
                        edge: Edge::Line {
                            demerits,
                            word_spacing: Length::ZERO,
                            overflow: true,
                        },
                    })?;
                }
                break;
            };
            let demerits = self
                .cfg
                .demerits(fit.badness, fit.fitness, node.fitness, b, end);
            self.offer(Node {
                pos: b.at,
                after: j + 1,
                slot: next,
                fitness: fit.fitness,
                cost: Cost {
                    overflows: node.cost.overflows,
                    demerits: node.cost.demerits.saturating_add(demerits),
                },
                parent: id,
                edge: Edge::Line {
                    demerits,
                    word_spacing: fit.word_spacing,
                    overflow: false,
                },
            })?;
            if forced {
                break;
            }
        }
        if !last_on_line {
            self.spend(1)?;
            self.offer(Node {
                slot: node.slot + 1,
                cost: Cost {
                    overflows: node.cost.overflows,
                    demerits: node.cost.demerits.saturating_add(self.cfg.pass_demerits()),
                },
                parent: id,
                edge: Edge::Pass,
                ..node
            })?;
        }
        Ok(())
    }

    fn run(&mut self) -> Result<usize, Abort> {
        let root = Node {
            pos: self.para.start,
            after: 0,
            slot: 0,
            fitness: DECENT,
            cost: Cost::default(),
            parent: usize::MAX,
            edge: Edge::Root,
        };
        self.offer(root)?;
        while let Some((_, id)) = self.frontier.pop_first() {
            self.expand(id)?;
        }
        // The root is always offered, so one of the two is set.
        Ok(self.complete.or(self.stopped).unwrap_or(0))
    }
}

/// Reads the ladder: walks geometry with no previous lines until it ends,
/// stalls, or gives `horizon` lines.
fn read_ladder(request: &ComposeRequest<'_>, horizon: usize, budget: u64) -> Result<Ladder, Abort> {
    let mut ladder = Ladder {
        answers: Vec::new(),
        rungs: Vec::new(),
        slots: Vec::new(),
        terminal: Terminal::Horizon,
        tail: 0,
    };
    let mut walk = Walk::new(request.block_start);
    let mut work = 0u64;
    while ladder.rungs.len() < horizon {
        let answer = walk.query(request.geometry, request.line_height, &[]);
        let size = match &answer {
            Available::Room(i) => i.len() as u64,
            _ => 0,
        };
        work = work.saturating_add(1 + size);
        if work > budget {
            return Err(Abort::Candidates);
        }
        ladder.answers.push(answer.clone());
        let (line, y) = (walk.line, walk.y);
        match walk.apply(answer, request.line_height) {
            Step::Moved => {}
            Step::End => {
                ladder.terminal = Terminal::End;
                break;
            }
            Step::Stalled => {
                ladder.terminal = Terminal::Stalled;
                break;
            }
            Step::Room(intervals) => {
                let first_slot = ladder.slots.len();
                let rung = ladder.rungs.len();
                let n = intervals.len();
                ladder
                    .slots
                    .extend(intervals.into_iter().enumerate().map(|(k, interval)| Slot {
                        line: rung,
                        k,
                        interval,
                        last_on_line: k + 1 == n,
                    }));
                ladder.rungs.push(Rung {
                    line,
                    y,
                    answer: ladder.answers.len() - 1,
                    first_slot,
                    slots: n,
                });
                walk.advance(request.line_height);
            }
        }
    }
    ladder.tail = ladder.rungs.len();
    if ladder.terminal == Terminal::Horizon {
        let widths = |r: &Rung| {
            ladder.slots[r.first_slot..r.first_slot + r.slots]
                .iter()
                .map(|s| s.interval.width())
        };
        if let Some(last) = ladder.rungs.last() {
            while ladder.tail > 0 && widths(&ladder.rungs[ladder.tail - 1]).eq(widths(last)) {
                ladder.tail -= 1;
            }
        }
    }
    Ok(ladder)
}

/// A fragment the search chose.
struct Planned {
    slot: usize,
    start: usize,
    end: usize,
    reason: BreakReason,
    score: i64,
    word_spacing: Length,
    overflow: bool,
}

fn fallback_note(why: &str) -> Note {
    Note::info(codes::FALLBACK, format!("set first-fit instead: {why}"))
}

/// Composes optimally, moving turnovers (lines that continue an authored
/// line) in by `indent`.
pub(crate) fn compose(cfg: &Optimal, request: &ComposeRequest<'_>, indent: Length) -> Composition {
    let para = Para::new(request);
    let style = FirstFit {
        indent,
        forced_reason_wins: true,
    };
    let mut out = Composition::default();
    if para.start >= para.len {
        // Nothing to break: the one empty line, or nothing after the end.
        first_fit(
            &para,
            &mut Walk::new(request.block_start),
            para.start,
            &mut out,
            style,
        );
        let empty = cfg.line_penalty() * cfg.line_penalty();
        for line in &mut out.lines {
            line.explanation.score = Some(empty);
        }
        return out;
    }

    let searched =
        read_ladder(request, para.breaks.len(), cfg.limits.max_candidates).and_then(|ladder| {
            let mut search = Search {
                cfg,
                para: &para,
                ladder: &ladder,
                indent,
                nodes: Vec::new(),
                frontier: BTreeMap::new(),
                complete: None,
                stopped: None,
                work: ladder.answers.len() as u64 + ladder.slots.len() as u64,
            };
            let best = search.run()?;
            let complete = search.complete.is_some();
            let plan = plan(&search, best);
            Ok((ladder, plan, complete))
        });
    let (ladder, plan, complete) = match searched {
        Ok(found) => found,
        Err(abort) => {
            out.notes.push(fallback_note(match abort {
                Abort::Active => "the search had more active states than its limit",
                Abort::Candidates => "the search evaluated more candidates than its limit",
            }));
            first_fit(
                &para,
                &mut Walk::new(request.block_start),
                para.start,
                &mut out,
                style,
            );
            return out;
        }
    };
    replay(&para, &ladder, &plan, complete, &mut out, style);
    out
}

/// The fragments on the chosen path, in order.
fn plan(search: &Search<'_, '_, '_>, best: usize) -> Vec<Planned> {
    let mut plan = Vec::new();
    let mut id = best;
    while let Some(node) = search.nodes.get(id) {
        let Some(parent) = search.nodes.get(node.parent) else {
            break;
        };
        if let Edge::Line {
            demerits,
            word_spacing,
            overflow,
        } = node.edge
        {
            let b = search.para.breaks.get(node.after.wrapping_sub(1));
            let forced = b.is_some_and(|b| b.kind == BreakKind::Forced);
            let reason = if forced {
                BreakReason::Forced
            } else if overflow {
                BreakReason::Overflow
            } else if node.pos >= search.para.len {
                BreakReason::End
            } else {
                BreakReason::Opportunity
            };
            plan.push(Planned {
                slot: parent.slot,
                start: parent.pos,
                end: node.pos,
                reason,
                score: demerits,
                word_spacing,
                overflow,
            });
        }
        id = node.parent;
    }
    plan.reverse();
    plan
}

/// Places the plan, asking geometry again with the real previous lines. If
/// an answer differs from the ladder, the rest is set first-fit.
fn replay(
    para: &Para<'_, '_>,
    ladder: &Ladder,
    plan: &[Planned],
    complete: bool,
    out: &mut Composition,
    style: FirstFit,
) {
    let request = para.request;
    let lh = request.line_height;
    let mut walk = Walk::new(request.block_start);
    let mut cursor = 0;
    let mut pos = para.start;
    // Replays answers up to `upto`, or reports where they diverged.
    let mut check = |walk: &mut Walk, out: &Composition, upto: usize| -> Result<Option<Step>, ()> {
        let mut last = None;
        while cursor < upto {
            let real = walk.query(request.geometry, lh, &out.lines);
            if ladder.answers.get(cursor) != Some(&real) {
                return Err(());
            }
            cursor += 1;
            last = Some(walk.apply(real, lh));
        }
        Ok(last)
    };
    let used = match (complete, plan.last()) {
        (true, Some(p)) => ladder.slots.get(p.slot).map_or(0, |s| s.line + 1),
        _ => ladder.rungs.len(),
    };
    let mut planned = plan.iter().peekable();
    for (index, rung) in ladder.rungs.iter().enumerate().take(used) {
        if check(&mut walk, out, rung.answer + 1).is_err() {
            out.notes.push(fallback_note(
                "geometry answered differently once earlier lines were known",
            ));
            first_fit(para, &mut walk, pos, out, style);
            return;
        }
        while let Some(p) =
            planned.next_if(|p| ladder.slots.get(p.slot).is_some_and(|s| s.line == index))
        {
            let Some(slot) = ladder.slots.get(p.slot) else {
                continue;
            };
            let interval = if slot.k == 0 && !para.begins_line(p.start) {
                indented(slot.interval, style.indent)
            } else {
                slot.interval
            };
            if p.overflow {
                out.notes.push(
                    Note::warning(
                        codes::OVERFLOW,
                        format!("text at bytes {}..{} overflows its line", p.start, p.end),
                    )
                    .at(p.start..p.end),
                );
            }
            let adjustment = Adjustment {
                word_spacing: p.word_spacing,
                letter_spacing: Length::ZERO,
            };
            out.lines.push(para.fragment(
                p.start..p.end,
                interval,
                rung.line,
                rung.y,
                p.reason,
                Some(p.score),
                adjustment,
            ));
            pos = p.end;
        }
        walk.advance(lh);
    }
    if !complete {
        match check(&mut walk, out, ladder.answers.len()) {
            Ok(Some(Step::End)) => out.rest = Some(pos),
            Ok(Some(Step::Stalled)) => {
                out.notes.push(stalled(&walk, pos, para.len));
                out.rest = Some(pos);
            }
            Ok(_) => first_fit(para, &mut walk, pos, out, style),
            Err(()) => {
                out.notes.push(fallback_note(
                    "geometry answered differently once earlier lines were known",
                ));
                first_fit(para, &mut walk, pos, out, style);
                return;
            }
        }
    }
    out.block_end = walk.y;
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use super::*;
    use crate::testing::Shaped;
    use crate::{
        GeometryProvider, Greedy, LineQuery, Measure, Polygon, break_opportunities, is_word_space,
    };

    const PARAGRAPH: &str = "In olden times when wishing still helped one, there lived a king \
        whose daughters were all beautiful, but the youngest was so beautiful that the sun \
        itself, which has seen so much, was astonished whenever it shone in her face. Close \
        by the king's castle lay a great dark forest, and under an old lime-tree in the forest \
        was a well, and when the day was very warm, the king's child went out into the forest \
        and sat down by the side of the cool fountain.";

    fn pt(v: i32) -> Length {
        Length::from_pt(v)
    }

    /// The demerits the model gives each of `lines` in a plain measure,
    /// or `None` if one of them doesn't fit.
    fn model(
        cfg: &Optimal,
        shaped: &Shaped,
        lines: &[Range<usize>],
        measure: Length,
    ) -> Option<Vec<i64>> {
        let shaper = shaped.shaper();
        let text_shaped = shaper.shape();
        let breaks = break_opportunities(&shaped.text);
        let request = ComposeRequest {
            text: &shaped.text,
            shaped: &text_shaped,
            reshape: &shaper,
            breaks: &breaks,
            line_height: pt(12),
            geometry: &Measure(measure),
            start: 0,
            block_start: Length::ZERO,
        };
        let para = Para::new(&request);
        let mut previous = DECENT;
        let mut out = Vec::new();
        for line in lines {
            let j = para.breaks.iter().position(|b| b.at == line.end)?;
            let b = &para.breaks[j];
            let end = b.at == para.len;
            let last = end || b.kind == BreakKind::Forced;
            let natural = para.line_width(line.start, j).0 as i64;
            let fit = cfg.fit(natural, measure.0 as i64, last, para.spaces(line.start, j))?;
            out.push(cfg.demerits(fit.badness, fit.fitness, previous, b, end));
            previous = fit.fitness;
        }
        Some(out)
    }

    #[test]
    fn badness_is_a_hundred_r_cubed_rounded_and_capped() {
        assert_eq!(badness(0, 100), 0);
        assert_eq!(badness(-5, 100), 0);
        assert_eq!(badness(100, 100), 100);
        assert_eq!(badness(200, 100), 800);
        assert_eq!(badness(50, 100), 13, "12.5 rounds half away from zero");
        assert_eq!(badness(25, 100), 2, "1.5625 rounds to 2");
        assert_eq!(badness(464, 100), 9990);
        assert_eq!(badness(465, 100), INF_BAD);
        assert_eq!(badness(1, 0), INF_BAD);
        assert_eq!(badness(1 << 40, 1), INF_BAD);
        assert_eq!(badness(i64::MAX, 1), INF_BAD, "saturates, never overflows");
    }

    #[test]
    fn demerits_follow_the_documented_formula() {
        let cfg = Optimal::default();
        let allowed = |penalty| Break {
            at: 1,
            kind: BreakKind::Allowed,
            penalty,
        };
        let forced = Break {
            kind: BreakKind::Forced,
            ..allowed(5000)
        };
        assert_eq!(
            cfg.demerits(100, VERY_LOOSE, LOOSE, &allowed(0), false),
            110 * 110
        );
        assert_eq!(
            cfg.demerits(0, DECENT, DECENT, &allowed(50), false),
            100 + 2500
        );
        assert_eq!(
            cfg.demerits(0, DECENT, DECENT, &allowed(-50), false),
            100 - 2500
        );
        assert_eq!(cfg.demerits(0, DECENT, DECENT, &forced, false), 100);
        assert_eq!(
            cfg.demerits(0, DECENT, DECENT, &allowed(50), true),
            100,
            "the end of the text has no penalty"
        );
        assert_eq!(
            cfg.demerits(0, DECENT, DECENT, &allowed(i32::MAX), false),
            100 + MAX_PENALTY * MAX_PENALTY,
            "penalties clamp"
        );
        assert_eq!(
            cfg.demerits(150, VERY_LOOSE, DECENT, &allowed(0), false),
            160 * 160 + 10_000,
            "two fitness classes apart"
        );
        assert_eq!(
            cfg.demerits(20, TIGHT, VERY_LOOSE, &allowed(0), false),
            30 * 30 + 10_000
        );
        assert_eq!(cfg.pass_demerits(), 10_010 * 10_010);
    }

    fn ranges(c: &Composition) -> Vec<Range<usize>> {
        c.lines.iter().map(|l| l.text.clone()).collect()
    }

    #[test]
    fn optimal_beats_greedy_on_a_known_paragraph() {
        let shaped = Shaped::new(PARAGRAPH);
        let cfg = Optimal::default();
        let mut strictly_better = Vec::new();
        for measure in (100..=220).step_by(10).map(pt) {
            let greedy = shaped.compose(&Greedy, &Measure(measure));
            let optimal = shaped.compose(&cfg, &Measure(measure));
            assert!(optimal.notes.is_empty(), "{:?}", optimal.notes);
            assert_eq!(shaped.joined(&optimal), PARAGRAPH);
            // Each fragment's score is its line's demerits under the model.
            let scores: Vec<i64> = optimal
                .lines
                .iter()
                .map(|l| l.explanation.score.expect("scored"))
                .collect();
            assert_eq!(
                Some(scores.clone()),
                model(&cfg, &shaped, &ranges(&optimal), measure)
            );
            let ours: i64 = scores.iter().sum();
            let theirs: i64 = model(&cfg, &shaped, &ranges(&greedy), measure)
                .expect("greedy's lines fit")
                .iter()
                .sum();
            assert!(ours <= theirs, "{measure:?}: {ours} > greedy's {theirs}");
            if ours < theirs {
                strictly_better.push((measure, ours, theirs));
            }
        }
        assert!(strictly_better.len() >= 3, "{strictly_better:?}");
    }

    #[test]
    fn ragged_lines_have_no_adjustment_and_fit() {
        let shaped = Shaped::new(PARAGRAPH);
        let c = shaped.compose(&Optimal::default(), &Measure(pt(160)));
        for l in &c.lines {
            assert_eq!(l.explanation.adjustment, Adjustment::default());
            assert!(l.width <= l.available.width());
        }
        assert_eq!(
            c.lines.last().map(|l| l.explanation.reason),
            Some(BreakReason::End)
        );
        assert_eq!(Optimal::default().name(), "optimal");
    }

    #[test]
    fn justified_lines_fill_the_measure() {
        let shaped = Shaped::new(PARAGRAPH);
        let measure = pt(160);
        let c = shaped.compose(&Optimal::justified(), &Measure(measure));
        assert!(c.notes.is_empty());
        let (last, body) = c.lines.split_last().expect("lines");
        assert!(body.len() > 3);
        let mut stretched = 0;
        for l in body {
            let content = shaped.text[l.text.clone()].trim_end();
            let spaces = content.chars().filter(|&c| is_word_space(c)).count() as i64;
            assert!(spaces > 0, "{content:?}");
            let ws = l.explanation.adjustment.word_spacing.0 as i64;
            let set = l.width.0 as i64 + spaces * ws;
            assert!(
                (set - measure.0 as i64).abs() * 2 <= spaces,
                "{content:?} sets to {set}, not {}",
                measure.0
            );
            if ws > 0 {
                stretched += 1;
            }
        }
        assert!(stretched > 0);
        assert_eq!(
            last.explanation.adjustment,
            Adjustment::default(),
            "the last line is natural"
        );
        assert_eq!(Optimal::justified().name(), "optimal-justified");
    }

    #[test]
    fn justified_lines_may_shrink_within_the_limit() {
        // One line that is a little too long at its natural width.
        let text = "aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa";
        let shaped = Shaped::new(text);
        let natural = shaped.compose(&Greedy, &Measure(Length::MAX)).lines[0].width;
        let measure = natural - pt(1);
        let c = shaped.compose(&Optimal::justified(), &Measure(measure));
        assert_eq!(c.lines.len(), 1, "{:?}", shaped.lines(&c));
        let ws = c.lines[0].explanation.adjustment.word_spacing;
        assert!(ws < Length::ZERO);
        let set = c.lines[0].width + ws.mul_ratio(7, 1);
        assert!((set - measure).abs() <= Length(4), "{set:?} vs {measure:?}");
        // Ragged can't shrink, so it breaks.
        let ragged = shaped.compose(&Optimal::default(), &Measure(measure));
        assert_eq!(ragged.lines.len(), 2);
    }

    #[test]
    fn a_long_paragraph_in_a_measure_merges_its_tail_and_stays_small() {
        let text = std::iter::repeat_n(PARAGRAPH, 12)
            .collect::<Vec<_>>()
            .join(" ");
        let shaped = Shaped::new(&text);
        let cfg = Optimal {
            limits: Limits {
                max_active: 256,
                max_candidates: 1 << 20,
            },
            ..Optimal::default()
        };
        let c = shaped.compose(&cfg, &Measure(pt(200)));
        assert!(c.notes.is_empty(), "no fallback: {:?}", c.notes);
        assert!(c.lines.len() > 100);
        assert!(c.lines.iter().all(|l| l.explanation.score.is_some()));
    }

    #[test]
    fn hitting_a_limit_falls_back_to_first_fit_with_a_note() {
        let shaped = Shaped::new(PARAGRAPH);
        for limits in [
            Limits {
                max_active: 2,
                ..Limits::default()
            },
            Limits {
                max_candidates: 10,
                ..Limits::default()
            },
        ] {
            let cfg = Optimal {
                limits,
                ..Optimal::default()
            };
            let c = shaped.compose(&cfg, &Measure(pt(150)));
            let greedy = shaped.compose(&Greedy, &Measure(pt(150)));
            assert_eq!(c.notes.len(), 1);
            assert_eq!(c.notes[0].code, codes::FALLBACK);
            assert_eq!(c.notes[0].severity, reprise_diag::Severity::Info);
            assert!(c.lines.iter().all(|l| l.explanation.score.is_none()));
            assert_eq!(
                shaped.lines(&c),
                shaped.lines(&greedy),
                "first-fit is greedy's"
            );
        }
    }

    /// Narrower for every line already composed: only knowable on replay.
    struct Narrowing;

    impl GeometryProvider for Narrowing {
        fn available(&self, q: &LineQuery<'_>) -> Available {
            let n = q.previous.len() as i32;
            Available::Room(vec![Interval::new(Length::ZERO, pt(200 - 10 * n.min(5)))])
        }
    }

    #[test]
    fn geometry_that_depends_on_previous_lines_falls_back_where_it_diverges() {
        let shaped = Shaped::new(PARAGRAPH);
        let c = shaped.compose(&Optimal::default(), &Narrowing);
        let fallbacks = c.notes.iter().filter(|n| n.code == codes::FALLBACK);
        assert_eq!(fallbacks.count(), 1);
        // The first line was asked with no previous lines both times.
        assert!(c.lines[0].explanation.score.is_some());
        let unscored = c.lines.iter().position(|l| l.explanation.score.is_none());
        let unscored = unscored.expect("lines after the divergence are first-fit");
        assert!(
            c.lines[unscored..]
                .iter()
                .all(|l| l.explanation.score.is_none())
        );
        for l in &c.lines {
            assert!(l.width <= l.available.width());
        }
        assert_eq!(shaped.joined(&c), PARAGRAPH);
    }

    #[test]
    fn a_region_that_ends_is_filled_as_far_as_first_fit_fills_it() {
        let shaped = Shaped::new(PARAGRAPH);
        let circle = Polygon::circle(reprise_geom::Point::new(pt(70), pt(70)), pt(70), 16);
        let greedy = shaped.compose(&Greedy, &circle);
        let optimal = shaped.compose(&Optimal::default(), &circle);
        assert!(optimal.notes.is_empty(), "{:?}", optimal.notes);
        let (Some(g), Some(o)) = (greedy.rest, optimal.rest) else {
            panic!("the circle is too small for the paragraph");
        };
        assert!(o >= g, "optimal placed less: {o} < {g}");
        assert_eq!(optimal.block_end, greedy.block_end);
        for l in &optimal.lines {
            assert!(l.width <= l.available.width());
            assert!(l.explanation.score.is_some());
        }
        assert_eq!(optimal.lines.last().map(|l| l.text.end), Some(o));
    }

    #[test]
    fn empty_text_gets_one_scored_empty_line() {
        let shaped = Shaped::new("");
        let c = shaped.compose(&Optimal::default(), &Measure(pt(100)));
        assert_eq!(c.lines.len(), 1);
        assert_eq!(c.lines[0].explanation.score, Some(100));
        assert_eq!(c.lines[0].explanation.reason, BreakReason::End);
    }

    struct Split;

    impl GeometryProvider for Split {
        fn available(&self, _: &LineQuery<'_>) -> Available {
            Available::Room(vec![
                Interval::new(pt(0), pt(4)),
                Interval::new(pt(10), pt(80)),
                Interval::new(pt(90), pt(160)),
            ])
        }
    }

    #[test]
    fn several_intervals_are_filled_in_order_and_slivers_passed() {
        let shaped = Shaped::new(PARAGRAPH);
        let c = shaped.compose(&Optimal::default(), &Split);
        assert!(c.notes.is_empty(), "{:?}", c.notes);
        assert!(
            c.lines.iter().all(|l| l.available.start != Length::ZERO),
            "the sliver is passed"
        );
        assert_eq!(c.lines[0].line, c.lines[1].line);
        assert_eq!(shaped.joined(&c), PARAGRAPH);
    }

    #[test]
    fn overlong_words_overflow_once_and_forced_breaks_say_forced() {
        let text = "a Pneumonoultramicroscopicsilicovolcanoconiosis b\nc d";
        let shaped = Shaped::new(text);
        let c = shaped.compose(&Optimal::default(), &Measure(pt(60)));
        let overflows: Vec<_> = c
            .notes
            .iter()
            .filter(|n| n.code == codes::OVERFLOW)
            .collect();
        assert_eq!(overflows.len(), 1, "{:?}", c.notes);
        assert_eq!(
            shaped.lines(&c),
            [
                "a",
                "Pneumonoultramicroscopicsilicovolcanoconiosis",
                "b",
                "c d"
            ]
        );
        let reasons: Vec<_> = c.lines.iter().map(|l| l.explanation.reason).collect();
        assert_eq!(
            reasons,
            [
                BreakReason::Opportunity,
                BreakReason::Overflow,
                BreakReason::Forced,
                BreakReason::End
            ]
        );
    }
}
