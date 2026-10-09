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
    Authored, BlockKind, Dim, DocError, Document, Expr, FrameRole, FrameTemplate, LayoutQuery,
    LengthExpr, NodeId, PageTemplate, Property, Relation, RelationSchema, Revision, SchemaId,
    SchemaRegistry, SnapshotOf, SnapshotRef, StructuralQuery, Style, Target, TargetClass,
};
use reprise_geom::Length;
use reprise_layout::Engine;

use crate::spike::{define_styles, find, follow};
use crate::templates::{flow_frame, long_text, margin_frame, two_columns};
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
        frame_shorter_than_a_line()?,
        no_main_flow()?,
        negative_page_size()?,
        zero_sized_frames()?,
        page_limit()?,
        unreadable_template()?,
        column_storm()?,
        concurrent_templates()?,
        no_margin_frame()?,
        bidi_line_override()?,
        follow_lines_across_frames()?,
        style_fragment_basis()?,
        justified_bidi()?,
        persistence_tombstones()?,
        transformed_rtl()?,
        vertical_rl()?,
        spiral_text()?,
        reading_cycle()?,
        degenerate_transform()?,
        rational_rotation_extreme()?,
        float_wider_than_frame()?,
        float_moves_its_anchor()?,
        note_taller_than_page()?,
        notes_nested_three_deep()?,
        note_on_last_line()?,
        notes_take_over_page()?,
        table_zero_columns()?,
        table_conflicting_widths()?,
        table_row_taller_than_page()?,
        infeasible_solver_domain()?,
        incremental_page_seam()?,
        editing_concurrent_delete_undo()?,
        editing_half_invalid()?,
        editing_empty_document()?,
        editing_empty_block()?,
        font_chain_missing()?,
        font_generics()?,
        font_three_faces()?,
        font_corrupt_declaration()?,
        font_collection_index()?,
        plugin_fuel()?,
        plugin_extensions()?,
        clipboard_unicode_seams()?,
        range_policy_endpoints()?,
        image_missing()?,
        image_corrupt()?,
        image_pixels_extreme()?,
        image_oversized()?,
        image_float()?,
        image_note()?,
        image_rotated()?,
        image_vertical()?,
        image_empty_alt()?,
        image_unreadable()?,
        space_before_zwj()?,
        incremental_long_paragraph_and_table()?,
        font_legacy_missing()?,
        collab_hostile_peer()?,
        format_overlap_storm()?,
        table_header_repeats()?,
        table_header_taller_than_frame()?,
        table_spans_whole_table()?,
        table_thousand_columns()?,
        table_rowspan_vertical_break()?,
        table_concurrent_overlapping_spans()?,
        pdf_structure_storm()?,
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
    // The built-in template's geometry, with a main column of zero width and a
    // margin column of negative width.
    let builtin = PageTemplate::builtin();
    let mut template = PageTemplate::new("narrow", builtin.width, builtin.height);
    for mut frame in builtin.frames {
        match frame.role {
            FrameRole::Margin | FrameRole::Notes => {
                // Where a margin column after a zero-wide main column starts.
                frame.x = Dim::pt(36 + 18);
                frame.width = Dim::Pt(Length::from_pt(-10));
            }
            FrameRole::Flow(_) => frame.width = Dim::Pt(Length::ZERO),
        }
        template.frames.push(frame);
    }
    doc.set_page_template(&template)?;
    doc.commit();
    Ok(Fixture::new(
        "zero_width_measure",
        doc,
        &["compose.overflow", "layout.degenerate-frame"],
    ))
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
    // A narrow main column, with the margin column right after it, as the
    // spike's page settings placed it.
    let builtin = PageTemplate::builtin();
    let mut template = PageTemplate::new("narrow-verse", builtin.width, builtin.height);
    for mut frame in builtin.frames {
        match frame.role {
            FrameRole::Margin | FrameRole::Notes => frame.x = Dim::pt(36 + 110 + 18),
            FrameRole::Flow(_) => frame.width = Dim::pt(110),
        }
        template.frames.push(frame);
    }
    fixture.doc.set_page_template(&template)?;
    fixture.doc.commit();
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
/// paragraph is laid out regardless. Layout publishes the `style.*` notes
/// from used style resolution on the affected node.
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
    Ok(Fixture::new(
        "style_expressions",
        doc,
        &[
            "style.expr-limit",
            "style.type-error",
            "style.unknown-function",
            "style.function-failed",
            "style.unparsed",
        ],
    ))
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
    Ok(Fixture::new(
        "style_cycles",
        doc,
        &["style.cycle", "style.parent-cycle", "style.parent-missing"],
    ))
}

/// Percentages and references whose basis is missing or depends on its own
/// content, such as the height of an auto-height frame (18). `tests/hostile.rs`
/// resolves it both in the default context, where every basis is unresolved,
/// and in a context where the main frame is auto-height.
pub fn style_bases() -> Result<Fixture, DocError> {
    let doc = document()?;
    // Half of a definite 24pt frame is a usable 12pt size. The shallow
    // frame still forces pagination, rather than making a 114pt font from
    // the built-in frame and overflowing every line.
    doc.set_page_template(
        &PageTemplate::new("style-bases", Dim::pt(360), Dim::pt(100))
            .with_frame(flow_frame(
                "main",
                Dim::pt(10),
                Dim::pt(10),
                Dim::pt(200),
                Dim::pt(24),
            ))
            .with_frame(margin_frame(
                "margin",
                Dim::pt(230),
                Dim::pt(10),
                Dim::pt(110),
                Dim::pt(24),
            )),
    )?;
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
    Ok(Fixture::new(
        "style_bases",
        doc,
        &["style.basis-unresolved"],
    ))
}

/// Every frame of the main flow is shorter than one line, so no line fits any
/// of them. The text must still be placed (overflowing, and reported), not
/// looked for in more and more pages. A note lands in a margin frame that is
/// shorter than a line too.
pub fn frame_shorter_than_a_line() -> Result<Fixture, DocError> {
    let doc = document()?;
    let template = PageTemplate::new("sliver", Dim::pt(300), Dim::pt(200))
        .with_frame(flow_frame(
            "sliver",
            Dim::pt(20),
            Dim::pt(20),
            Dim::pt(150),
            Dim::pt(4),
        ))
        .with_frame(margin_frame(
            "margin",
            Dim::pt(190),
            Dim::pt(20),
            Dim::pt(90),
            Dim::pt(4),
        ));
    doc.set_page_template(&template)?;
    paragraph_with_note(
        &doc,
        "Two lines at least, in a frame that holds none.",
        "least",
    )?;
    doc.append_block(BlockKind::Paragraph, "body", "And a second paragraph.")?;
    doc.commit();
    Ok(Fixture::new(
        "frame_shorter_than_a_line",
        doc,
        &["layout.frame-overflow"],
    ))
}

/// A template whose only text frame belongs to a flow no paragraph is in:
/// there is nowhere for the main flow to go, so the built-in template stands in.
pub fn no_main_flow() -> Result<Fixture, DocError> {
    let doc = document()?;
    let template = PageTemplate::new("sidebar-only", Dim::pt(300), Dim::pt(200))
        .with_frame(FrameTemplate::new(
            "sidebar",
            FrameRole::Flow("sidebar".into()),
            (Dim::pt(10), Dim::pt(10)),
            (Dim::pt(100), Dim::pt(180)),
        ))
        .with_frame(margin_frame(
            "margin",
            Dim::pt(150),
            Dim::pt(10),
            Dim::pt(100),
            Dim::pt(180),
        ));
    doc.set_page_template(&template)?;
    paragraph_with_note(&doc, "Text with no frame of its own to flow in.", "own")?;
    doc.commit();
    Ok(Fixture::new(
        "no_main_flow",
        doc,
        &["layout.template-unusable"],
    ))
}

/// A page of negative width: the template can't be used, so the built-in one
/// stands in.
pub fn negative_page_size() -> Result<Fixture, DocError> {
    let doc = document()?;
    let mut template = two_columns();
    template.width = Dim::pt(-100);
    doc.set_page_template(&template)?;
    paragraph_with_note(&doc, "A page that is less than nothing wide.", "nothing")?;
    doc.commit();
    Ok(Fixture::new(
        "negative_page_size",
        doc,
        &["layout.template-unusable"],
    ))
}

