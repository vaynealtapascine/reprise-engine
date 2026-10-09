//! A declared domain of at most 256 line translations. No line is rebroken in
//! this pass. Acyclic dependencies settle in at most 256 rounds; cycles and
//! their dependents retain the complete pre-pin geometry with a diagnostic.
use std::collections::{BTreeMap, BTreeSet};

use reprise_diag::Severity;
use reprise_doc::{
    Document, Param, RelationId,
    marks::{ALIGNMENT, Alignment},
};
use reprise_geom::{FrameSpace, Length, Point};

use super::resolve::Resolver;
use crate::marks::{AnchorGuide, edges, position};
use crate::{
    Diagnostic, Engine, LayoutSnapshot, LineRef, RelationLayout, RelationStatus, Resolution,
    Subject, codes,
};

pub const MAX_ALIGNMENT_DOMAIN: usize = 256;
#[derive(Clone)]
struct Pin {
    id: RelationId,
    source: LineRef,
    source_x: Length,
    target: LineRef,
    target_point: Point<FrameSpace>,
    source_end: bool,
    target_offset: usize,
    target_edge: String,
}

fn target(snapshot: &LayoutSnapshot, resolved: &Resolution) -> Option<(LineRef, usize)> {
    match resolved {
        Resolution::Range { node, bytes } => snapshot
            .line_containing(*node, bytes.start)
            .map(|l| (l, bytes.start)),
        Resolution::Line(l) => snapshot.line(*l).map(|v| (*l, v.text.start)),
        Resolution::Lines(lines) if lines.len() == 1 => lines
            .first()
            .and_then(|l| snapshot.line(*l).map(|v| (*l, v.text.start))),
        _ => None,
    }
}
fn keyword<'a>(params: &'a BTreeMap<String, Param>, key: &str, default: &'a str) -> &'a str {
    match params.get(key) {
        Some(Param::Text(t)) => t,
        _ => default,
    }
}
fn note(snapshot: &mut LayoutSnapshot, id: RelationId, code: reprise_diag::Code, message: &str) {
    snapshot.diagnostics.push(Diagnostic::new(
        Severity::Warning,
        code,
        Subject::Relation(id),
        message,
    ));
}
fn translate(snapshot: &mut LayoutSnapshot, at: LineRef, shift: Length, narrow: bool) {
    if let Some(block) = snapshot.blocks.iter_mut().find(|b| b.node == at.node)
        && let Some(line) = block.lines.get_mut(at.line)
    {
        line.rect.origin.x += shift;
        for run in &mut line.runs {
            run.x += shift;
        }
        if narrow {
            line.rect.width = line.width;
        }
    }
}

