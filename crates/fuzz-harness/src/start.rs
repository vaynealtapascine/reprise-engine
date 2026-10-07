//! Starting documents: synthetic ones built from a [`BaseSpec`], and hostile
//! fixtures that come back through the file format.

use std::cell::RefCell;

use reprise_doc::relation::builtin::{FLOAT, NOTE};
use reprise_doc::text::RangePolicy;
use reprise_doc::{
    BlockKind, Document, LengthExpr, NodeId, Param, PersistenceMode, RangeId, Relation,
    SchemaRegistry, Target,
};
use reprise_fixtures::templates::{responsive_columns, two_columns};
use reprise_format::{DocumentId, Limits, MigrationRegistry, Package};
use reprise_geom::Length;

use crate::authored::STYLE_POOL;
use crate::pool;
use crate::scenario::{BaseSpec, Start};

/// Peer IDs: the pinned pair every fixture uses.
pub const PEER_A: u64 = reprise_fixtures::PEER;
pub const PEER_B: u64 = reprise_fixtures::OTHER_PEER;

pub struct Started {
    pub docs: [Document; 2],
    /// Persistent ranges known to both peers, for relation targets.
    pub ranges: Vec<RangeId>,
}

pub fn start(start: &Start) -> Result<Started, String> {
    match start {
        Start::Synthetic(spec) => synthetic(spec),
        Start::Fixture(index) => fixture(*index),
    }
}

fn boundaries(text: &str) -> Vec<usize> {
    text.char_indices()
        .map(|(i, _)| i)
        .chain([text.len()])
        .collect()
}

fn policy(byte: u8) -> RangePolicy {
    match byte % 3 {
        0 => RangePolicy::EXPANDING,
        1 => RangePolicy::FIXED,
        _ => RangePolicy::POINT,
    }
}

fn synthetic(spec: &BaseSpec) -> Result<Started, String> {
    let doc = Document::new(PEER_A).map_err(|e| e.to_string())?;
    let fail = |e: reprise_doc::DocError| e.to_string();
    reprise_fixtures::spike::define_styles(&doc).map_err(fail)?;
    for &(name, variant, family, parent) in &spec.styles {
        let name = STYLE_POOL[usize::from(name) % STYLE_POOL.len()];
        // "body" and "note" keep their fixture definitions; the others are free.
        if name.is_empty() || name == "body" || name == "note" {
            continue;
        }
        let style = pool::style(
            usize::from(variant),
            usize::from(family),
            usize::from(parent),
        );
        doc.define_style(name, &style).map_err(fail)?;
    }
    let mut nodes: Vec<(NodeId, String)> = Vec::new();
    for block in &spec.blocks {
        let text = pool::text(usize::from(block.text));
        let kind = if block.annotation {
            BlockKind::Annotation
        } else {
            BlockKind::Paragraph
        };
        let style = pool::style_name(usize::from(block.style) % 3 + 1);
        let id = doc.append_block(kind, style, text).map_err(fail)?;
        if block.extra % 5 == 0 {
            let style = pool::style(
                usize::from(block.extra),
                usize::from(block.style),
                usize::from(block.text),
            );
            doc.set_overrides(id, &style).map_err(fail)?;
        }
        nodes.push((id, text.to_owned()));
    }
    let mut ranges = Vec::new();
    for range in &spec.ranges {
        let Some((node, text)) = nodes.get(usize::from(range.block) % nodes.len().max(1)) else {
            continue;
        };
        let bounds = boundaries(text);
        let a = bounds[usize::from(range.from) % bounds.len()];
        let b = bounds[usize::from(range.to) % bounds.len()];
        if let Ok(id) = doc.add_range(*node, a.min(b)..a.max(b), policy(range.policy)) {
            ranges.push(id);
        }
    }
    let schemas = SchemaRegistry::builtin();
    for relation in &spec.relations {
        let Some((owner, _)) = nodes.get(usize::from(relation.owner) % nodes.len().max(1)) else {
            continue;
        };
        let Some(&range) = ranges.get(usize::from(relation.range) % ranges.len().max(1)) else {
            continue;
        };
        let built = match relation.kind % 3 {
            0 => reprise_fixtures::spike::follow(*owner, range),
            1 => Relation::new(NOTE)
                .owned_by(*owner)
                .target("anchor", Target::Range(range)),
            _ => Relation::new(FLOAT)
                .owned_by(*owner)
                .target("anchor", Target::Range(range))
                .param(
                    "width",
                    Param::Length(LengthExpr::Pt(Length::from_pt(
                        20 + i32::from(relation.aux % 60),
                    ))),
                ),
        };
        // Schema violations are the editing kernel's to refuse; here they're
        // simply not part of the starting document.
        let _ = doc.add_relation(&schemas, &built);
    }
    match spec.template % 4 {
        0 => {}
        1 => doc.set_page_template(&two_columns()).map_err(fail)?,
        2 => doc.set_page_template(&responsive_columns()).map_err(fail)?,
        _ => doc
            .set_page_template(&reprise_doc::PageTemplate::builtin())
            .map_err(fail)?,
    }
    doc.commit();
    let other = doc.fork(PEER_B).map_err(fail)?;
    Ok(Started {
        docs: [doc, other],
        ranges,
    })
}

type Packages = Vec<(&'static str, Vec<u8>)>;

thread_local! {
    /// Each hostile fixture saved once as a package: a fixture's engine and
    /// document aren't `Send`, but its bytes are.
    static FIXTURES: RefCell<Option<Packages>> = const { RefCell::new(None) };
}

fn with_packages<T>(f: impl FnOnce(&Packages) -> T) -> Result<T, String> {
    FIXTURES.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let mut packages = Vec::new();
            for fixture in reprise_fixtures::hostile::all().map_err(|e| e.to_string())? {
                let package =
                    Package::new(&fixture.doc, DocumentId([9; 16]), PersistenceMode::History)
                        .map_err(|e| e.to_string())?;
                packages.push((fixture.name, package.save().map_err(|e| e.to_string())?));
            }
            *slot = Some(packages);
        }
        Ok(f(slot.as_ref().ok_or("fixtures missing")?))
    })
}

/// The number of hostile fixtures a scenario can start from.
pub fn fixture_count() -> usize {
    with_packages(Vec::len).unwrap_or(0)
}

/// A fixture's name, for reports.
pub fn fixture_name(index: u8) -> Option<&'static str> {
    with_packages(|all| all.get(usize::from(index) % all.len().max(1)).map(|p| p.0))
        .ok()
        .flatten()
}

fn fixture(index: u8) -> Result<Started, String> {
    let bytes = with_packages(|all| {
        all.get(usize::from(index) % all.len().max(1))
            .map(|p| p.1.clone())
    })?
    .ok_or("no hostile fixtures")?;
    let open = |peer| -> Result<Document, String> {
        Package::open(
            &bytes,
            peer,
            Limits::default(),
            &MigrationRegistry::builtin(),
        )
        .map_err(|e| e.to_string())?
        .into_document()
        .map_err(|e| e.to_string())
    };
    Ok(Started {
        docs: [open(PEER_A)?, open(PEER_B)?],
        ranges: Vec::new(),
    })
}