/// Frames of zero and negative size. Ones with no depth are passed over, and
/// the text flows into the first frame that has some. The margin frame is
/// zero wide, so its notes overflow it.
pub fn zero_sized_frames() -> Result<Fixture, DocError> {
    let doc = document()?;
    let template = PageTemplate::new("degenerate", Dim::pt(300), Dim::pt(200))
        .with_frame(flow_frame(
            "no-depth",
            Dim::pt(10),
            Dim::pt(10),
            Dim::pt(100),
            Dim::pt(0),
        ))
        .with_frame(flow_frame(
            "negative-depth",
            Dim::pt(10),
            Dim::pt(10),
            Dim::pt(100),
            Dim::pt(-50),
        ))
        .with_frame(flow_frame(
            "negative-width",
            Dim::pt(10),
            Dim::pt(10),
            Dim::pt(-100),
            Dim::pt(0),
        ))
        .with_frame(flow_frame(
            "column",
            Dim::pt(10),
            Dim::pt(10),
            Dim::pt(120),
            Dim::pt(60),
        ))
        .with_frame(margin_frame(
            "margin",
            Dim::pt(150),
            Dim::pt(10),
            Dim::pt(0),
            Dim::pt(180),
        ));
    doc.set_page_template(&template)?;
    let text = long_text(1);
    paragraph_with_note(&doc, &text, "corridor")?;
    doc.commit();
    Ok(Fixture::new(
        "zero_sized_frames",
        doc,
        &["layout.degenerate-frame", "compose.overflow"],
    ))
}

/// A document that needs more pages than the engine allows. The text beyond
/// the limit is left out and reported; notes on text that was placed still
/// follow it, and a note on text that was left out is reported too.
pub fn page_limit() -> Result<Fixture, DocError> {
    let doc = document()?;
    doc.set_page_template(&two_columns())?;
    let text = long_text(6);
    let first = doc.append_block(BlockKind::Paragraph, "body", &text)?;
    let second = doc.append_block(BlockKind::Paragraph, "body", "Beyond the limit: a target.")?;
    let schemas = SchemaRegistry::builtin();
    let early = doc.append_block(BlockKind::Annotation, "note", "On the first page.")?;
    let r = doc.add_range(first, 0..3, RangePolicy::FIXED)?;
    doc.add_relation(&schemas, &follow(early, r))?;
    let lost = doc.append_block(BlockKind::Annotation, "note", "On text that is gone.")?;
    let r = doc.add_range(second, 0..6, RangePolicy::FIXED)?;
    doc.add_relation(&schemas, &follow(lost, r))?;
    doc.commit();
    let mut fixture = Fixture::new(
        "page_limit",
        doc,
        &[
            "layout.page-limit",
            "layout.text-unplaced",
            "relation.no-match",
            "layout.unplaced",
        ],
    );
    fixture.engine.flow.max_pages = 2;
    Ok(fixture)
}

/// The template in use is stored by a newer engine, in a form this one can't
/// read. It stays in the document untouched, and the built-in template stands in.
pub fn unreadable_template() -> Result<Fixture, DocError> {
    let doc = document()?;
    doc.store_raw_page_template(
        "future",
        r#"{"version":9,"template":{"name":"future","columns":"as many as fit"}}"#,
    )?;
    doc.store_raw_page_template("also-future", "{ not even json")?;
    doc.use_page_template("future")?;
    paragraph_with_note(&doc, "Laid out on the built-in template.", "built-in")?;
    doc.commit();
    Ok(Fixture::new(
        "unreadable_template",
        doc,
        &["layout.template-unreadable"],
    ))
}

/// Forty frames, each holding exactly one line, so a paragraph is cut into a
/// fragment per frame across several pages.
pub fn column_storm() -> Result<Fixture, DocError> {
    let doc = document()?;
    let mut template = PageTemplate::new("storm", Dim::pt(420), Dim::pt(300));
    for i in 0..40 {
        template.frames.push(flow_frame(
            &format!("cell-{i}"),
            Dim::pt(10 + (i % 4) * 100),
            Dim::pt(10 + (i / 4) * 16),
            Dim::pt(90),
            Dim::pt(14),
        ));
    }
    doc.set_page_template(&template)?;
    doc.append_block(BlockKind::Paragraph, "body", &long_text(6))?;
    doc.append_block(BlockKind::Paragraph, "body", "")?;
    doc.append_block(BlockKind::Paragraph, "body", &long_text(1))?;
    doc.commit();
    Ok(Fixture::new("column_storm", doc, &[]))
}

/// Two peers redefine the same template at once, and one of them also edits
/// the text. Both replicas must converge on one template and one layout.
pub fn concurrent_templates() -> Result<Fixture, DocError> {
    let doc = document()?;
    doc.set_page_template(&two_columns())?;
    doc.append_block(BlockKind::Paragraph, "body", &long_text(2))?;
    doc.commit();
    let other = doc.fork(OTHER_PEER)?;

    let mut wider = two_columns();
    wider.frames[0].width = Dim::pt(100);
    doc.define_page_template(&wider)?;
    let mut taller = two_columns();
    taller.frames[0].height = Dim::pt(40);
    other.define_page_template(&taller)?;
    other.define_page_template(&PageTemplate::new("extra", Dim::pt(200), Dim::pt(200)))?;
    let first = doc.blocks()[0];
    other.block(first)?.text.insert(0, "Edited. ")?;

    doc.merge(&other)?;
    other.merge(&doc)?;
    let mut fixture = Fixture::new("concurrent_templates", doc, &[]);
    fixture.replica = Some(other);
    Ok(fixture)
}

/// A relation whose target line is on a page that has no margin frame to
/// place its owner in.
pub fn no_margin_frame() -> Result<Fixture, DocError> {
    let doc = document()?;
    let template =
        PageTemplate::new("text-only", Dim::pt(300), Dim::pt(200)).with_frame(flow_frame(
            "column",
            Dim::pt(20),
            Dim::pt(20),
            Dim::pt(200),
            Dim::pt(160),
        ));
    doc.set_page_template(&template)?;
    paragraph_with_note(&doc, "A paragraph whose note has no margin.", "note")?;
    doc.commit();
    Ok(Fixture::new(
        "no_margin_frame",
        doc,
        &["relation.no-frame", "layout.unplaced"],
    ))
}

/// RLO continues across soft and forced line breaks, with hanging spaces.
pub fn bidi_line_override() -> Result<Fixture, DocError> {
    let doc = document()?;
    doc.set_page_template(&two_columns())?;
    doc.append_block(BlockKind::Paragraph, "body",
        "Latin \u{202e}office 123 office 456 office 789    \nmore reversed office 321    \u{202c} normal end.   ")?;
    doc.append_block(BlockKind::Paragraph, "body", "אבג 123 אבג    \nאבג 456    ")?;
    doc.commit();
    Ok(Fixture::new("bidi_line_override", doc, &[]))
}

/// A follow target covers lines on both sides of a frame break. A required
/// single-line role must report all candidates as ambiguous and place none.
pub fn follow_lines_across_frames() -> Result<Fixture, DocError> {
    let doc = document()?;
    doc.set_page_template(&two_columns())?;
    let text = long_text(1);
    let p = doc.append_block(BlockKind::Paragraph, "body", &text)?;
    let note = doc.append_block(BlockKind::Annotation, "note", "Which column?")?;
    let range = doc.add_range(p, 0..text.len(), RangePolicy::FIXED)?;
    let relation = Relation::new(reprise_doc::relation::builtin::FOLLOW)
        .owned_by(note)
        .target("line", Target::Layout(LayoutQuery::LinesIn { range }));
    doc.add_relation(&SchemaRegistry::builtin(), &relation)?;
    doc.commit();
    Ok(Fixture::new(
        "follow_lines_across_frames",
        doc,
        &["relation.ambiguous", "layout.unplaced"],
    ))
}