pub(crate) fn run(engine: &Engine, doc: &Document, snapshot: &mut LayoutSnapshot) {
    let Some(schema) = engine.schemas.get(&ALIGNMENT) else {
        return;
    };
    let mut resolver = Resolver::new(doc);
    let mut pins = Vec::new();
    let mut labels = BTreeMap::new();
    for (id, relation) in doc.relations() {
        let Ok(relation) = relation else {
            continue;
        };
        if relation.schema != ALIGNMENT {
            continue;
        }
        let mut result = RelationLayout {
            anchor: None,
            id,
            schema: relation.schema.clone(),
            owner: relation.owner,
            status: RelationStatus::Valid,
            applied: false,
            targets: Vec::new(),
        };
        if relation.owner.is_some_and(|n| !doc.is_live(n)) {
            result.status = RelationStatus::OwnerDeleted;
            snapshot.diagnostics.push(Diagnostic::new(
                Severity::Info,
                codes::RELATION_OWNER_DELETED,
                Subject::Relation(id),
                "alignment owner was deleted",
            ));
            snapshot.relations.push(result);
            continue;
        }
        if engine.schemas.validate(&relation).is_err() {
            note(
                snapshot,
                id,
                codes::ALIGNMENT_INVALID,
                "alignment does not match its schema; retained without application",
            );
            result.status = RelationStatus::Missing;
            snapshot.relations.push(result);
            continue;
        }
        let resolved = resolver.resolve(snapshot, id, schema, &relation);
        let source = resolved.unique("line").and_then(|r| target(snapshot, r));
        let to = resolved.unique("to").and_then(|r| target(snapshot, r));
        result.status = resolved
            .targets
            .iter()
            .map(|t| t.status)
            .max()
            .unwrap_or(RelationStatus::Missing);
        result.targets = resolved.targets;
        snapshot.diagnostics.extend(resolved.diagnostics);
        if let Some((source, at)) = source {
            let has_to = relation.first("to").is_some();
            if !has_to {
                if let Some(a) = Alignment::parse(keyword(&relation.params, "alignment", "")) {
                    labels.insert(source, (id, a));
                } else {
                    note(
                        snapshot,
                        id,
                        codes::ALIGNMENT_INVALID,
                        "line alignment needs start, centre or end",
                    );
                }
            } else {
                let from = snapshot
                    .block(source.node)
                    .zip(snapshot.line(source))
                    .map(|(b, l)| {
                        let (start, end) = edges(b, l);
                        Point::new(
                            if keyword(&relation.params, "edge", "start") == "end" {
                                end
                            } else {
                                start
                            },
                            l.baseline,
                        )
                    });
                let valid_edge =
                    matches!(keyword(&relation.params, "edge", "start"), "start" | "end");
                let target_point = to.and_then(|(to, offset)| {
                    let block = snapshot.block(to.node)?;
                    let line = snapshot.line(to)?;
                    let edge = keyword(&relation.params, "target-edge", "position");
                    let x = match edge {
                        "position" => position(block, line, offset),
                        "gap-start" | "gap-end"
                            if block
                                .text
                                .get(offset..)
                                .is_some_and(|s| s.starts_with('\t')) =>
                        {
                            position(
                                block,
                                line,
                                offset.saturating_add(usize::from(edge == "gap-end")),
                            )
                        }
                        "line-start" => edges(block, line).0,
                        "line-end" => edges(block, line).1,
                        _ => return None,
                    };
                    Some((to, Point::new(x, line.baseline)))
                });
                if let Some(from) = from {
                    result.anchor = Some(AnchorGuide {
                        source,
                        offset: at,
                        from,
                        target: target_point,
                    });
                    if let Some((target, target_point)) = target_point.filter(|_| valid_edge) {
                        pins.push(Pin {
                            id,
                            source,
                            source_x: from.x,
                            target,
                            target_point,
                            source_end: keyword(&relation.params, "edge", "start") == "end",
                            target_offset: to.map_or(0, |(_, at)| at),
                            target_edge: keyword(&relation.params, "target-edge", "position")
                                .into(),
                        });
                    } else if to.is_some() || !valid_edge {
                        note(
                            snapshot,
                            id,
                            codes::ALIGNMENT_INVALID,
                            "anchor edge is unreadable or its gap has gone",
                        );
                        result.status = RelationStatus::Missing;
                    }
                }
            }
        }
        snapshot.relations.push(result);
    }
    // Relation ID order chooses between concurrent defaults, as formatting does.
    for (at, (id, alignment)) in labels {
        let shift = snapshot
            .block(at.node)
            .zip(snapshot.line(at))
            .map(|(b, l)| {
                let surplus = (l.available.width() - l.width).max(Length::ZERO);
                let offset = match alignment {
                    Alignment::Centre => surplus.mul_ratio(1, 2),
                    Alignment::End if b.base_level % 2 == 0 => surplus,
                    Alignment::Start if b.base_level % 2 == 1 => surplus,
                    _ => Length::ZERO,
                };
                l.available.start + offset - edges(b, l).0.min(edges(b, l).1)
            })
            .unwrap_or_default();
        translate(snapshot, at, shift, true);
        if let Some(block) = snapshot.blocks.iter_mut().find(|b| b.node == at.node)
            && let Some(line) = block.lines.get_mut(at.line)
        {
            line.alignment = Some(alignment);
        }
        if let Some(r) = snapshot.relations.iter_mut().find(|r| r.id == id) {
            r.applied = true;
        }
    }
    // Recompute endpoints after defaults so pins follow aligned targets.
    for pin in &mut pins {
        if let Some((b, l)) = snapshot
            .block(pin.source.node)
            .zip(snapshot.line(pin.source))
        {
            pin.source_x = if pin.source_end {
                edges(b, l).1
            } else {
                edges(b, l).0
            };
        }
        if let Some((b, l)) = snapshot
            .block(pin.target.node)
            .zip(snapshot.line(pin.target))
        {
            pin.target_point.x = match pin.target_edge.as_str() {
                "gap-end" => position(b, l, pin.target_offset.saturating_add(1)),
                "line-start" => edges(b, l).0,
                "line-end" => edges(b, l).1,
                _ => position(b, l, pin.target_offset),
            };
        }
    }
    let mut conflicts = BTreeSet::new();
    let mut unique = BTreeMap::new();
    for pin in &pins {
        if unique.insert(pin.source, pin.clone()).is_some() {
            conflicts.insert(pin.source);
        }
    }
    for pin in &pins {
        if conflicts.contains(&pin.source) {
            unique.remove(&pin.source);
            if let Some(r) = snapshot.relations.iter_mut().find(|r| r.id == pin.id) {
                r.status = RelationStatus::Ambiguous;
            }
            note(
                snapshot,
                pin.id,
                codes::ALIGNMENT_CONFLICT,
                "several pins constrain this line; none chosen",
            );
        }
    }
    if unique.len() > MAX_ALIGNMENT_DOMAIN {
        for pin in unique.values() {
            note(
                snapshot,
                pin.id,
                codes::ALIGNMENT_LIMIT,
                "alignment domain exceeds 256 constrained lines; all pins retain pre-pin geometry",
            );
        }
        refresh_guides(doc, snapshot);
        return;
    }
    let mut shifts = BTreeMap::new();
    for _ in 0..MAX_ALIGNMENT_DOMAIN {
        let mut progress = false;
        for (&source, pin) in &unique {
            if shifts.contains_key(&source) {
                continue;
            }
            let target_shift = if unique.contains_key(&pin.target) {
                let Some(&s) = shifts.get(&pin.target) else {
                    continue;
                };
                s
            } else {
                Length::ZERO
            };
            let desired = snapshot
                .line(source)
                .and_then(|l| snapshot.frame(l.frame))
                .zip(
                    snapshot
                        .line(pin.target)
                        .and_then(|l| snapshot.frame(l.frame)),
                )
                .and_then(|(source_frame, target_frame)| {
                    let inverse = source_frame.to_page.inverse()?;
                    let page = target_frame.to_page.apply(Point::new(
                        pin.target_point.x + target_shift,
                        pin.target_point.y,
                    ));
                    Some(inverse.apply(page).x - pin.source_x)
                });
            if let Some(shift) = desired {
                shifts.insert(source, shift);
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }
    for (&source, pin) in &unique {
        if let Some(&shift) = shifts.get(&source) {
            translate(snapshot, source, shift, false);
            if let Some(r) = snapshot.relations.iter_mut().find(|r| r.id == pin.id) {
                r.applied = true;
            }
            if snapshot
                .block(source.node)
                .zip(snapshot.line(source))
                .is_some_and(|(b, l)| {
                    let (a, z) = edges(b, l);
                    a.min(z) < l.available.start || a.max(z) > l.available.end
                })
            {
                note(
                    snapshot,
                    pin.id,
                    codes::ALIGNMENT_OUTSIDE,
                    "pinned line overflows its composing interval",
                );
            }
        } else {
            note(
                snapshot,
                pin.id,
                codes::ALIGNMENT_CYCLE,
                "cyclic, dependent or noninvertible pin retains pre-pin geometry (256-round domain)",
            );
        }
    }
    refresh_guides(doc, snapshot);
}

fn refresh_guides(doc: &Document, snapshot: &mut LayoutSnapshot) {
    let relations: BTreeMap<_, _> = doc.relations().into_iter().collect();
    let guides: Vec<_> = snapshot
        .relations
        .iter()
        .filter_map(|result| {
            let guide = result.anchor.as_ref()?;
            let r = relations.get(&result.id)?.as_ref().ok()?;
            let (b, l) = snapshot
                .block(guide.source.node)
                .zip(snapshot.line(guide.source))?;
            let x = if keyword(&r.params, "edge", "start") == "end" {
                edges(b, l).1
            } else {
                edges(b, l).0
            };
            let mut updated = guide.clone();
            updated.from = Point::new(x, l.baseline);
            updated.target = guide.target.and_then(|(at, _)| {
                let (b, l) = snapshot.block(at.node).zip(snapshot.line(at))?;
                let offset = result
                    .targets
                    .iter()
                    .find(|t| t.role == "to")
                    .and_then(|t| match &t.resolved {
                        Some(Resolution::Range { bytes, .. }) => Some(bytes.start),
                        _ => None,
                    })
                    .unwrap_or(l.text.start);
                let x = match keyword(&r.params, "target-edge", "position") {
                    "gap-end" => position(b, l, offset.saturating_add(1)),
                    "line-start" => edges(b, l).0,
                    "line-end" => edges(b, l).1,
                    _ => position(b, l, offset),
                };
                Some((at, Point::new(x, l.baseline)))
            });
            Some((result.id, updated))
        })
        .collect();
    for (id, guide) in guides {
        if let Some(r) = snapshot.relations.iter_mut().find(|r| r.id == id) {
            r.anchor = Some(guide);
        }
    }
}
