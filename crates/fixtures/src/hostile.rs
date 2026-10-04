//! Hostile fixtures: documents built to break layout (37, 39).
//!
//! Each [`Fixture`] says which diagnostic codes layout must report for it.
//! `tests/hostile.rs` also checks, for every fixture, that layout doesn't
//! panic, is deterministic, covers every byte of text with lines, never splits
//! a grapheme cluster, and renders in every backend.
//!
//! When a workstream changes what a fixture reports, for example by adding
//! font fallback so emoji stop being `.notdef`, it updates `expect` here and
//! says why in its commit.

use reprise_compose::{AuthoredBreak, Optimal, Turnover};
use reprise_doc::relation::builtin::REFERENCE;
use reprise_doc::relation::{
    CopyCrossing, CopyInside, CopyPolicy, OnTargetDeleted, Ownership, RoleSpec,
};
use reprise_doc::text::RangePolicy;
use reprise_doc::{
    Authored, BlockKind, DocError, Document, Expr, LayoutQuery, LengthExpr, NodeId, Property,
    Relation, RelationSchema, Revision, SchemaId, SchemaRegistry, SnapshotOf, SnapshotRef,
    StructuralQuery, Style, Target, TargetClass,
};
use reprise_geom::Length;
use reprise_layout::{Engine, PageSettings};

use crate::spike::{define_styles, find, follow};
use crate::{OTHER_PEER, PEER, engine};

pub struct Fixture {
    pub name: &'static str,
    pub engine: Engine,
    pub doc: Document,
    /// A second replica, edited concurrently and merged, that must lay out
    /// identically to `doc`.
    pub replica: Option<Document>,
    /// Diagnostic codes layout must report, each at least once. Codes not
    /// listed may appear too, but only with severity info.
    pub expect: &'static [&'static str],
}