/// The style keeps its starting frame's 10pt size across a narrower column.
/// Named frames and medium/page/block bases resolve, but auto block height
/// and the unavailable line extent diagnose and fall back.
pub fn style_fragment_basis() -> Result<Fixture, DocError> {
    let doc = document()?;
    doc.set_page_template(
        &PageTemplate::new("unequal-style-columns", Dim::pt(420), Dim::pt(180))
            .with_frame(flow_frame(
                "wide",
                Dim::pt(10),
                Dim::pt(10),
                Dim::pt(200),
                Dim::pt(30),
            ))
            .with_frame(flow_frame(
                "narrow",
                Dim::pt(220),
                Dim::pt(10),
                Dim::pt(80),
                Dim::pt(60),
            ))
            .with_frame(margin_frame(
                "margin",
                Dim::pt(310),
                Dim::pt(10),
                Dim::pt(100),
                Dim::pt(60),
            )),
    )?;
    let mut style = Style::default();
    style.set(Property::Size, expr("5% * frame-width")?);
    doc.define_style("frame-sized", &style)?;
    doc.append_block(BlockKind::Paragraph, "frame-sized", &long_text(1))?;
    paragraph_sized(&doc, "named-narrow", expr(r#"frame-width("narrow") / 8"#)?)?;
    paragraph_sized(
        &doc,
        "medium-page-block",
        expr("min(medium-width / 42, page-width / 42, block-width / 8)")?,
    )?;
    paragraph_sized(&doc, "auto-block-height", expr("10% * block-height")?)?;
    paragraph_sized(&doc, "no-line-yet", expr("line-width / 20")?)?;
    doc.commit();
    Ok(Fixture::new(
        "style_fragment_basis",
        doc,
        &["style.basis-indefinite", "style.basis-unresolved"],
    ))
}

/// Justification attacks RTL hanging spaces, a wrapping RLO, mixed levels,
/// non-ASCII word separators, single words and all-space forced lines.
pub fn justified_bidi() -> Result<Fixture, DocError> {
    let doc = document()?;
    doc.set_page_template(&two_columns())?;
    let rtl = format!("{}\nאבג 456    ", "אבג דהו זחט יכל 123 ".repeat(8));
    doc.append_block(BlockKind::Paragraph, "body", &rtl)?;
    let reversed = format!(
        "Latin \u{202e}{}    \u{202c} normal end.   ",
        "office 123 office 456 more words ".repeat(6)
    );
    doc.append_block(BlockKind::Paragraph, "body", &reversed)?;
    let mixed = format!(
        "{}\u{2028}A short last line.   ",
        "Latin אבג 123 words דהו 456 office ".repeat(6)
    );
    doc.append_block(BlockKind::Paragraph, "body", &mixed)?;
    doc.append_block(BlockKind::Paragraph, "body", "word\nword\n    \nword")?;
    doc.append_block(
        BlockKind::Paragraph,
        "body",
        &"one\u{a0}two three\u{1361}four five six seven ".repeat(6),
    )?;
    doc.commit();
    let mut fixture = Fixture::new("justified_bidi", doc, &[]);
    fixture.engine.composer = Box::new(Optimal::justified());
    Ok(fixture)
}

/// Save must retain tombstoned characters and IDs, Unicode byte anchors,
/// unreadable authored forms and a historical reference across reopening.
pub fn persistence_tombstones() -> Result<Fixture, DocError> {
    let doc = document()?;
    let p = doc.append_block(BlockKind::Paragraph, "body", "a\u{301} corridor 🏠")?;
    let range = doc.add_range(p, 4..12, RangePolicy::FIXED)?;
    doc.commit();
    let version = doc.revision();
    doc.add_relation(
        &SchemaRegistry::builtin(),
        &Relation::new(REFERENCE).owned_by(p).target(
            "to",
            Target::Snapshot(SnapshotRef {
                version,
                of: SnapshotOf::Range(range),
            }),
        ),
    )?;
    let deleted = doc.append_block(BlockKind::Paragraph, "body", "discarded")?;
    doc.delete_block(deleted)?;
    doc.block(p)?.text.delete(4..5)?;
    doc.store_raw_page_template("future-unused", "{future:opaque}")?;
    doc.commit();
    Ok(Fixture::new("persistence_tombstones", doc, &[]))
}

pub fn transformed_rtl() -> Result<Fixture, DocError> {
    let doc = document()?;
    let mut f = flow_frame(
        "reflected",
        Dim::pt(220),
        Dim::pt(55),
        Dim::pt(210),
        Dim::pt(100),
    );
    f.transform.rotation = reprise_doc::Rotation::Quarter(1);
    f.transform.mirror_x = true;
    f.transform.origin_x = Dim::pt(105);
    f.transform.origin_y = Dim::pt(50);
    doc.set_page_template(
        &PageTemplate::new("reflected", Dim::pt(420), Dim::pt(360)).with_frame(f),
    )?;
    doc.append_block(
        BlockKind::Paragraph,
        "body",
        "אבג The hall turns backwards. דהו A mirror keeps the words in order.",
    )?;
    doc.commit();
    Ok(Fixture::new("transformed_rtl", doc, &[]))
}

pub fn vertical_rl() -> Result<Fixture, DocError> {
    let doc = document()?;
    let mut f = flow_frame(
        "vertical",
        Dim::pt(30),
        Dim::pt(25),
        Dim::pt(100),
        Dim::pt(240),
    );
    f.writing_mode = reprise_doc::WritingMode::VerticalRl;
    doc.set_page_template(
        &PageTemplate::new("vertical", Dim::pt(180), Dim::pt(300)).with_frame(f),
    )?;
    doc.append_block(BlockKind::Paragraph, "body", "The columns descend, one after another, from the right edge of the page. The house is taller inside than outside. The next column is a step to the left.")?;
    doc.commit();
    Ok(Fixture::new("vertical_rl", doc, &[]))
}

pub fn spiral_text() -> Result<Fixture, DocError> {
    let doc = document()?;
    doc.define_style(
        "spiral",
        &Style {
            size: Some(LengthExpr::Pt(Length::from_pt(5))),
            line_height: Some(LengthExpr::Pt(Length::from_pt(6))),
            ..Default::default()
        },
    )?;
    let mut f = flow_frame(
        "spiral",
        Dim::pt(190),
        Dim::pt(190),
        Dim::pt(20),
        Dim::pt(7),
    );
    f.path = Some(reprise_doc::Spiral {
        radius: Dim::pt(56),
        growth: Dim::pt(32),
        start_millidegrees: 0,
        sweep_millidegrees: 1_080_000,
        segments: 96,
    });
    doc.set_page_template(&PageTemplate::new("spiral", Dim::pt(380), Dim::pt(380)).with_frame(f))?;
    doc.append_block(
        BlockKind::Paragraph,
        "spiral",
        &"the hall is a lie a door is a hall ".repeat(32),
    )?;
    doc.commit();
    Ok(Fixture::new("spiral_text", doc, &[]))
}

pub fn reading_cycle() -> Result<Fixture, DocError> {
    let doc = document()?;
    let a = doc.append_block(BlockKind::Paragraph, "body", "First room.")?;
    let b = doc.append_block(BlockKind::Paragraph, "body", "Second room.")?;
    let c = doc.append_block(BlockKind::Paragraph, "body", "Third room.")?;
    for (before, after) in [(a, b), (b, c), (c, a), (b, a)] {
        doc.add_relation(
            &SchemaRegistry::builtin(),
            &reprise_doc::reading::before(before, after),
        )?;
    }
    doc.commit();
    Ok(Fixture::new(
        "reading_cycle",
        doc,
        &["layout.reading-cycle", "layout.reading-conflict"],
    ))
}

pub fn degenerate_transform() -> Result<Fixture, DocError> {
    let doc = document()?;
    let mut f = flow_frame(
        "singular",
        Dim::pt(30),
        Dim::pt(30),
        Dim::pt(180),
        Dim::pt(150),
    );
    f.transform.rotation = reprise_doc::Rotation::Matrix {
        xx: reprise_geom::Fixed::ZERO,
        yx: reprise_geom::Fixed::ZERO,
        xy: reprise_geom::Fixed::ZERO,
        yy: reprise_geom::Fixed::ONE,
    };
    doc.set_page_template(
        &PageTemplate::new("singular", Dim::pt(250), Dim::pt(210)).with_frame(f),
    )?;
    doc.append_block(
        BlockKind::Paragraph,
        "body",
        "Zero scale cannot erase this room. Identity is the reported fallback.",
    )?;
    doc.commit();
    Ok(Fixture::new(
        "degenerate_transform",
        doc,
        &["layout.transform-unusable"],
    ))
}

pub fn rational_rotation_extreme() -> Result<Fixture, DocError> {
    let doc = document()?;
    let mut f = flow_frame(
        "awkward",
        Dim::pt(220),
        Dim::pt(180),
        Dim::pt(160),
        Dim::pt(90),
    );
    f.transform.rotation = reprise_doc::Rotation::Direction {
        dx: i32::MIN,
        dy: i32::MAX - 123_456_789,
    };
    doc.set_page_template(&PageTemplate::new("awkward", Dim::pt(360), Dim::pt(360)).with_frame(f))?;
    doc.append_block(
        BlockKind::Paragraph,
        "body",
        "An exact irrational direction, authored with integers near their limits.",
    )?;
    doc.commit();
    Ok(Fixture::new("rational_rotation_extreme", doc, &[]))
}

fn region_document() -> Result<Document, DocError> {
    let doc = Document::new(PEER)?;
    define_styles(&doc)?;
    for name in ["body", "note"] {
        doc.define_style(
            name,
            &Style {
                size: Some(LengthExpr::Pt(Length::from_pt(10))),
                line_height: Some(LengthExpr::Pt(Length::from_pt(12))),
                ..Default::default()
            },
        )?;
    }
    doc.define_page_template(
        &PageTemplate::new("regions", Dim::pt(100), Dim::pt(60))
            .with_frame(FrameTemplate::new(
                "body",
                FrameRole::Flow("main".into()),
                (Dim::pt(0), Dim::pt(0)),
                (Dim::pt(100), Dim::pt(60)),
            ))
            .with_frame(FrameTemplate::new(
                "notes",
                FrameRole::Notes,
                (Dim::pt(0), Dim::pt(0)),
                (Dim::pt(100), Dim::pt(60)),
            )),
    )?;
    doc.use_page_template("regions")?;
    Ok(doc)
}

fn attach_region(
    doc: &Document,
    schema: SchemaId,
    owner: NodeId,
    anchor: NodeId,
    at: usize,
) -> Result<reprise_doc::RelationId, DocError> {
    let range = doc.add_range(anchor, at..at + 1, RangePolicy::FIXED)?;
    doc.add_relation(
        &SchemaRegistry::builtin(),
        &Relation::new(schema)
            .owned_by(owner)
            .target("anchor", Target::Range(range)),
    )
}

pub fn float_wider_than_frame() -> Result<Fixture, DocError> {
    let doc = region_document()?;
    let body = doc.append_block(BlockKind::Paragraph, "body", "Anchor text.")?;
    let owner = doc.append_block(BlockKind::Annotation, "note", "float")?;
    let range = doc.add_range(body, 1..2, RangePolicy::FIXED)?;
    doc.add_relation(
        &SchemaRegistry::builtin(),
        &Relation::new(reprise_doc::relation::builtin::FLOAT)
            .owned_by(owner)
            .target("anchor", Target::Range(range))
            .param(
                "width",
                reprise_doc::Param::Length(LengthExpr::Pt(Length::from_pt(101))),
            ),
    )?;
    doc.commit();
    Ok(Fixture::new(
        "float_wider_than_frame",
        doc,
        &["layout.float-unplaceable"],
    ))
}

pub fn float_moves_its_anchor() -> Result<Fixture, DocError> {
    let doc = region_document()?;
    let body = doc.append_block(BlockKind::Paragraph, "body", "Anchor text.")?;
    let owner = doc.append_block(BlockKind::Annotation, "note", "float")?;
    let range = doc.add_range(body, 1..2, RangePolicy::FIXED)?;
    doc.add_relation(
        &SchemaRegistry::builtin(),
        &Relation::new(reprise_doc::relation::builtin::FLOAT)
            .owned_by(owner)
            .target("anchor", Target::Range(range))
            .param(
                "width",
                reprise_doc::Param::Length(LengthExpr::Pt(Length::from_pt(100))),
            )
            .param(
                "margin",
                reprise_doc::Param::Length(LengthExpr::Pt(Length::ZERO)),
            ),
    )?;
    doc.commit();
    Ok(Fixture::new(
        "float_moves_its_anchor",
        doc,
        &["layout.region-cycle"],
    ))
}

pub fn note_taller_than_page() -> Result<Fixture, DocError> {
    let doc = region_document()?;
    let body = doc.append_block(BlockKind::Paragraph, "body", "Anchor and later body text.")?;
    let note = doc.append_block(
        BlockKind::Annotation,
        "note",
        "aa\nbb\ncc\ndd\nee\nff\ngg\nhh\nii\njj\nkk",
    )?;
    attach_region(&doc, reprise_doc::relation::builtin::NOTE, note, body, 1)?;
    doc.commit();
    Ok(Fixture::new(
        "note_taller_than_page",
        doc,
        &["layout.note-continued"],
    ))
}

pub fn notes_nested_three_deep() -> Result<Fixture, DocError> {
    let doc = region_document()?;
    let body = doc.append_block(BlockKind::Paragraph, "body", "Anchor text.")?;
    let mut anchor = body;
    for text in ["First note", "Second note", "Third note"] {
        let note = doc.append_block(BlockKind::Annotation, "note", text)?;
        attach_region(&doc, reprise_doc::relation::builtin::NOTE, note, anchor, 1)?;
        anchor = note;
    }
    let range = doc.add_range(anchor, 1..2, RangePolicy::FIXED)?;
    doc.add_relation(
        &SchemaRegistry::builtin(),
        &Relation::new(REFERENCE)
            .owned_by(body)
            .target("to", Target::Layout(LayoutQuery::LineContaining { range })),
    )?;
    doc.commit();
    Ok(Fixture::new("notes_nested_three_deep", doc, &[]))
}

pub fn note_on_last_line() -> Result<Fixture, DocError> {
    let doc = region_document()?;
    let text = "aa\nbb\ncc\ndd\nee";
    let body = doc.append_block(BlockKind::Paragraph, "body", text)?;
    let note = doc.append_block(BlockKind::Annotation, "note", "Last-line note")?;
    attach_region(
        &doc,
        reprise_doc::relation::builtin::NOTE,
        note,
        body,
        text.len() - 1,
    )?;
    doc.commit();
    Ok(Fixture::new(
        "note_on_last_line",
        doc,
        &["layout.note-continued"],
    ))
}

pub fn notes_take_over_page() -> Result<Fixture, DocError> {
    let doc = region_document()?;
    let body = doc.append_block(
        BlockKind::Paragraph,
        "body",
        "Anchor text.\nLater body text.\nLater again.",
    )?;
    let note = doc.append_block(
        BlockKind::Annotation,
        "note",
        "aa\nbb\ncc\ndd\nee\nff\ngg\nhh\nii\njj\nkk\nll\nmm",
    )?;
    attach_region(&doc, reprise_doc::relation::builtin::NOTE, note, body, 1)?;
    doc.commit();
    Ok(Fixture::new(
        "notes_take_over_page",
        doc,
        &["layout.note-continued"],
    ))
}

pub fn table_zero_columns() -> Result<Fixture, DocError> {
    let doc = region_document()?;
    doc.append_table(reprise_doc::TableColumns { columns: vec![] })?;
    doc.append_block(BlockKind::Paragraph, "body", "The rest still flows.")?;
    doc.commit();
    Ok(Fixture::new(
        "table_zero_columns",
        doc,
        &["layout.table-invalid"],
    ))
}

fn region_table(widths: Vec<reprise_doc::ColumnWidth>, text: &str) -> Result<Document, DocError> {
    let doc = region_document()?;
    let table = doc.append_table(reprise_doc::TableColumns {
        columns: widths
            .into_iter()
            .map(|width| reprise_doc::Column { width })
            .collect(),
    })?;
    let row = doc.append_table_row(table, false)?;
    for column in 0..2 {
        let cell = doc.append_table_cell(row, column)?;
        doc.append_cell_block(cell, BlockKind::Paragraph, "body", text)?;
    }
    doc.commit();
    Ok(doc)
}

pub fn table_conflicting_widths() -> Result<Fixture, DocError> {
    let doc = region_table(
        vec![reprise_doc::ColumnWidth::Fixed(Length::from_pt(90)); 2],
        "cell",
    )?;
    Ok(Fixture::new(
        "table_conflicting_widths",
        doc,
        &["layout.solver-infeasible"],
    ))
}

pub fn table_row_taller_than_page() -> Result<Fixture, DocError> {
    let doc = region_table(
        vec![reprise_doc::ColumnWidth::Proportional(1); 2],
        "aa\nbb\ncc\ndd\nee\nff\ngg\nhh\nii\njj\nkk",
    )?;
    Ok(Fixture::new("table_row_taller_than_page", doc, &[]))
}

pub fn infeasible_solver_domain() -> Result<Fixture, DocError> {
    let doc = region_table(
        vec![
            reprise_doc::ColumnWidth::Fixed(Length::MIN),
            reprise_doc::ColumnWidth::Proportional(u32::MAX),
        ],
        "cell",
    )?;
    Ok(Fixture::new(
        "infeasible_solver_domain",
        doc,
        &["layout.solver-infeasible"],
    ))
}

/// A following note anchored beside a UTF-8 edit at a pagination seam.
pub fn incremental_page_seam() -> Result<Fixture, DocError> {
    let doc = document()?;
    doc.set_page_template(&two_columns())?;
    let first = doc.append_block(BlockKind::Paragraph, "body", &long_text(2))?;
    let seam = doc.append_block(
        BlockKind::Paragraph,
        "body",
        "seam e\u{301} \u{5d0} needle and more text ",
    )?;
    let range = doc.add_range(
        seam,
        find(&doc.block(seam)?.text.to_string(), "needle"),
        RangePolicy::FIXED,
    )?;
    let owner = doc.append_block(
        BlockKind::Annotation,
        "note",
        "a note across the edit boundary",
    )?;
    doc.add_relation(&SchemaRegistry::builtin(), &follow(owner, range))?;
    doc.add_relation(
        &SchemaRegistry::builtin(),
        &reprise_doc::reading::before(first, seam),
    )?;
    doc.commit();
    Ok(Fixture::new("incremental_page_seam", doc, &[]))
}

/// Delete while a collaborator types, then undo: the same node, range and note
/// must return with the merged text.
pub fn editing_concurrent_delete_undo() -> Result<Fixture, DocError> {
    let doc = document()?;
    let node = doc.append_block(BlockKind::Paragraph, "body", "anchor")?;
    let range = doc.add_range(node, 0..6, RangePolicy::FIXED)?;
    let note = doc.append_block(BlockKind::Annotation, "note", "Restored identity.")?;
    doc.add_relation(&SchemaRegistry::builtin(), &follow(note, range))?;
    let replica = doc.fork(OTHER_PEER)?;
    let mut undo = doc.undo_stack();
    doc.delete_block(node)?;
    doc.commit_step();
    replica
        .block(node)?
        .text
        .insert(6, " office e\u{301} \u{5D0}\u{5D1}")?;
    doc.merge(&replica)?;
    replica.merge(&doc)?;
    if doc.is_live(node) || !undo.undo()? {
        return Err(DocError::Store("delete/undo lifecycle failed".into()));
    }
    doc.merge(&replica)?;
    replica.merge(&doc)?;
    let mut fixture = Fixture::new("editing_concurrent_delete_undo", doc, &[]);
    fixture.replica = Some(replica);
    Ok(fixture)
}

/// Baseline for a good first command followed by an invalid UTF-8 offset.
pub fn editing_half_invalid() -> Result<Fixture, DocError> {
    let doc = document()?;
    doc.append_block(BlockKind::Paragraph, "body", "h\u{e9}llo")?;
    doc.append_block(BlockKind::Paragraph, "body", "Keep this block.")?;
    doc.commit();
    Ok(Fixture::new("editing_half_invalid", doc, &[]))
}

/// A document with no blocks has a page but no hittable caret.
pub fn editing_empty_document() -> Result<Fixture, DocError> {
    let doc = document()?;
    doc.commit();
    Ok(Fixture::new("editing_empty_document", doc, &[]))
}

/// Empty, zero-width and bidi clusters exercise coincident carets, ligatures,
/// combining marks and extreme page-space hit points.
pub fn editing_empty_block() -> Result<Fixture, DocError> {
    let doc = document()?;
    doc.append_block(BlockKind::Paragraph, "body", "")?;
    doc.append_block(
        BlockKind::Paragraph,
        "body",
        "office e\u{301} \u{200b} \u{5D0}\u{5D1}\u{5D2} xyz",
    )?;
    let zero = doc.append_block(BlockKind::Paragraph, "body", "zero")?;
    doc.set_overrides(
        zero,
        &Style {
            size: Some(LengthExpr::Pt(Length::ZERO)),
            line_height: Some(LengthExpr::Pt(Length::ZERO)),
            ..Default::default()
        },
    )?;
    doc.commit();
    Ok(Fixture::new("editing_empty_block", doc, &[]))
}

fn font_document(chains: &[&[&str]], text: &str) -> Result<Document, DocError> {
    let doc = document()?;
    for chain in chains {
        let node = doc.append_block(BlockKind::Paragraph, "body", "")?;
        doc.block(node)?.text.insert(0, text)?;
        doc.set_overrides(
            node,
            &Style {
                families: Some(chain.iter().map(|f| f.to_string()).collect()),
                ..Style::default()
            },
        )?;
    }
    doc.commit();
    Ok(doc)
}
/// Missing named family, implicit generic terminal, and no font covers U+10FFFF.
pub fn font_chain_missing() -> Result<Fixture, DocError> {
    Ok(Fixture::new(
        "font_chain_missing",
        font_document(&[&["Absent"], &[]], "Fallback \u{10ffff}")?,
        &["font.fallback", "font.missing"],
    ))
}
/// Every generic class resolves without frontend registration, including empty text.
pub fn font_generics() -> Result<Fixture, DocError> {
    Ok(Fixture::new(
        "font_generics",
        font_document(
            &[&["serif"], &["sans-serif"], &["monospace"], &["script"]],
            "office café",
        )?,
        &[],
    ))
}
/// Per-character fallback covers the string across three pinned faces.
pub fn font_three_faces() -> Result<Fixture, DocError> {
    let mut fixture = Fixture::new(
        "font_three_faces",
        font_document(
            &[&["One", "Two", "sans-serif"]],
            &crate::fonts::three_face_text(),
        )?,
        &["font.fallback"],
    );
    let script = fixture
        .engine
        .fonts
        .generic(reprise_font::GenericFamily::Script)
        .clone();
    fixture
        .engine
        .fonts
        .register(script.data(), crate::fonts::declaration("One", 0))
        .expect("bundled face");
    fixture
        .engine
        .fonts
        .register(crate::SERIF, crate::fonts::declaration("Two", 0))
        .expect("bundled face");
    Ok(fixture)
}
/// The frontend receives an Error for a corrupt declaration; layout can continue.
pub fn font_corrupt_declaration() -> Result<Fixture, DocError> {
    let mut fixture = Fixture::new(
        "font_corrupt_declaration",
        font_document(&[&["Corrupt", "serif"]], "still readable")?,
        &["font.fallback"],
    );
    let error = fixture
        .engine
        .fonts
        .register(
            b"not a font".as_slice(),
            crate::fonts::declaration("Corrupt", 0),
        )
        .expect_err("corrupt font refused");
    assert_eq!(error.note().code.as_str(), "font.unreadable");
    Ok(fixture)
}
/// An out-of-range collection index is refused; missing family falls through.
pub fn font_collection_index() -> Result<Fixture, DocError> {
    let mut fixture = Fixture::new(
        "font_collection_index",
        font_document(&[&["BadIndex", "monospace"]], "index fallback")?,
        &["font.fallback"],
    );
    let error = fixture
        .engine
        .fonts
        .register(
            crate::fonts::collection(),
            crate::fonts::declaration("BadIndex", u32::MAX),
        )
        .expect_err("collection index refused");
    assert_eq!(error.note().code.as_str(), "font.unreadable");
    Ok(fixture)
}

/// One module loops at every extension point: text must survive all three fallbacks.
pub fn plugin_fuel() -> Result<Fixture, DocError> {
    let doc = document()?;
    paragraph_sized(&doc, "plugin-loop", expr("plugin-size(9pt)")?)?;
    let node = doc.append_block(
        BlockKind::Paragraph,
        "body",
        "Fuel is a deterministic bound. Text survives a failed extension.",
    )?;
    let mut fixture = Fixture::new(
        "plugin_fuel",
        doc,
        &["style.function-failed", "plugin.fuel"],
    );
    let plugin = crate::plugins::load(crate::plugins::LOOP)?;
    fixture
        .engine
        .install_plugin_functions(&plugin)
        .map_err(|e| DocError::Store(e.to_string()))?;
    fixture
        .engine
        .install_plugin_geometry(plugin.clone(), 2, Box::new(reprise_compose::Greedy))
        .map_err(|n| DocError::Store(n.message))?;
    fixture
        .engine
        .install_plugin_relation(
            crate::plugins::schema(),
            reprise_layout::plugins::RelationBinding {
                plugin: Some(plugin),
                operation: 3,
            },
        )
        .map_err(|e| DocError::Store(e.to_string()))?;
    fixture.doc.add_relation(
        &fixture.engine.schemas,
        &Relation::new(crate::plugins::REPORT)
            .target("to", Target::Layout(LayoutQuery::FirstLine { node })),
    )?;
    fixture.doc.commit();
    Ok(fixture)
}

/// The same source-defined plugin doubles style size, tapers room and reports a resolved line.
pub fn plugin_extensions() -> Result<Fixture, DocError> {
    let doc = document()?;
    let mut style = Style::default();
    style.set(Property::Size, expr("plugin-size(9pt)")?);
    doc.define_style("plugin", &style)?;
    let node = doc.append_block(BlockKind::Paragraph, "plugin", "A sandbox shapes this paragraph with three steps of inset. Every byte remains in reading order, and the relation reports its first line.")?;
    let mut fixture = Fixture::new("plugin_extensions", doc, &[]);
    let plugin = crate::plugins::load(crate::plugins::DEMO)?;
    fixture
        .engine
        .install_plugin_functions(&plugin)
        .map_err(|e| DocError::Store(e.to_string()))?;
    fixture
        .engine
        .install_plugin_geometry(plugin.clone(), 2, Box::new(reprise_compose::Greedy))
        .map_err(|n| DocError::Store(n.message))?;
    fixture
        .engine
        .install_plugin_relation(
            crate::plugins::schema(),
            reprise_layout::plugins::RelationBinding {
                plugin: Some(plugin),
                operation: 3,
            },
        )
        .map_err(|e| DocError::Store(e.to_string()))?;
    fixture.doc.add_relation(
        &fixture.engine.schemas,
        &Relation::new(crate::plugins::REPORT)
            .target("to", Target::Layout(LayoutQuery::FirstLine { node })),
    )?;
    fixture.doc.commit();
    Ok(fixture)
}

/// Clipboard seams cross a Latin ligature, a combining sequence and Hebrew;
/// expanding/fixed/point ranges and a note must survive ID remapping together.
pub fn clipboard_unicode_seams() -> Result<Fixture, DocError> {
    let doc = document()?;
    let schemas = SchemaRegistry::builtin();
    let paragraph = doc.append_block(
        BlockKind::Paragraph,
        "body",
        "office e\u{301} אבג\nA second authored line.",
    )?;
    let ligature = doc.add_range(paragraph, 1..5, RangePolicy::EXPANDING)?;
    let _mark = doc.add_range(paragraph, 8..10, RangePolicy::FIXED)?;
    let _point = doc.add_range(paragraph, 10..10, RangePolicy::POINT)?;
    let note = doc.append_block(
        BlockKind::Annotation,
        "note",
        "A copied note beside the ligature.",
    )?;
    doc.add_relation(&schemas, &follow(note, ligature))?;
    let other = doc.append_block(
        BlockKind::Paragraph,
        "body",
        "Outside a partial copy; inside copy-all.",
    )?;
    doc.add_relation(&schemas, &reference(paragraph, Target::Node(other)))?;
    doc.commit();
    Ok(Fixture::new("clipboard_unicode_seams", doc, &[]))
}

/// Every authored endpoint policy on Unicode text and the ambiguous empty-text point.
pub fn range_policy_endpoints() -> Result<Fixture, DocError> {
    use reprise_doc::text::{Affinity, Empty};
    let doc = document()?;
    let node = doc.append_block(
        BlockKind::Paragraph,
        "body",
        "office e\u{301} \u{5d0}\u{5d1}\u{5d2}",
    )?;
    let blank = doc.append_block(BlockKind::Paragraph, "body", "")?;
    let len = doc.block(node)?.text.len();
    for start in [Affinity::Before, Affinity::After] {
        for end in [Affinity::Before, Affinity::After] {
            for empty in [Empty::Keep, Empty::Missing] {
                doc.add_range(node, 0..len, RangePolicy { start, end, empty })?;
            }
            doc.add_range(
                blank,
                0..0,
                RangePolicy {
                    start,
                    end,
                    empty: Empty::Keep,
                },
            )?;
        }
    }
    doc.commit();
    let replica = doc.fork(OTHER_PEER)?;
    doc.merge(&replica)?;
    replica.merge(&doc)?;
    let mut fixture = Fixture::new("range_policy_endpoints", doc, &[]);
    fixture.replica = Some(replica);
    Ok(fixture)
}

fn image_fixture(
    name: &'static str,
    bytes: &[u8],
    alt: &str,
    expect: &'static [&'static str],
) -> Result<Fixture, DocError> {
    let mut fixture = Fixture::new(name, document()?, expect);
    let hash = fixture
        .engine
        .assets
        .insert(bytes)
        .map_err(|e| DocError::Store(e.into()))?;
    fixture
        .doc
        .append_image("body", &reprise_doc::image::ImageData::new(hash), alt)?;
    fixture.doc.commit();
    Ok(fixture)
}

pub fn image_missing() -> Result<Fixture, DocError> {
    let fixture = Fixture::new("image_missing", document()?, &["layout.image-missing"]);
    fixture.doc.append_image(
        "body",
        &reprise_doc::image::ImageData::new("0".repeat(64)),
        "A missing image",
    )?;
    fixture
        .doc
        .append_block(BlockKind::Paragraph, "body", "The following text remains.")?;
    fixture.doc.commit();
    Ok(fixture)
}

pub fn image_corrupt() -> Result<Fixture, DocError> {
    image_fixture(
        "image_corrupt",
        b"\x89PNG\r\n\x1a\ncorrupt",
        "Corrupt header",
        &["layout.image-header"],
    )
}

pub fn image_pixels_extreme() -> Result<Fixture, DocError> {
    let mut fixture = image_fixture(
        "image_pixels_extreme",
        include_bytes!("../../../fixtures/images/red-1x1.png"),
        "One pixel",
        &["layout.image-size"],
    )?;
    let hash = fixture
        .engine
        .assets
        .insert(include_bytes!("../../../fixtures/images/header-65535.png").as_slice())
        .map_err(|e| DocError::Store(e.into()))?;
    fixture.doc.append_image(
        "body",
        &reprise_doc::image::ImageData::new(hash),
        "65535 square pixels; metadata only",
    )?;
    fixture.doc.commit();
    Ok(fixture)
}

pub fn image_oversized() -> Result<Fixture, DocError> {
    let fixture = image_fixture(
        "image_oversized",
        include_bytes!("../../../fixtures/images/red-1x1.png"),
        "Oversized image",
        &["layout.image-size", "layout.frame-overflow"],
    )?;
    let node = fixture
        .doc
        .blocks()
        .into_iter()
        .next()
        .ok_or_else(|| DocError::Store("missing fixture image".into()))?;
    let mut image = fixture.doc.image(node)?;
    image.width = Some(LengthExpr::Pt(Length::from_pt(1000)));
    image.height = Some(LengthExpr::Pt(Length::from_pt(2000)));
    fixture.doc.set_image(node, &image)?;
    fixture.doc.commit();
    Ok(fixture)
}

fn region_image(name: &'static str, schema: SchemaId) -> Result<Fixture, DocError> {
    let mut fixture = Fixture::new(name, region_document()?, &[]);
    let anchor = fixture.doc.append_block(
        BlockKind::Paragraph,
        "body",
        "Anchor text flows around the image. More words flow below the image in the same frame.",
    )?;
    let hash = fixture
        .engine
        .assets
        .insert(include_bytes!("../../../fixtures/images/red-2x1.jpg").as_slice())
        .map_err(|e| DocError::Store(e.into()))?;
    let mut image = reprise_doc::image::ImageData::new(hash);
    image.width = Some(LengthExpr::Pt(Length::from_pt(24)));
    let owner = fixture
        .doc
        .append_image("note", &image, "An image in a region")?;
    attach_region(&fixture.doc, schema, owner, anchor, 0)?;
    fixture.doc.commit();
    Ok(fixture)
}

pub fn image_float() -> Result<Fixture, DocError> {
    region_image("image_float", reprise_doc::relation::builtin::FLOAT)
}
pub fn image_note() -> Result<Fixture, DocError> {
    region_image("image_note", reprise_doc::relation::builtin::NOTE)
}

pub fn image_rotated() -> Result<Fixture, DocError> {
    let fixture = image_fixture(
        "image_rotated",
        include_bytes!("../../../fixtures/images/red-1x1.png"),
        "Rotated and mirrored image",
        &[],
    )?;
    let mut frame = flow_frame(
        "image",
        Dim::pt(100),
        Dim::pt(100),
        Dim::pt(100),
        Dim::pt(100),
    );
    frame.transform.rotation = reprise_doc::Rotation::Quarter(1);
    frame.transform.mirror_x = true;
    fixture.doc.define_page_template(
        &PageTemplate::new("images", Dim::pt(300), Dim::pt(300)).with_frame(frame),
    )?;
    fixture.doc.use_page_template("images")?;
    fixture.doc.commit();
    Ok(fixture)
}

pub fn image_vertical() -> Result<Fixture, DocError> {
    let fixture = image_fixture(
        "image_vertical",
        include_bytes!("../../../fixtures/images/red-2x1.jpg"),
        "Vertical image",
        &[],
    )?;
    let mut frame = flow_frame(
        "image",
        Dim::pt(100),
        Dim::pt(100),
        Dim::pt(100),
        Dim::pt(100),
    );
    frame.writing_mode = reprise_doc::WritingMode::VerticalRl;
    fixture.doc.define_page_template(
        &PageTemplate::new("images", Dim::pt(300), Dim::pt(300)).with_frame(frame),
    )?;
    fixture.doc.use_page_template("images")?;
    fixture.doc.commit();
    Ok(fixture)
}

pub fn image_empty_alt() -> Result<Fixture, DocError> {
    image_fixture(
        "image_empty_alt",
        include_bytes!("../../../fixtures/images/red-1x1.png"),
        "",
        &[],
    )
}

pub fn image_unreadable() -> Result<Fixture, DocError> {
    let fixture = image_fixture(
        "image_unreadable",
        include_bytes!("../../../fixtures/images/red-1x1.png"),
        "Future image",
        &["layout.image-record"],
    )?;
    let node = fixture
        .doc
        .blocks()
        .into_iter()
        .next()
        .ok_or_else(|| DocError::Store("missing fixture image".into()))?;
    fixture
        .doc
        .set_image_record(node, r#"{"version":999,"future":["kept"]}"#)?;
    fixture.doc.commit();
    Ok(fixture)
}

/// Legacy single-family input must render through the terminal engine default.
pub fn font_legacy_missing() -> Result<Fixture, DocError> {
    let doc = document()?;
    doc.define_style(
        "missing-legacy",
        &Style {
            family: Some("Not Registered Legacy Serif".into()),
            ..Style::default()
        },
    )?;
    doc.append_block(
        BlockKind::Paragraph,
        "missing-legacy",
        "office ffi e\u{301}",
    )?;
    doc.commit();
    Ok(Fixture::new("font_legacy_missing", doc, &["font.fallback"]))
}

// ---- Tables: repeated headers and spans (24, 33, 37) ----

fn header_table(doc: &Document, columns: usize) -> Result<NodeId, DocError> {
    doc.append_table(reprise_doc::TableColumns {
        columns: (0..columns)
            .map(|_| reprise_doc::Column {
                width: reprise_doc::ColumnWidth::Proportional(1),
            })
            .collect(),
    })
}

fn table_row_of(
    doc: &Document,
    table: NodeId,
    header: bool,
    texts: &[&str],
) -> Result<Vec<NodeId>, DocError> {
    let row = doc.append_table_row(table, header)?;
    let mut cells = Vec::new();
    for (column, text) in texts.iter().enumerate() {
        let cell = doc.append_table_cell(row, column as u32)?;
        doc.append_cell_block(cell, BlockKind::Paragraph, "body", text)?;
        cells.push(cell);
    }
    Ok(cells)
}

/// A two-row header repeats on every continuation frame of a long table.
pub fn table_header_repeats() -> Result<Fixture, DocError> {
    let doc = region_document()?;
    let table = header_table(&doc, 2)?;
    table_row_of(&doc, table, true, &["Room", "Measure"])?;
    for i in 0..9 {
        table_row_of(&doc, table, false, &[&format!("door {i}"), "inside"])?;
    }
    doc.commit();
    Ok(Fixture::new("table_header_repeats", doc, &[]))
}

/// A header taller than the frame cannot repeat; nothing is lost or looped.
pub fn table_header_taller_than_frame() -> Result<Fixture, DocError> {
    let doc = region_document()?;
    let table = header_table(&doc, 2)?;
    table_row_of(&doc, table, true, &["a\nb\nc\nd\ne\nf\ng\nh", "title"])?;
    for i in 0..4 {
        table_row_of(&doc, table, false, &[&format!("row {i}"), "x"])?;
    }
    doc.commit();
    Ok(Fixture::new(
        "table_header_taller_than_frame",
        doc,
        &["layout.table-header-unrepeated"],
    ))
}

/// A span over the whole table, zero spans, spans past the edge and a cell
/// that collides with an earlier span.
pub fn table_spans_whole_table() -> Result<Fixture, DocError> {
    let doc = region_document()?;
    let table = header_table(&doc, 3)?;
    let title = table_row_of(&doc, table, true, &["The whole table"])?;
    doc.set_table_cell_span(title[0], u32::MAX, u32::MAX)?;
    let row = table_row_of(&doc, table, false, &["zero", "wide", "late"])?;
    doc.set_table_cell_span(row[0], 0, 0)?;
    doc.set_table_cell_span(row[1], 7, 1)?;
    doc.commit();
    Ok(Fixture::new(
        "table_spans_whole_table",
        doc,
        &["layout.table-span", "layout.table-invalid"],
    ))
}

/// 1,000 declared columns: more than a solver domain takes, so the table is
/// omitted and the paragraph after it still flows.
pub fn table_thousand_columns() -> Result<Fixture, DocError> {
    let doc = region_document()?;
    let table = header_table(&doc, 1000)?;
    table_row_of(&doc, table, false, &["x"])?;
    doc.append_block(BlockKind::Paragraph, "body", "The rest still flows.")?;
    doc.commit();
    Ok(Fixture::new(
        "table_thousand_columns",
        doc,
        &["layout.table-invalid"],
    ))
}

/// A row span longer than a rotated, mirrored vertical frame: it cannot be kept
/// together, so it splits across pages with a warning and loses nothing.
pub fn table_rowspan_vertical_break() -> Result<Fixture, DocError> {
    let doc = document()?;
    let mut f = flow_frame(
        "vertical",
        Dim::pt(220),
        Dim::pt(55),
        Dim::pt(100),
        Dim::pt(120),
    );
    f.writing_mode = reprise_doc::WritingMode::VerticalRl;
    f.transform.rotation = reprise_doc::Rotation::Quarter(1);
    f.transform.mirror_x = true;
    f.transform.origin_x = Dim::pt(105);
    f.transform.origin_y = Dim::pt(50);
    doc.set_page_template(
        &PageTemplate::new("rotated-table", Dim::pt(420), Dim::pt(360)).with_frame(f),
    )?;
    let table = header_table(&doc, 2)?;
    table_row_of(&doc, table, true, &["Head", "Cols"])?;
    let first = table_row_of(
        &doc,
        table,
        false,
        &["l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9", "top"],
    )?;
    doc.set_table_cell_span(first[0], 1, 2)?;
    let second = doc.append_table_row(table, false)?;
    let cell = doc.append_table_cell(second, 1)?;
    doc.append_cell_block(cell, BlockKind::Paragraph, "body", "below")?;
    doc.commit();
    Ok(Fixture::new(
        "table_rowspan_vertical_break",
        doc,
        &["layout.table-rowspan-split"],
    ))
}

/// Two peers concurrently author spans that collide. After the merge both
/// replicas resolve the same grid, and so lay out identically.
pub fn table_concurrent_overlapping_spans() -> Result<Fixture, DocError> {
    let doc = region_document()?;
    let table = header_table(&doc, 3)?;
    let top = table_row_of(&doc, table, false, &["a", "b", "c"])?;
    let bottom = table_row_of(&doc, table, false, &["d", "e", "f"])?;
    doc.commit();
    let other = doc.fork(OTHER_PEER)?;
    // Peer 1 spans `a` over two columns and rows; peer 2 spans `b` over two
    // rows and `d` over two columns. All three overlap.
    doc.set_table_cell_span(top[0], 2, 2)?;
    other.set_table_cell_span(top[1], 1, 2)?;
    other.set_table_cell_span(bottom[0], 2, 1)?;
    doc.merge(&other)?;
    other.merge(&doc)?;
    let mut fixture = Fixture::new(
        "table_concurrent_overlapping_spans",
        doc,
        &["layout.table-invalid"],
    );
    fixture.replica = Some(other);
    Ok(fixture)
}
/// Everything the tagged PDF structure has to survive at once: a heading style, note
/// anchors that cut a ligature, overlap, sit at a point or sit in right-to-left text,
/// a note on a note, a float, a table with an empty cell and an image-less alt.
pub fn pdf_structure_storm() -> Result<Fixture, DocError> {
    let doc = region_document()?;
    doc.define_style(
        "h1",
        &Style {
            size: Some(LengthExpr::Pt(Length::from_pt(12))),
            line_height: Some(LengthExpr::Pt(Length::from_pt(12))),
            ..Default::default()
        },
    )?;
    doc.append_block(BlockKind::Paragraph, "h1", "Hall\u{301}way")?;
    let text = "office \u{5d0}\u{5d1}\u{5d2} end";
    let body = doc.append_block(BlockKind::Paragraph, "body", text)?;
    let anchors: [(usize, usize); 5] = [(0, 2), (0, 4), (text.len(), text.len()), (7, 9), (1, 1)];
    let mut last = body;
    for (i, (start, end)) in anchors.into_iter().enumerate() {
        let note = doc.append_block(BlockKind::Annotation, "note", &format!("Note {i}"))?;
        let policy = if start == end {
            RangePolicy::POINT
        } else {
            RangePolicy::FIXED
        };
        let range = doc.add_range(body, start..end, policy)?;
        doc.add_relation(
            &SchemaRegistry::builtin(),
            &Relation::new(reprise_doc::relation::builtin::NOTE)
                .owned_by(note)
                .target("anchor", Target::Range(range)),
        )?;
        last = note;
    }
    // A note on the last note, and an empty note.
    let nested = doc.append_block(BlockKind::Annotation, "note", "On a note")?;
    attach_region(&doc, reprise_doc::relation::builtin::NOTE, nested, last, 1)?;
    let empty = doc.append_block(BlockKind::Annotation, "note", "")?;
    attach_region(&doc, reprise_doc::relation::builtin::NOTE, empty, body, 3)?;
    let float = doc.append_block(BlockKind::Annotation, "note", "float")?;
    let range = doc.add_range(body, 2..3, RangePolicy::FIXED)?;
    doc.add_relation(
        &SchemaRegistry::builtin(),
        &Relation::new(reprise_doc::relation::builtin::FLOAT)
            .owned_by(float)
            .target("anchor", Target::Range(range)),
    )?;
    let table = doc.append_table(reprise_doc::TableColumns {
        columns: vec![
            reprise_doc::Column {
                width: reprise_doc::ColumnWidth::Proportional(1)
            };
            2
        ],
    })?;
    let row = doc.append_table_row(table, true)?;
    for (column, text) in [(0, "head"), (1, "")] {
        let cell = doc.append_table_cell(row, column)?;
        doc.append_cell_block(cell, BlockKind::Paragraph, "body", text)?;
    }
    doc.commit();
    Ok(Fixture::new(
        "pdf_structure_storm",
        doc,
        &["layout.note-continued", "layout.table-header-unrepeated"],
    ))
}

pub fn space_before_zwj() -> Result<Fixture, DocError> {
    let doc = document()?;
    for words in 0..46 {
        let joiner = if words % 2 == 0 {
            "\u{200D}\u{1F467}"
        } else {
            "\u{301}\u{200D}"
        };
        let text = format!("{}  {joiner} tail end.", "word ".repeat(words));
        doc.append_block(BlockKind::Paragraph, "body", &text)?;
    }
    doc.commit();
    Ok(Fixture::new("space_before_zwj", doc, &[]))
}

pub fn incremental_long_paragraph_and_table() -> Result<Fixture, DocError> {
    let doc = document()?;
    let table = header_table(&doc, 2)?;
    table_row_of(&doc, table, true, &["Room", "Measure"])?;
    for i in 0..20 {
        table_row_of(&doc, table, false, &[&format!("hall {i}"), "longer inside"])?;
    }
    let hebrew = "\u{5e9}\u{5dc}\u{5d5}\u{5dd} \u{5e2}\u{5d5}\u{5dc}\u{5dd} ".repeat(1000);
    let latin = "the hallway grows longer ".repeat(800);
    let cluster = format!("a{}", "\u{301}".repeat(9000));
    doc.append_block(
        BlockKind::Paragraph,
        "body",
        &format!("{hebrew}{latin}{cluster} end"),
    )?;
    doc.commit();
    let mut fixture = Fixture::new(
        "incremental_long_paragraph_and_table",
        doc,
        &["layout.page-limit", "layout.text-unplaced"],
    );
    fixture.engine.flow.max_pages = 3;
    Ok(fixture)
}

/// A hostile peer bypasses the editing kernel (docs/collaboration.md): it
/// gives one paragraph an integer kind and physically deletes a note that a
/// relation owns, while an honest peer edits concurrently. Every replica
/// imports both delta packets, in different orders, converges and lays out
/// the same: the malformed paragraph is left out, the rest flows.
pub fn collab_hostile_peer() -> Result<Fixture, DocError> {
    let doc = document()?;
    let honest = doc.append_block(
        BlockKind::Paragraph,
        "body",
        "The honest paragraph keeps its text.",
    )?;
    let malformed = doc.append_block(
        BlockKind::Paragraph,
        "body",
        "This paragraph is given an unreadable kind.",
    )?;
    paragraph_with_note(&doc, "A noted paragraph whose note is torn out.", "noted")?;
    doc.commit();
    let note = doc
        .blocks()
        .into_iter()
        .find(|&n| doc.kind_of(n) == Some(BlockKind::Annotation))
        .ok_or_else(|| DocError::Store("missing fixture note".into()))?;
    let since = doc.version_vector();
    let replica = doc.fork(OTHER_PEER)?;
    let attacker = doc.fork(66)?;
    reprise_doc::hostile::scripted(&attacker, malformed, note)?;
    replica.block(honest)?.text.insert(4, "and concurrent ")?;
    replica.commit();
    let sync = |e: reprise_doc::sync::SyncError| DocError::Store(e.to_string());
    let attack = attacker.export_delta(&since).map_err(sync)?;
    let edit = replica.export_delta(&since).map_err(sync)?;
    doc.import_packet(&attack).map_err(sync)?;
    doc.import_packet(&edit).map_err(sync)?;
    replica.import_packet(&attack).map_err(sync)?;
    Ok(Fixture {
        replica: Some(replica),
        ..Fixture::new("collab_hostile_peer", doc, &["layout.malformed-block"])
    })
}

/// Character formatting under attack (docs/text-formatting.md): a mixed-script
/// paragraph with combining marks gets hundreds of overlapping size, language
/// and feature actions, some starting inside grapheme clusters, a reset, a
/// concurrent peer's competing action, and one envelope a hostile peer
/// overwrites with garbage. A split then carries formatting into a new
/// paragraph. Every replica resolves the same runs; the garbage is reported
/// and kept, never applied.
pub fn format_overlap_storm() -> Result<Fixture, DocError> {
    use reprise_doc::formatting::{TextFeature, TextStyle};
    let doc = document()?;
    let text = "Hallway e\u{301}\u{301} \u{5e9}\u{5dc}\u{5d5}\u{5dd} grows 1234 \
                \u{1f469}\u{200d}\u{1f467} longer";
    let p = doc.append_block(BlockKind::Paragraph, "body", text)?;
    doc.commit();
    let len = text.len();
    let size = |pt| TextStyle {
        size: Some(Length::from_pt(pt)),
        ..TextStyle::default()
    };
    for i in 0..300usize {
        // Char boundaries only; grapheme clusters may still be split.
        let mut start = (i * 7) % len;
        while !text.is_char_boundary(start) {
            start -= 1;
        }
        let mut end = (start + 1 + (i * 13) % 17).min(len);
        while !text.is_char_boundary(end) {
            end += 1;
        }
        let style = match i % 3 {
            0 => size(8 + (i % 20) as i32),
            1 => TextStyle {
                language: Some(if i % 2 == 0 { "he" } else { "en" }.into()),
                ..TextStyle::default()
            },
            _ => TextStyle {
                features: Some(vec![TextFeature {
                    tag: *b"liga",
                    value: (i % 2) as u32,
                }]),
                ..TextStyle::default()
            },
        };
        doc.format_text(p, start..end, &style, RangePolicy::EXPANDING)?;
    }
    doc.format_text(
        p,
        0..9,
        &TextStyle {
            reset: true,
            ..TextStyle::default()
        },
        RangePolicy::FIXED,
    )?;
    doc.commit();
    let since = doc.version_vector();
    let replica = doc.fork(OTHER_PEER)?;
    replica.format_text(p, 2..len - 2, &size(14), RangePolicy::EXPANDING)?;
    replica.commit();
    let attacker = doc.fork(66)?;
    let victim = attacker.format_text(p, 0..len, &size(40), RangePolicy::FIXED)?;
    attacker.commit();
    reprise_doc::hostile::corrupt_format(&attacker, victim, r#"{"version":1,"order":-1}"#)
        .map_err(|e| DocError::Store(e.to_string()))?;
    let sync = |e: reprise_doc::sync::SyncError| DocError::Store(e.to_string());
    let attack = attacker.export_delta(&since).map_err(sync)?;
    let edit = replica.export_delta(&since).map_err(sync)?;
    doc.import_packet(&attack).map_err(sync)?;
    doc.import_packet(&edit).map_err(sync)?;
    replica.import_packet(&attack).map_err(sync)?;
    doc.split_block(p, text.find("grows").unwrap_or(0))?;
    doc.commit();
    replica.merge(&doc)?;
    Ok(Fixture {
        replica: Some(replica),
        ..Fixture::new("format_overlap_storm", doc, &["style.format-unreadable"])
    })
}
