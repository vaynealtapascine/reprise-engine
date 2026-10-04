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
use reprise_doc::relation::{
    CopyCrossing, CopyInside, CopyPolicy, OnTargetDeleted, Ownership, RoleSpec,
};
use reprise_doc::text::RangePolicy;
use reprise_doc::{
    BlockKind, DocError, Document, LengthExpr, Relation, RelationSchema, SchemaId, SchemaRegistry,
    Style, Target, TargetClass,
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
        optimal_paragraph()?,
        verse_turnover()?,
        optimal_extreme_lengths()?,
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