impl Fixture {
    fn new(name: &'static str, doc: Document, expect: &'static [&'static str]) -> Fixture {
        Fixture {
            name,
            engine: engine(),
            doc,
            replica: None,
            expect,
        }
    }
}

/// Every hostile fixture.
pub fn all() -> Result<Vec<Fixture>, DocError> {
    Ok(vec![
        empty_text()?,
        combining_marks()?,
        emoji_zwj()?,
        rtl_mixed()?,
        overlong_word()?,
        zero_width_measure()?,
        deleted_targets()?,
        concurrent_edits()?,
        extreme_lengths()?,
        bidi_stray_controls()?,
        bidi_override_ligature()?,
        scripts_common_inherited()?,
        display_text_clusters()?,
        structural_matches()?,
        snapshot_targets()?,
        snapshot_compacted()?,
        concurrent_policy_deletion()?,
        self_reference()?,
        optimal_paragraph()?,
        verse_turnover()?,
        optimal_extreme_lengths()?,
        style_expressions()?,
        style_cycles()?,
        style_bases()?,
    ])
}

fn document() -> Result<Document, DocError> {
    let doc = Document::new(PEER)?;
    define_styles(&doc)?;
    Ok(doc)
}

/// A paragraph followed by a note on `needle` in it.
fn paragraph_with_note(doc: &Document, text: &str, needle: &str) -> Result<(), DocError> {
    let p = doc.append_block(BlockKind::Paragraph, "body", text)?;
    let n = doc.append_block(BlockKind::Annotation, "note", "A note.")?;
    let r = doc.add_range(p, find(text, needle), RangePolicy::FIXED)?;
    doc.add_relation(&SchemaRegistry::builtin(), &follow(n, r))?;
    Ok(())
}

/// Empty and whitespace-only blocks, and a note anchored at a point in an
/// empty paragraph. Each still gets a line, so carets have somewhere to go.
pub fn empty_text() -> Result<Fixture, DocError> {
    let doc = document()?;
    let empty = doc.append_block(BlockKind::Paragraph, "body", "")?;
    doc.append_block(BlockKind::Paragraph, "body", "   ")?;
    let note = doc.append_block(BlockKind::Annotation, "note", "")?;
    let point = doc.add_range(empty, 0..0, RangePolicy::POINT)?;
    doc.add_relation(&SchemaRegistry::builtin(), &follow(note, point))?;
    doc.commit();
    Ok(Fixture::new("empty_text", doc, &[]))
}

/// Stacked combining marks, and a range that ends between a base letter and
/// its accent.
pub fn combining_marks() -> Result<Fixture, DocError> {
    let doc = document()?;
    let text = "Zalgo: Z\u{335}\u{322}\u{31b}\u{332}a\u{335}\u{322}l\u{338}g\u{334}o, then \
                e\u{301}e\u{301}e\u{301} and a\u{30a}\u{30a}\u{30a}\u{30a}\u{30a}\u{30a} \
                stacked over and over until the line has to wrap somewhere.";
    let p = doc.append_block(BlockKind::Paragraph, "body", text)?;
    let n = doc.append_block(BlockKind::Annotation, "note", "Mid-cluster.")?;
    let e = text.find("e\u{301}").expect("present");
    let r = doc.add_range(p, e..e + 1, RangePolicy::FIXED)?;
    doc.add_relation(&SchemaRegistry::builtin(), &follow(n, r))?;
    doc.commit();
    Ok(Fixture::new("combining_marks", doc, &[]))
}

/// Emoji ZWJ sequences, flags, skin tones and keycaps. The bundled font has
/// none of them.
pub fn emoji_zwj() -> Result<Fixture, DocError> {
    let doc = document()?;
    let text = "Family \u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466} and flags \
                \u{1F1EF}\u{1F1F5}\u{1F1EB}\u{1F1F7}, a thumb \u{1F44D}\u{1F3FD}, a keycap \
                1\u{FE0F}\u{20E3}, and a chain \
                \u{1F9D1}\u{200D}\u{1F91D}\u{200D}\u{1F9D1}\u{1F9D1}\u{200D}\u{1F91D}\u{200D}\u{1F9D1}\
                \u{1F9D1}\u{200D}\u{1F91D}\u{200D}\u{1F9D1} with no spaces.";
    paragraph_with_note(&doc, text, "a thumb")?;
    doc.commit();
    Ok(Fixture::new("emoji_zwj", doc, &[]))
}

/// Hebrew and Arabic mixed with English and numbers, with a note on the
/// Hebrew. The bundled font has no Hebrew or Arabic glyphs.
pub fn rtl_mixed() -> Result<Fixture, DocError> {
    let doc = document()?;
    let text = "English, then \u{5E2}\u{5D1}\u{5E8}\u{5D9}\u{5EA} \u{5E2}\u{5DD} 123 \
                \u{5D5}-456, then \u{627}\u{644}\u{639}\u{631}\u{628}\u{64A}\u{629} with \
                English inside it, then back to English.";
    paragraph_with_note(&doc, text, "\u{5E2}\u{5D1}\u{5E8}\u{5D9}\u{5EA}")?;
    doc.commit();
    Ok(Fixture::new("rtl_mixed", doc, &[]))
}

/// Words and a URL wider than the column.
pub fn overlong_word() -> Result<Fixture, DocError> {
    let doc = document()?;
    let text = "Short, then Pneumonoultramicroscopicsilicovolcanoconiosisandmore and \
                https://example.com/a/very/long/path/with/no/spaces/at/all/whatsoever/really \
                then short again.";
    paragraph_with_note(&doc, text, "short again")?;
    doc.commit();
    Ok(Fixture::new("overlong_word", doc, &["compose.overflow"]))
}

/// Columns of zero and negative width.
pub fn zero_width_measure() -> Result<Fixture, DocError> {
    let doc = document()?;
    paragraph_with_note(&doc, "a few short words", "short")?;
    doc.commit();
    let mut fixture = Fixture::new("zero_width_measure", doc, &["compose.overflow"]);
    fixture.engine.page = PageSettings {
        column_width: Length::ZERO,
        margin_column_width: Length::from_pt(-10),
        ..PageSettings::default()
    };
    Ok(fixture)
}

/// Every way a relation can lose what it points at.
pub fn deleted_targets() -> Result<Fixture, DocError> {
    let doc = document()?;
    let schemas = SchemaRegistry::builtin();

    // The target's text is deleted.
    let gone = "The first paragraph has a target phrase in it.";
    let p1 = doc.append_block(BlockKind::Paragraph, "body", gone)?;
    let n1 = doc.append_block(BlockKind::Annotation, "note", "Lost its text.")?;
    let r1 = doc.add_range(p1, find(gone, "target phrase"), RangePolicy::FIXED)?;
    doc.add_relation(&schemas, &follow(n1, r1))?;

    // The target loses its last letter: rebound, still placed.
    let shrunk = "The second paragraph loses a letter.";
    let p2 = doc.append_block(BlockKind::Paragraph, "body", shrunk)?;
    let n2 = doc.append_block(BlockKind::Annotation, "note", "Rebound.")?;
    let letter = find(shrunk, "letter");
    let r2 = doc.add_range(p2, letter.clone(), RangePolicy::FIXED)?;
    doc.add_relation(&schemas, &follow(n2, r2))?;

    // The target's whole block is deleted.
    let p3 = doc.append_block(BlockKind::Paragraph, "body", "Doomed paragraph.")?;
    let n3 = doc.append_block(BlockKind::Annotation, "note", "Lost its block.")?;
    let r3 = doc.add_range(p3, 0..6, RangePolicy::FIXED)?;
    doc.add_relation(&schemas, &follow(n3, r3))?;

    // The owner is deleted.
    let n4 = doc.append_block(BlockKind::Annotation, "note", "Deleted note.")?;
    let r4 = doc.add_range(p2, 0..3, RangePolicy::FIXED)?;
    doc.add_relation(&schemas, &follow(n4, r4))?;

    // A schema this engine doesn't know, written by one that did.
    let mut theirs = SchemaRegistry::builtin();
    theirs.register(RelationSchema {
        id: SchemaId::new("elsewhere.unknown"),
        version: 1,
        ownership: Ownership::Independent,
        roles: vec![RoleSpec {
            name: "a".into(),
            accepts: vec![TargetClass::Node].into(),
            min: 1,
            max: None,
        }],
        params: Vec::new(),
        on_target_deleted: OnTargetDeleted::Rebind,
        on_copy: CopyPolicy {
            inside: CopyInside::Duplicate,
            crossing: CopyCrossing::Drop,
        },
    })?;
    let unknown = Relation::new(SchemaId::new("elsewhere.unknown")).target("a", Target::Node(p1));
    doc.add_relation(&theirs, &unknown)?;

    // A relation whose owner is a paragraph, which no relation can place.
    let r5 = doc.add_range(p2, 4..10, RangePolicy::FIXED)?;
    doc.add_relation(&schemas, &follow(p2, r5))?;

    // A note nothing places.
    doc.append_block(BlockKind::Annotation, "note", "Orphan.")?;

    let t1 = doc.block(p1)?.text;
    let at = find(gone, "target phrase");
    t1.delete(at)?;
    doc.block(p2)?.text.delete(letter.end - 1..letter.end)?;
    doc.delete_block(p3)?;
    doc.delete_block(n4)?;
    doc.commit();
    Ok(Fixture::new(
        "deleted_targets",
        doc,
        &[
            "relation.missing-target",
            "relation.rebound",
            "relation.owner-not-placeable",
            "relation.owner-deleted",
            "relation.unknown-schema",
            "layout.unplaced",
        ],
    ))
}

/// Two peers edit the same range and note at once: one deletes the range's
/// text while the other types inside it; one deletes a note while the other
/// edits it. Both replicas must converge and lay out identically.
pub fn concurrent_edits() -> Result<Fixture, DocError> {
    let doc = document()?;
    let schemas = SchemaRegistry::builtin();
    let text = "Before the middle words after, and a second phrase to follow.";
    let p = doc.append_block(BlockKind::Paragraph, "body", text)?;
    let n1 = doc.append_block(BlockKind::Annotation, "note", "On the middle.")?;
    let n2 = doc.append_block(BlockKind::Annotation, "note", "On the second.")?;
    let middle = find(text, "the middle words");
    let r1 = doc.add_range(p, middle.clone(), RangePolicy::FIXED)?;
    let r2 = doc.add_range(p, find(text, "second phrase"), RangePolicy::FIXED)?;
    doc.add_relation(&schemas, &follow(n1, r1))?;
    doc.add_relation(&schemas, &follow(n2, r2))?;
    doc.commit();

    let other = doc.fork(OTHER_PEER)?;
    // Peer 1 deletes the range's text and edits the second note.
    doc.block(p)?.text.delete(middle.clone())?;
    doc.block(n2)?.text.insert(0, "Edited: ")?;
    // Peer 2 types inside the range and deletes the second note.
    let at = middle.start + "the middle".len();
    other.block(p)?.text.insert(at, " [typed]")?;
    other.delete_block(n2)?;

    doc.merge(&other)?;
    other.merge(&doc)?;
    let mut fixture = Fixture::new(
        "concurrent_edits",
        doc,
        &["relation.rebound", "relation.owner-deleted"],
    );
    fixture.replica = Some(other);
    Ok(fixture)
}

/// Sizes and line heights at the limits of the fixed-point range: enormous,
/// zero, negative, and a line too tall for any frame. Negative values are
/// clamped, so the normal paragraph after them still lands inside its frame.
pub fn extreme_lengths() -> Result<Fixture, DocError> {
    let doc = document()?;
    let styles = [
        ("huge", LengthExpr::Pt(Length::MAX), LengthExpr::Em(-5000)),
        ("zero", LengthExpr::Pt(Length::ZERO), LengthExpr::Em(1200)),
        (
            "negative",
            LengthExpr::Pt(Length::from_pt(-12)),
            LengthExpr::Em(i32::MAX),
        ),
        (
            "tall",
            LengthExpr::Pt(Length::from_pt(11)),
            LengthExpr::Pt(Length::MAX),
        ),
    ];
    for (name, size, line_height) in styles {
        doc.define_style(
            name,
            &Style {
                size: Some(size),
                line_height: Some(line_height),
                ..Default::default()
            },
        )?;
        doc.append_block(BlockKind::Paragraph, name, "Text at the edge of the range.")?;
    }
    doc.append_block(
        BlockKind::Paragraph,
        "body",
        "A normal paragraph after them.",
    )?;
    doc.commit();
    Ok(Fixture::new(
        "extreme_lengths",
        doc,
        &[
            "compose.overflow",
            "layout.frame-overflow",
            "layout.style-clamped",
        ],
    ))
}

/// Stray PDF/PDI, an unterminated isolate, and embedding depth beyond UAX #9's 125.
pub fn bidi_stray_controls() -> Result<Fixture, DocError> {
    let doc = document()?;
    doc.append_block(
        BlockKind::Paragraph,
        "body",
        "\u{202c}\u{2069}Stray controls, then \u{2067}an unterminated isolate with 123.",
    )?;
    let text = format!(
        "Before {}nested office{} after.",
        "\u{202b}".repeat(140),
        "\u{202c}".repeat(140)
    );
    doc.append_block(BlockKind::Paragraph, "body", &text)?;
    doc.commit();
    Ok(Fixture::new("bidi_stray_controls", doc, &[]))
}

/// Real odd-level Latin glyphs, including a ligature under an RLO override.
pub fn bidi_override_ligature() -> Result<Fixture, DocError> {
    let doc = document()?;
    paragraph_with_note(
        &doc,
        "Before \u{202e}office 123\u{202c}, then normal English after it.",
        "office",
    )?;
    doc.commit();
    Ok(Fixture::new("bidi_override_ligature", doc, &[]))
}

/// Common punctuation between scripts, nested pairs and inherited accents.
pub fn scripts_common_inherited() -> Result<Fixture, DocError> {
    let doc = document()?;
    doc.append_block(
        BlockKind::Paragraph,
        "body",
        "(Latin e\u{301}, [Ελληνικά α\u{301}] after), Кириллица и\u{301}; Latin again.",
    )?;
    doc.commit();
    Ok(Fixture::new("scripts_common_inherited", doc, &[]))
}

/// A single paragraph attacks selectable PDF text with a ligature, stacked
/// marks, an explicit right-to-left override and unsupported ZWJ emoji.
pub fn display_text_clusters() -> Result<Fixture, DocError> {
    let doc = document()?;
    doc.append_block(
        BlockKind::Paragraph,
        "body",
        "office Z\u{335}\u{322}\u{31b}\u{332} a\u{30a}\u{30a} \u{202e}abc office\u{202c} 👩‍👩‍👧‍👦 end.",
    )?;
    doc.commit();
    Ok(Fixture::new("display_text_clusters", doc, &[]))
}

/// A `reprise.reference` from `owner` to `target`.
fn reference(owner: NodeId, target: Target) -> Relation {
    Relation::new(REFERENCE)
        .owned_by(owner)
        .target("to", target)
}

fn next_sibling(from: NodeId, kind: Option<BlockKind>) -> Target {
    Target::Structural(StructuralQuery::NextSibling { from, kind })
}

/// Structural queries that match nothing, one block, and several; a node
/// deleted with two equally good successors; and one deleted with none.
pub fn structural_matches() -> Result<Fixture, DocError> {
    let doc = document()?;
    let schemas = SchemaRegistry::builtin();
    let first = doc.append_block(BlockKind::Paragraph, "body", "The first paragraph.")?;
    let second = doc.append_block(BlockKind::Paragraph, "body", "The second paragraph.")?;
    let last = doc.append_block(BlockKind::Paragraph, "body", "The last paragraph.")?;
    let lost = doc.append_block(BlockKind::Paragraph, "body", "Deleted without an heir.")?;
    let doomed = doc.append_block(BlockKind::Paragraph, "body", "Replaced by two halves.")?;
    let half_a = doc.append_block(BlockKind::Paragraph, "body", "Replaced by")?;
    let half_b = doc.append_block(BlockKind::Paragraph, "body", "two halves.")?;

    // One match: the paragraph after the first.
    doc.add_relation(&schemas, &reference(first, next_sibling(first, None)))?;
    // Zero matches: nothing follows the last block, and no block is an annotation.
    doc.add_relation(&schemas, &reference(second, next_sibling(half_b, None)))?;
    doc.add_relation(
        &schemas,
        &reference(second, next_sibling(second, Some(BlockKind::Annotation))),
    )?;
    // Several matches in a role that takes one: ambiguous, none chosen.
    let all = StructuralQuery::Children {
        of: None,
        kind: Some(BlockKind::Paragraph),
    };
    doc.add_relation(&schemas, &reference(last, Target::Structural(all)))?;
    // A deleted block split into two halves: two equally good successors.
    doc.add_relation(&schemas, &reference(first, Target::Node(doomed)))?;
    // A deleted block nothing replaced: nothing to rebind on.
    doc.add_relation(&schemas, &reference(first, Target::Node(lost)))?;

    doc.supersede(doomed, half_a)?;
    doc.supersede(doomed, half_b)?;
    doc.delete_block(doomed)?;
    doc.delete_block(lost)?;
    doc.commit();
    Ok(Fixture::new(
        "structural_matches",
        doc,
        &[
            "relation.no-match",
            "relation.ambiguous",
            "relation.missing-target",
        ],
    ))
}

/// Snapshot targets at a version that exists, at one before the subject
/// did, and at ones that don't exist or can't be: all fail soft.
pub fn snapshot_targets() -> Result<Fixture, DocError> {
    let doc = document()?;
    let schemas = SchemaRegistry::builtin();
    let owner = doc.append_block(BlockKind::Paragraph, "body", "The owner.")?;
    let quoted = doc.append_block(BlockKind::Paragraph, "body", "The original wording.")?;
    let range = doc.add_range(quoted, 4..12, RangePolicy::FIXED)?;
    doc.commit();
    let then = doc.revision();

    let later = doc.append_block(BlockKind::Paragraph, "body", "Written afterwards.")?;
    doc.block(quoted)?.text.delete(4..12)?;
    doc.block(quoted)?.text.insert(4, "revised")?;
    doc.commit();

    let snapshot = |version: &Revision, of| {
        Target::Snapshot(SnapshotRef {
            version: version.clone(),
            of,
        })
    };
    // Found: the text as it was, though it has changed since.
    let found = snapshot(&then, SnapshotOf::Node(quoted));
    doc.add_relation(&schemas, &reference(owner, found))?;
    let found = snapshot(&then, SnapshotOf::Range(range));
    doc.add_relation(&schemas, &reference(owner, found))?;
    // The subject was not written yet.
    let early = snapshot(&then, SnapshotOf::Node(later));
    doc.add_relation(&schemas, &reference(owner, early))?;
    // The empty version: before anything.
    let empty = snapshot(&Revision(vec![]), SnapshotOf::Node(owner));
    doc.add_relation(&schemas, &reference(owner, empty))?;
    // Versions that don't exist, and ones that are not versions.
    for version in [
        Revision(vec![(PEER, i32::MAX)]),
        Revision(vec![(OTHER_PEER + 40, 3)]),
        Revision(vec![(PEER, -1)]),
        Revision(vec![(PEER, 2), (PEER, 1)]),
    ] {
        let missing = snapshot(&version, SnapshotOf::Node(quoted));
        doc.add_relation(&schemas, &reference(owner, missing))?;
    }
    doc.commit();
    Ok(Fixture::new(
        "snapshot_targets",
        doc,
        &["relation.no-match", "relation.snapshot-unavailable"],
    ))
}

/// A snapshot target whose version was compacted away (07).
pub fn snapshot_compacted() -> Result<Fixture, DocError> {
    let schemas = SchemaRegistry::builtin();
    let doc = document()?;
    let owner = doc.append_block(BlockKind::Paragraph, "body", "The owner.")?;
    let quoted = doc.append_block(BlockKind::Paragraph, "body", "Before compaction.")?;
    doc.commit();
    let old = doc.revision();
    doc.block(quoted)?.text.insert(0, "Edited. ")?;
    doc.commit();
    let kept = doc.revision();
    doc.block(quoted)?.text.insert(0, "Edited again. ")?;
    doc.commit();

    // The compacted replica is the document from here on.
    let doc = doc.compact_history(&kept, PEER)?;
    let at = |version: &Revision| {
        reference(
            owner,
            Target::Snapshot(SnapshotRef {
                version: version.clone(),
                of: SnapshotOf::Node(quoted),
            }),
        )
    };
    doc.add_relation(&schemas, &at(&old))?;
    doc.add_relation(&schemas, &at(&kept))?;
    doc.commit();
    Ok(Fixture::new(
        "snapshot_compacted",
        doc,
        &["relation.snapshot-unavailable"],
    ))
}

/// A relation schema with an `OnTargetDeleted` policy and no layout
/// behaviour: layout resolves and reports it.
fn policy_schema(id: &'static str, on_target_deleted: OnTargetDeleted) -> RelationSchema {
    RelationSchema {
        id: SchemaId::new(id),
        version: 1,
        ownership: Ownership::Owned,
        roles: vec![RoleSpec {
            name: "to".into(),
            accepts: vec![TargetClass::Node].into(),
            min: 1,
            max: Some(1),
        }],
        params: Vec::new(),
        on_target_deleted,
        on_copy: CopyPolicy {
            inside: CopyInside::Duplicate,
            crossing: CopyCrossing::KeepOutside,
        },
    }
}

/// Registers the two policy schemas that `reprise.reference` doesn't cover:
/// `fixtures.keep` (`KeepMissing`) and `fixtures.delete` (`Delete`).
pub fn policy_schemas(schemas: &mut SchemaRegistry) -> Result<(), DocError> {
    schemas.register(policy_schema("fixtures.keep", OnTargetDeleted::KeepMissing))?;
    schemas.register(policy_schema("fixtures.delete", OnTargetDeleted::Delete))?;
    Ok(())
}

/// Targets deleted by one peer while the other works. One relation of each
/// deletion policy: rebind (to a successor the other peer recorded at the
/// same time), keep missing, and delete. Both replicas must agree.
pub fn concurrent_policy_deletion() -> Result<Fixture, DocError> {
    let mut engine = engine();
    policy_schemas(&mut engine.schemas)?;
    let doc = document()?;
    let owner = doc.append_block(
        BlockKind::Paragraph,
        "body",
        "The owner of three relations.",
    )?;
    let rebinds = doc.append_block(BlockKind::Paragraph, "body", "A target that gets replaced.")?;
    let keeps = doc.append_block(
        BlockKind::Paragraph,
        "body",
        "A target that is only missed.",
    )?;
    let deletes = doc.append_block(
        BlockKind::Paragraph,
        "body",
        "A target that takes its relation.",
    )?;
    let heir = doc.append_block(BlockKind::Paragraph, "body", "The replacement.")?;
    doc.add_relation(&engine.schemas, &reference(owner, Target::Node(rebinds)))?;
    for (schema, target) in [("fixtures.keep", keeps), ("fixtures.delete", deletes)] {
        let relation = Relation::new(SchemaId::new(schema))
            .owned_by(owner)
            .target("to", Target::Node(target));
        doc.add_relation(&engine.schemas, &relation)?;
    }
    doc.commit();

    let other = doc.fork(OTHER_PEER)?;
    // Peer 1 replaces the first target, and keeps writing.
    doc.supersede(rebinds, heir)?;
    doc.block(heir)?.text.insert(0, "Edited: ")?;
    // Peer 2 deletes all three targets.
    for node in [rebinds, keeps, deletes] {
        other.delete_block(node)?;
    }
    doc.merge(&other)?;
    other.merge(&doc)?;
    let mut fixture = Fixture::new(
        "concurrent_policy_deletion",
        doc,
        &[
            "relation.rebound",
            "relation.missing-target",
            "relation.target-deleted",
            "relation.not-applied",
        ],
    );
    fixture.engine = engine;
    fixture.replica = Some(other);
    Ok(fixture)
}

/// Relations whose structural or layout query resolves to their own owner.
pub fn self_reference() -> Result<Fixture, DocError> {
    let doc = document()?;
    let schemas = SchemaRegistry::builtin();
    let text = "A paragraph that refers to itself, and then to another.";
    let first = doc.append_block(BlockKind::Paragraph, "body", text)?;
    let second = doc.append_block(BlockKind::Paragraph, "body", "The second paragraph.")?;
    let note = doc.append_block(BlockKind::Annotation, "note", "A note.")?;
    let on = doc.add_range(first, 0..11, RangePolicy::FIXED)?;
    doc.add_relation(&schemas, &follow(note, on))?;

    // The next block after the one before the owner is the owner.
    doc.add_relation(&schemas, &reference(second, next_sibling(first, None)))?;
    // The first block is the owner.
    let nth = StructuralQuery::NthChild {
        of: None,
        index: 0,
        from_end: false,
        kind: None,
    };
    doc.add_relation(&schemas, &reference(first, Target::Structural(nth)))?;
    // The last annotation is the owner, which is an annotation.
    let last = StructuralQuery::LastChild {
        of: None,
        kind: Some(BlockKind::Annotation),
    };
    doc.add_relation(&schemas, &reference(note, Target::Structural(last)))?;
    // A node targeting itself, and a layout query for its own first line.
    doc.add_relation(&schemas, &reference(first, Target::Node(first)))?;
    let own_line = Target::Layout(LayoutQuery::FirstLine { node: first });
    doc.add_relation(&schemas, &reference(first, own_line))?;
    // Its own parent: a top-level block has none.
    let parent = Target::Structural(StructuralQuery::Parent { of: first });
    doc.add_relation(&schemas, &reference(first, parent))?;
    doc.commit();
    Ok(Fixture::new(
        "self_reference",
        doc,
        &["relation.self-reference", "relation.no-match"],
    ))
}

/// The optimal composer on a paragraph with an overlong word, a forced line
/// break (U+2028) and a URL: the overlong word overflows once and is
/// reported, the forced break ends its line, and every line is scored.
pub fn optimal_paragraph() -> Result<Fixture, DocError> {
    let doc = document()?;
    let text = "The optimal composer weighs every line of a paragraph at once, so a \
                Pneumonoultramicroscopicsilicovolcanoconiosis has to overflow on a line of \
                its own while the lines around it stay even.\u{2028}After the forced break \
                the paragraph goes on, past https://example.com/a/long/path/to/somewhere, \
                to a short last line.";
    paragraph_with_note(&doc, text, "forced break")?;
    doc.commit();
    let mut fixture = Fixture::new("optimal_paragraph", doc, &["compose.overflow"]);
    fixture.engine.composer = Box::new(Optimal::default());
    Ok(fixture)
}

/// Verse with the authored-break composer in a narrow column: each authored
/// line ends at its forced break, and the long ones turn over with a hanging
/// indent. The margin note is composed the same way.
pub fn verse_turnover() -> Result<Fixture, DocError> {
    let doc = document()?;
    let text = "Whose woods these are I think I know.\u{2028}\
                His house is in the village though;\u{2028}\
                He will not see me stopping here\u{2028}\
                To watch his woods fill up with snow.\u{2028}\
                \u{2028}\
                My little horse must think it queer";
    paragraph_with_note(&doc, text, "village")?;
    doc.commit();
    let mut fixture = Fixture::new("verse_turnover", doc, &[]);
    fixture.engine.composer = Box::new(AuthoredBreak {
        turnover_indent: Length::from_pt(14),
        turnover: Turnover::Optimal(Optimal::default()),
    });
    fixture.engine.page = PageSettings {
        column_width: Length::from_pt(110),
        ..PageSettings::default()
    };
    Ok(fixture)
}

/// [`extreme_lengths`] with the justified optimal composer: sizes and line
/// heights at the limits of the fixed-point range must not overflow its
/// demerits or its search.
pub fn optimal_extreme_lengths() -> Result<Fixture, DocError> {
    let mut fixture = extreme_lengths()?;
    fixture.name = "optimal_extreme_lengths";
    fixture.engine.composer = Box::new(Optimal::justified());
    Ok(fixture)
}

fn expr(text: &str) -> Result<Authored, DocError> {
    Expr::parse(text)
        .map(Authored::Expr)
        .map_err(|e| DocError::Store(format!("fixture expression {text:?}: {e}")))
}

/// Defines a style whose `size` is `value` and lays out a paragraph in it.
fn paragraph_sized(doc: &Document, name: &str, value: Authored) -> Result<(), DocError> {
    let mut style = Style::default();
    style.set(Property::Size, value);
    doc.define_style(name, &style)?;
    doc.append_block(BlockKind::Paragraph, name, "Under a hostile style.")?;
    Ok(())
}

/// Style values that are too big, mistyped, unknown, from a newer format, or
/// damaged (17, 34). Each style falls back to the value it inherited and the
/// paragraph is laid out regardless. The `style.*` notes are reported by
/// `Document::computed_style`; layout does not publish them yet, so `expect`
/// is empty until the wiring lands and `tests/hostile.rs` checks the notes.
pub fn style_expressions() -> Result<Fixture, DocError> {
    let doc = document()?;
    let deep = format!("expr1:{}1pt{}", "(".repeat(64), ")".repeat(64));
    let wide = format!("expr1:{}", vec!["1pt"; 300].join(" + "));
    paragraph_sized(&doc, "deep", Authored::Unparsed(deep))?;
    paragraph_sized(&doc, "wide", Authored::Unparsed(wide))?;
    paragraph_sized(&doc, "type-error", expr("1pt * 2pt")?)?;
    paragraph_sized(&doc, "not-a-length", expr("2")?)?;
    paragraph_sized(&doc, "unknown-function", expr("frob(12pt) + 1em")?)?;
    paragraph_sized(&doc, "function-fails", expr("scale(1em, ratio(1, 0))")?)?;
    paragraph_sized(
        &doc,
        "from-the-future",
        Authored::Unparsed("expr2:quantum(3)".into()),
    )?;
    paragraph_sized(&doc, "damaged", Authored::Unparsed("furlongs:5".into()))?;
    doc.commit();
    Ok(Fixture::new("style_expressions", doc, &[]))
}

/// A font size in `lh`, a line height in `lh`, and style chains that loop or
/// lead nowhere (18). Cycles are cut and reported, never followed forever.
pub fn style_cycles() -> Result<Fixture, DocError> {
    let doc = document()?;
    paragraph_sized(&doc, "size-in-lh", expr("2lh")?)?;
    let mut style = Style::default();
    style.set(Property::LineHeight, expr("3lh")?);
    doc.define_style("line-height-in-lh", &style)?;
    doc.append_block(
        BlockKind::Paragraph,
        "line-height-in-lh",
        "Under a hostile style.",
    )?;
    for (name, parent) in [("loop-a", "loop-b"), ("loop-b", "loop-a"), ("self", "self")] {
        doc.define_style(
            name,
            &Style {
                parent: Some(parent.into()),
                size: Some(LengthExpr::Pt(Length::from_pt(9))),
                ..Default::default()
            },
        )?;
    }
    doc.define_style(
        "orphan",
        &Style {
            parent: Some("nobody".into()),
            ..Default::default()
        },
    )?;
    for name in ["loop-a", "self", "orphan", "undefined"] {
        doc.append_block(BlockKind::Paragraph, name, "Under a hostile style.")?;
    }
    doc.commit();
    Ok(Fixture::new("style_cycles", doc, &[]))
}

/// Percentages and references whose basis is missing or depends on its own
/// content, such as the height of an auto-height frame (18). `tests/hostile.rs`
/// resolves it both in the default context, where every basis is unresolved,
/// and in a context where the main frame is auto-height.
pub fn style_bases() -> Result<Fixture, DocError> {
    let doc = document()?;
    paragraph_sized(&doc, "half-the-height", expr("50% * frame-height")?)?;
    paragraph_sized(&doc, "named-frame", expr("10% * frame-width(\"margin\")")?)?;
    paragraph_sized(
        &doc,
        "no-such-frame",
        expr("frame-width(\"nowhere\") / 20")?,
    )?;
    // These need no frame and are fine in every context.
    paragraph_sized(&doc, "of-the-inherited-size", expr("120%")?)?;
    let mut style = Style::default();
    style.set(Property::LineHeight, expr("150%")?);
    doc.define_style("line-height-percent", &style)?;
    doc.append_block(
        BlockKind::Paragraph,
        "line-height-percent",
        "Under a hostile style.",
    )?;
    doc.commit();
    Ok(Fixture::new("style_bases", doc, &[]))
}
