//! Orchestrator review: two peers write different values to the same property
//! of the same named style at once, one of them a value this engine can't
//! read. After merging either way round, both replicas must hold the same
//! style, lay out identically, and keep the unreadable value rather than
//! drop it (34).

use reprise_doc::expr::Expr;
use reprise_doc::{Authored, BlockKind, Document, Property, Style};
use reprise_fixtures::spike::define_styles;
use reprise_fixtures::{OTHER_PEER, PEER, engine};

#[test]
fn concurrent_style_expressions_converge_and_keep_unreadable_values() {
    let one = Document::new(PEER).unwrap();
    define_styles(&one).unwrap();
    one.append_block(
        BlockKind::Paragraph,
        "note",
        "A note whose size two peers fight over.",
    )
    .unwrap();
    one.commit();
    let two = one.fork(OTHER_PEER).unwrap();

    let mut a = one.style("note").unwrap();
    a.set(
        Property::Size,
        Authored::Expr(Expr::parse("1.5em + 2pt").unwrap()),
    );
    a.set(
        Property::LineHeight,
        Authored::Expr(Expr::parse("2 * 1lh").unwrap()),
    );
    one.define_style("note", &a).unwrap();
    one.commit();

    let mut b: Style = two.style("note").unwrap();
    b.set(
        Property::Size,
        Authored::Unparsed("expr9:from(the future)".into()),
    );
    two.define_style("note", &b).unwrap();
    two.commit();

    let left = one.fork(PEER).unwrap();
    left.merge(&two).unwrap();
    let right = two.fork(OTHER_PEER).unwrap();
    right.merge(&one).unwrap();

    assert_eq!(left.style("note"), right.style("note"), "styles converge");
    let engine = engine();
    let l = engine.layout(&left);
    let r = engine.layout(&right);
    assert_eq!(l.blocks, r.blocks, "layouts converge");
    assert_eq!(l.diagnostics, r.diagnostics, "diagnostics converge");

    // Whichever write won, an unreadable value is kept verbatim, not
    // silently replaced by a default on the next save.
    let merged = left.style("note").unwrap();
    if let Some(Authored::Unparsed(text)) = merged.values.get(&Property::Size) {
        assert_eq!(text, "expr9:from(the future)");
        left.define_style("note", &merged).unwrap();
        left.commit();
        assert_eq!(
            left.style("note").unwrap().values.get(&Property::Size),
            Some(&Authored::Unparsed("expr9:from(the future)".into()))
        );
    }
}
