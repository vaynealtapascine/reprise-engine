use super::*;
use crate::context::{Axis, Extent, Level};
use crate::function::{Builtin, Signature};

fn pt(n: i32) -> Length {
    Length::from_pt(n)
}

fn reg() -> FunctionRegistry {
    FunctionRegistry::builtin()
}

fn parse(s: &str) -> Expr {
    Expr::parse(s).unwrap_or_else(|e| panic!("{s:?} should parse: {e}"))
}

fn codes_of(notes: &[Note]) -> Vec<String> {
    notes.iter().map(|n| n.code.as_str().to_string()).collect()
}

/// Computes with a free `em` and `lh` and a 200pt percentage basis, then
/// resolves with a 10pt font size and a 12pt line height.
fn run(text: &str, ctx: &ResolutionContext) -> UsedLength {
    let functions = reg();
    let scope = Scope {
        em: Term::Free,
        lh: Term::Free,
        percent: Term::Bound(ComputedLength::Absolute(pt(200))),
    };
    let computed = parse(text)
        .compute(&scope, &functions)
        .unwrap_or_else(|n| panic!("{text:?} should compute: {n:?}"));
    computed.used(&UsedEnv {
        functions: &functions,
        context: ctx,
        em: Some(pt(10)),
        lh: Some(pt(12)),
    })
}

fn clean(text: &str) -> Length {
    let u = run(text, &ResolutionContext::default());
    assert!(
        u.notes.is_empty() && !u.degraded,
        "{text:?} should be clean: {:?}",
        u.notes
    );
    u.value
}

fn failure_codes(text: &str) -> Vec<String> {
    let scope = Scope::known(pt(10), pt(12), Some(pt(200)));
    match parse(text).compute(&scope, &reg()) {
        Ok(c) => panic!("{text:?} should not compute, got {c:?}"),
        Err(notes) => codes_of(&notes),
    }
}

// --- text form -------------------------------------------------------------

#[test]
fn canonical_spelling() {
    for (written, canonical) in [
        ("12pt", "12pt"),
        ("calc(12pt+0.50em)", "12pt + 0.5em"),
        ("  1.50em", "1.5em"),
        ("007.0pt", "7pt"),
        (".5em", "0.5em"),
        ("(1pt + 2pt) * 3", "(1pt + 2pt) * 3"),
        ("((1pt) + (2pt)) + 3pt", "1pt + 2pt + 3pt"),
        ("1pt - (2pt - 3pt)", "1pt - (2pt - 3pt)"),
        ("1pt - 2pt - 3pt", "1pt - 2pt - 3pt"),
        ("1pt+2pt*3", "1pt + 2pt * 3"),
        ("-(1pt + 2pt)", "-(1pt + 2pt)"),
        ("+5pt", "5pt"),
        ("- -5pt", "-(-5pt)"),
        ("min(1pt,2pt , 3pt)", "min(1pt, 2pt, 3pt)"),
        ("clamp(1pt, 50%, 3pt)", "clamp(1pt, 50%, 3pt)"),
        ("frame-width", "frame-width"),
        ("frame-width( \"main\" )", "frame-width(\"main\")"),
        ("nearest-height(\"block\")", "nearest-height(\"block\")"),
        ("scale(12pt,ratio(3,2))", "scale(12pt, ratio(3, 2))"),
        ("frob()", "frob()"),
    ] {
        let e = parse(written);
        assert_eq!(e.to_string(), canonical, "{written:?}");
        assert_eq!(parse(canonical), e, "{canonical:?} reads back the same");
    }
}

#[test]
fn names_with_quotes_and_backslashes_round_trip() {
    let e = parse(r#"frame-width("a\"b\\c") + 1pt"#);
    let Node::Binary(_, a, _) = e.node() else {
        panic!("a sum")
    };
    assert_eq!(
        **a,
        Node::Basis(Basis::frame("a\"b\\c", Axis::Width)),
        "escapes are undone"
    );
    assert_eq!(parse(&e.to_string()), e);
}

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }
}

fn generate(r: &mut Rng, depth: u32) -> Node {
    if depth == 0 || r.below(4) == 0 {
        return match r.below(8) {
            0 => Node::Basis(Basis::exact(
                Level::ALL[r.below(5) as usize],
                [Axis::Width, Axis::Height][r.below(2) as usize],
            )),
            1 => Node::Basis(Basis::frame("m\"x", Axis::Height)),
            2 => Node::Basis(Basis::nearest(Level::ALL[r.below(5) as usize], Axis::Width)),
            _ => Node::Quantity(
                Decimal::new(r.below(1_000_000), r.below(6) as u8).unwrap_or(Decimal::ZERO),
                [Unit::None, Unit::Pt, Unit::Em, Unit::Lh, Unit::Percent][r.below(5) as usize],
            ),
        };
    }
    let sub = |r: &mut Rng| generate(r, depth - 1);
    match r.below(7) {
        0 => Node::Neg(Box::new(sub(r))),
        1..=3 => {
            let op = [Op::Add, Op::Sub, Op::Mul, Op::Div][r.below(4) as usize];
            Node::binary(op, sub(r), sub(r))
        }
        4 => Node::Min((0..=r.below(3)).map(|_| sub(r)).collect()),
        5 => Node::Clamp(Box::new(sub(r)), Box::new(sub(r)), Box::new(sub(r))),
        _ => Node::Call("ratio".into(), vec![sub(r), sub(r)]),
    }
}

#[test]
fn generated_expressions_round_trip() {
    let mut rng = Rng(7);
    for _ in 0..2000 {
        let node = generate(&mut rng, 4);
        let Ok(e) = Expr::new(node) else { continue };
        let text = e.to_string();
        let back = parse(&text);
        assert_eq!(back, e, "{text}");
        assert_eq!(back.to_string(), text, "printing is stable");
    }
}

#[test]
fn serde_uses_the_text_form() {
    let e = parse("1.5em + 2pt");
    let json = serde_json::to_string(&e).unwrap();
    assert_eq!(json, "\"1.5em + 2pt\"");
    assert_eq!(serde_json::from_str::<Expr>(&json).unwrap(), e);
    assert!(serde_json::from_str::<Expr>("\"1.5furlongs\"").is_err());
}

#[test]
fn malformed_text_is_an_error_not_a_panic() {
    for bad in [
        "",
        "   ",
        "1pt +",
        "* 2",
        "1 pt",
        "12furlongs",
        "1pt 2pt",
        "(1pt",
        "1pt)",
        "frame-width(5)",
        "frame-width(\"unterminated)",
        "frame-width(\"a\\n\")",
        "page-width(\"x\")",
        "nearest-width",
        "nearest-width(\"galaxy\")",
        "clamp(1pt)",
        "clamp(1pt, 2pt, 3pt, 4pt)",
        "min()",
        "calc()",
        "calc(1pt, 2pt)",
        "foo",
        "Foo(1pt)",
        "1.",
        "1.2.3",
        "e",
        "1pt,2pt",
        "ratio(1,)",
        "\u{1f4a5}",
    ] {
        assert!(Expr::parse(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn literals_are_bounded() {
    assert!(matches!(
        Expr::parse("1234567890123456789pt"),
        Err(ExprError::Literal { .. })
    ));
    assert!(matches!(
        Expr::parse("0.1234567890123pt"),
        Err(ExprError::Literal { .. })
    ));
    assert!(Expr::parse("999999999999999999pt").is_ok());
    assert!(Expr::parse("0.123456789012pt").is_ok());
}

// --- bounds ----------------------------------------------------------------

#[test]
fn deep_nesting_is_rejected_without_exhausting_the_stack() {
    for text in [
        "(".repeat(2000),
        "-".repeat(2000) + "1pt",
        "min(".repeat(500),
        format!("{}1pt{}", "(".repeat(40), ")".repeat(40)),
    ] {
        assert_eq!(
            Expr::parse(&text).err(),
            Some(ExprError::Limit(Limit::Depth)),
            "{}",
            &text[..text.len().min(20)]
        );
    }
    // Exactly at the limit is fine.
    let ok = format!("{}1pt{}", "(".repeat(30), ")".repeat(30));
    assert!(Expr::parse(&ok).is_ok());
    let sum = vec!["1pt"; 33].join(" + ");
    assert_eq!(
        Expr::parse(&sum).err(),
        Some(ExprError::Limit(Limit::Depth)),
        "a long left-leaning chain is as deep as it is long"
    );
}

#[test]
fn too_many_nodes_and_too_much_text_are_rejected() {
    // A flat argument list is wide, not deep, so the node limit is what stops it.
    let exactly = vec!["2"; 255].join(", ");
    let e = Expr::parse(&format!("min({exactly})")).unwrap();
    assert_eq!(e.node_count(), MAX_NODES);
    let one_more = vec!["2"; 256].join(", ");
    assert_eq!(
        Expr::parse(&format!("min({one_more})")).err(),
        Some(ExprError::Limit(Limit::Nodes))
    );
    assert_eq!(
        Expr::parse(&" ".repeat(MAX_TEXT + 1)).err(),
        Some(ExprError::Limit(Limit::Text))
    );
    assert_eq!(
        ExprError::Limit(Limit::Nodes).code().as_str(),
        "style.expr-limit"
    );
    assert_eq!(
        ExprError::Literal { at: 0 }.code().as_str(),
        "style.unparsed"
    );
}

#[test]
fn built_trees_are_bounded_too() {
    let mut node = Node::Quantity(Decimal::ZERO, Unit::Pt);
    for _ in 0..100_000 {
        node = Node::Neg(Box::new(node));
    }
    // `Expr::new` walks it with an explicit stack and stops at the limit.
    assert_eq!(Expr::new(node).err(), Some(ExprError::Limit(Limit::Depth)));

    for bad in [
        Node::Min(vec![]),
        Node::Call("min".into(), vec![]),
        Node::Call("Frob".into(), vec![]),
        Node::Call("frame-width".into(), vec![]),
        Node::Basis(Basis::frame("x", Axis::Width)),
        Node::Basis(Basis {
            level: Level::Page,
            axis: Axis::Width,
            scope: BasisScope::Named("x".into()),
        }),
    ] {
        let r = Expr::new(bad.clone());
        if matches!(&bad, Node::Basis(b) if b.level == Level::Frame) {
            assert!(r.is_ok());
        } else {
            assert!(matches!(r, Err(ExprError::Shape(_))), "{bad:?}");
        }
    }
}

// --- types -----------------------------------------------------------------

#[test]
fn dimensions_are_checked_before_evaluation() {
    let r = reg();
    let ty = |s: &str| parse(s).check(&r);
    assert_eq!(ty("1pt + 2em"), Ok(Dim::Length));
    assert_eq!(ty("2 + 3"), Ok(Dim::Number));
    assert_eq!(ty("10% + 5%"), Ok(Dim::Percentage));
    assert_eq!(ty("10% + 5pt"), Ok(Dim::Length));
    assert_eq!(ty("5pt + 10%"), Ok(Dim::Length));
    assert_eq!(ty("2 * 3"), Ok(Dim::Number));
    assert_eq!(ty("2 * 3pt"), Ok(Dim::Length));
    assert_eq!(ty("3pt * 2"), Ok(Dim::Length));
    assert_eq!(ty("50% * 2"), Ok(Dim::Percentage));
    assert_eq!(ty("50% * 3pt"), Ok(Dim::Length));
    assert_eq!(ty("3pt / 2"), Ok(Dim::Length));
    assert_eq!(ty("3pt / 2pt"), Ok(Dim::Number));
    assert_eq!(ty("50% / 2"), Ok(Dim::Percentage));
    assert_eq!(ty("-3pt"), Ok(Dim::Length));
    assert_eq!(ty("min(1pt, 2pt)"), Ok(Dim::Length));
    assert_eq!(ty("max(1, 2)"), Ok(Dim::Number));
    assert_eq!(ty("min(1pt, 50%)"), Ok(Dim::Length));
    assert_eq!(ty("clamp(1pt, 2pt, 3pt)"), Ok(Dim::Length));
    assert_eq!(ty("ratio(16, 9)"), Ok(Dim::Ratio));
    assert_eq!(ty("scale(1pt, ratio(1, 2))"), Ok(Dim::Length));
    assert_eq!(ty("scale(50%, ratio(1, 2))"), Ok(Dim::Length));
    assert_eq!(ty("frame-width * 0.5"), Ok(Dim::Length));

    for (bad, why) in [
        ("1pt * 2pt", "length times length"),
        ("1pt + 2", "length plus number"),
        ("2 + 50%", "number plus percentage"),
        ("50% * 50%", "percentage times percentage"),
        ("1pt / 50%", "length over percentage"),
        ("2 / 1pt", "number over length"),
        ("-ratio(1, 2)", "negated ratio"),
        ("ratio(1, 2) + ratio(1, 2)", "ratio sum"),
        ("min(1pt, 2)", "mixed min"),
        ("max(ratio(1, 2))", "max of ratios"),
        ("clamp(1, 2pt, 3)", "mixed clamp"),
        ("ratio(1pt, 2)", "wrong argument type"),
        ("ratio(1)", "too few arguments"),
        ("ratio(1, 2, 3)", "too many arguments"),
        ("scale(1pt, 2)", "number for ratio"),
    ] {
        assert!(
            matches!(ty(bad), Err(CheckError::Type(_))),
            "{bad} ({why}) should be a type error, got {:?}",
            ty(bad)
        );
    }
}

#[test]
fn only_lengths_are_computed_for_properties() {
    assert_eq!(failure_codes("2"), ["style.type-error"]);
    assert_eq!(failure_codes("ratio(1, 2)"), ["style.type-error"]);
    assert_eq!(failure_codes("1pt * 2pt"), ["style.type-error"]);
}

#[test]
fn unknown_functions_are_kept_and_reported() {
    let e = parse("frob(1pt) + 2pt");
    assert_eq!(e.to_string(), "frob(1pt) + 2pt");
    assert_eq!(
        e.check(&reg()),
        Err(CheckError::UnknownFunction("frob".into()))
    );
    assert_eq!(failure_codes("frob(1pt)"), ["style.unknown-function"]);
    // Not even an empty registry knows the built-ins.
    let none = FunctionRegistry::empty();
    assert_eq!(
        parse("ratio(1, 2)").check(&none),
        Err(CheckError::UnknownFunction("ratio".into()))
    );
    // And the dependency set still says it was called.
    assert!(
        e.dependencies(&reg())
            .contains(&Dependency::Function("frob".into()))
    );
}

// --- units and evaluation --------------------------------------------------

#[test]
fn every_unit_and_basis() {
    assert_eq!(clean("12pt"), pt(12));
    assert_eq!(clean("1.5pt"), Length(1536));
    assert_eq!(clean("1.5em"), pt(15), "em is the font size");
    assert_eq!(clean("2lh"), pt(24), "lh is the line height");
    assert_eq!(clean("50%"), pt(100), "% is of the declared basis");
    assert_eq!(clean("50% * 10pt"), pt(5), "% scales a length");
    assert_eq!(clean("10% + 5pt"), pt(25));
    assert_eq!(clean("-3pt"), pt(-3));
    assert_eq!(clean("2 * 3pt"), pt(6));
    assert_eq!(clean("7pt / 2"), Length(3584));
    assert_eq!(
        clean("6pt / 3pt * 2pt"),
        pt(4),
        "a length ratio is a number"
    );
    assert_eq!(clean("1pt + 2pt * 3"), pt(7));
    assert_eq!(clean("(1pt + 2pt) * 3"), pt(9));
    assert_eq!(clean("min(1pt, 2pt, 3pt)"), pt(1));
    assert_eq!(clean("max(1pt, 2pt, 3pt)"), pt(3));
    assert_eq!(clean("min(10%, 30pt)"), pt(20));
    assert_eq!(clean("clamp(1pt, 5pt, 8pt)"), pt(5));
    assert_eq!(clean("clamp(1pt, 0pt, 8pt)"), pt(1));
    assert_eq!(clean("clamp(1pt, 99pt, 8pt)"), pt(8));
    assert_eq!(
        clean("clamp(10pt, 5pt, 8pt)"),
        pt(10),
        "the lower bound wins when the bounds cross"
    );
    assert_eq!(clean("scale(12pt, ratio(3, 2))"), pt(18));
    assert_eq!(clean("round-to(13pt, 5pt)"), pt(15));
    assert_eq!(clean("round-to(-13pt, 5pt)"), pt(-15));
    assert_eq!(
        clean("round-to(12.5pt, 5pt)"),
        pt(15),
        "halves round away from zero"
    );
}

#[test]
fn division_rounds_half_away_from_zero() {
    let unit = "0.0009765625pt"; // exactly one sub-unit
    assert_eq!(clean(unit), Length(1));
    assert_eq!(clean(&format!("1.5 * {unit}")), Length(2));
    assert_eq!(clean(&format!("-1.5 * {unit}")), Length(-2));
    assert_eq!(clean(&format!("{unit} / 2")), Length(1));
    assert_eq!(clean(&format!("-{unit} / 2")), Length(-1));
    assert_eq!(clean(&format!("{unit} / 3")), Length(0));
    assert_eq!(clean(&format!("{unit} * 3 / 2")), Length(2));
}

#[test]
fn bases_resolve_against_the_context() {
    let ctx = ResolutionContext::default()
        .with_medium(Extent::definite(pt(1000), pt(900)))
        .with_page(Extent::definite(pt(600), pt(800)))
        .with_current_frame("main", Extent::auto_height(pt(400)))
        .with_named_frame("margin", Extent::definite(pt(100), pt(700)))
        .with_block(Extent::auto_height(pt(380)))
        .with_line(Extent::auto_height(pt(300)));
    let v = |s: &str| {
        let u = run(s, &ctx);
        assert!(u.notes.is_empty(), "{s}: {:?}", u.notes);
        u.value
    };
    assert_eq!(v("medium-width"), pt(1000));
    assert_eq!(v("medium-height"), pt(900));
    assert_eq!(v("page-width"), pt(600));
    assert_eq!(v("page-height"), pt(800));
    assert_eq!(v("frame-width"), pt(400));
    assert_eq!(v("frame-width(\"main\")"), pt(400));
    assert_eq!(v("frame-width(\"margin\")"), pt(100));
    assert_eq!(v("frame-height(\"margin\")"), pt(700));
    assert_eq!(v("block-width"), pt(380));
    assert_eq!(v("line-width"), pt(300));
    assert_eq!(v("nearest-width(\"line\")"), pt(300));
    assert_eq!(v("nearest-height(\"page\")"), pt(800));
    assert_eq!(v("50% * frame-width(\"main\") - 2em"), pt(180));
    let u = run("frame-width(\"margin\")", &ctx);
    assert_eq!(u.bases, ["frame-width(\"margin\") = 100pt"]);
}

#[test]
fn an_unresolved_basis_counts_as_zero_and_is_reported() {
    for text in [
        "frame-width",
        "page-height + 3pt",
        "medium-width",
        "frame-width(\"nowhere\")",
        "block-width",
        "nearest-width(\"line\")",
    ] {
        let u = run(text, &ResolutionContext::default());
        assert!(u.degraded, "{text}");
        assert_eq!(codes_of(&u.notes), ["style.basis-unresolved"], "{text}");
    }
    let u = run("page-height + 3pt", &ResolutionContext::default());
    assert_eq!(u.value, pt(3), "the missing term is zero");
}

#[test]
fn an_indefinite_basis_counts_as_zero_and_is_reported() {
    let ctx = ResolutionContext::default()
        .with_page(Extent::definite(pt(600), pt(800)))
        .with_current_frame("main", Extent::auto_height(pt(400)));
    for text in [
        "50% * frame-height",
        "frame-height(\"main\")",
        "nearest-height(\"frame\")",
    ] {
        let u = run(text, &ctx);
        assert!(u.degraded, "{text}");
        assert_eq!(codes_of(&u.notes), ["style.basis-indefinite"], "{text}");
        assert_eq!(u.value, Length::ZERO);
    }
}

#[test]
fn a_percentage_with_no_basis_is_unresolved() {
    let scope = Scope {
        em: Term::Free,
        lh: Term::Free,
        percent: Term::Unavailable,
    };
    let r = parse("10% + 1pt").compute(&scope, &reg());
    assert_eq!(codes_of(&r.unwrap_err()), ["style.basis-unresolved"]);
    // Scaling a length needs no basis.
    assert!(parse("10% * 1pt").compute(&scope, &reg()).is_ok());
}

#[test]
fn cyclic_terms_are_reported() {
    let scope = Scope {
        em: Term::Free,
        lh: Term::Cyclic,
        percent: Term::Unavailable,
    };
    let r = parse("2lh").compute(&scope, &reg());
    assert_eq!(codes_of(&r.unwrap_err()), ["style.cycle"]);
    assert!(parse("2em + 1pt").compute(&scope, &reg()).is_ok());
}

#[test]
fn arithmetic_saturates_and_says_so() {
    let u = run("1000000000pt * 1000000000", &ResolutionContext::default());
    assert_eq!(u.value, Length::MAX);
    assert_eq!(codes_of(&u.notes), ["style.saturated"]);
    assert!(!u.degraded, "a saturated value is still the value");

    let u = run("-1000000000pt * 1000000000", &ResolutionContext::default());
    assert_eq!(u.value, Length::MIN);
    assert_eq!(codes_of(&u.notes), ["style.saturated"]);

    let u = run("2147483647pt + 1pt", &ResolutionContext::default());
    assert_eq!(u.value, Length::MAX);
    let u = run("-(-1000000000pt - 1pt)", &ResolutionContext::default());
    assert_eq!(
        u.value,
        Length::MAX,
        "negating the saturated minimum saturates"
    );
    assert_eq!(codes_of(&u.notes), ["style.saturated"]);
}

#[test]
fn division_by_zero_saturates_by_sign() {
    let ctx = ResolutionContext::default();
    let u = run("5pt / 0", &ctx);
    assert_eq!(
        (u.value, codes_of(&u.notes)),
        (Length::MAX, vec!["style.divide-by-zero".to_string()])
    );
    let u = run("-5pt / 0", &ctx);
    assert_eq!(u.value, Length::MIN);
    let u = run("0pt / 0", &ctx);
    assert_eq!(u.value, Length::ZERO);
    assert_eq!(codes_of(&u.notes), ["style.divide-by-zero"]);
    // A length over a zero length is a number, saturated; it scales 1pt.
    let u = run("5pt / (3pt - 3pt) * 1pt", &ctx);
    assert_eq!(u.value, Length(33_554_432));
    assert_eq!(codes_of(&u.notes), ["style.divide-by-zero"]);
    let u = run("(1 / 0) * 1pt", &ctx);
    assert_eq!(u.value, Length(33_554_432));
}

#[test]
fn folding_leaves_what_it_cannot_settle() {
    let scope = Scope {
        em: Term::Bound(ComputedLength::Absolute(pt(10))),
        lh: Term::Free,
        percent: Term::Unavailable,
    };
    let c = |s: &str| parse(s).compute(&scope, &reg()).unwrap();
    assert_eq!(c("1pt + 2pt"), ComputedLength::Absolute(pt(3)));
    assert_eq!(c("1.5em"), ComputedLength::Absolute(pt(15)));
    assert_eq!(c("2 * 3pt - 1pt"), ComputedLength::Absolute(pt(5)));
    assert_eq!(c("frame-width + 2 * 1em").describe(), "frame-width + 20pt");
    assert_eq!(
        c("min(page-width, 2pt * 3)").describe(),
        "min(page-width, 6pt)"
    );
    // Saturation is not baked into a literal; the used stage reports it.
    let ComputedLength::Symbolic(e) = c("1000000000pt * 1000000000") else {
        panic!("should stay symbolic")
    };
    assert_eq!(e.to_string(), "1000000000pt * 1000000000");
    // A failing function is not folded either.
    assert!(matches!(
        c("scale(1pt, ratio(1, 0))"),
        ComputedLength::Symbolic(_)
    ));
}

// --- functions ---------------------------------------------------------------

#[test]
fn function_failures_are_reported_not_trusted() {
    let u = run("scale(1pt, ratio(1, 0))", &ResolutionContext::default());
    assert!(u.degraded);
    assert!(
        codes_of(&u.notes)
            .iter()
            .all(|c| c == "style.function-failed")
    );
    assert_eq!(u.value, Length::ZERO);

    let mut functions = reg();
    functions
        .register(
            "liar",
            Builtin::new(Signature::new(&[Dim::Length], Dim::Length), |_| {
                Ok(Value::Number(Fixed::ONE))
            }),
        )
        .unwrap();
    let scope = Scope::known(pt(10), pt(12), None);
    let computed = parse("liar(1pt)").compute(&scope, &functions).unwrap();
    let ctx = ResolutionContext::default();
    let u = computed.used(&UsedEnv {
        functions: &functions,
        context: &ctx,
        em: None,
        lh: None,
    });
    assert!(u.degraded);
    assert_eq!(codes_of(&u.notes), ["style.function-failed"]);
}

#[test]
fn registered_functions_are_checked_and_called() {
    let mut functions = FunctionRegistry::builtin();
    functions
        .register(
            "double",
            Builtin::new(
                Signature::new(&[Dim::Length], Dim::Length),
                |args| match args {
                    [Value::Length(l)] => Ok(Value::Length(*l + *l)),
                    _ => Err(crate::function::FunctionError("a length".into())),
                },
            ),
        )
        .unwrap();
    let scope = Scope::known(pt(10), pt(12), Some(pt(200)));
    let ctx = ResolutionContext::default();
    let used = |s: &str| {
        parse(s)
            .compute(&scope, &functions)
            .unwrap()
            .used(&UsedEnv {
                functions: &functions,
                context: &ctx,
                em: None,
                lh: None,
            })
            .value
    };
    assert_eq!(used("double(2em)"), pt(40));
    assert_eq!(
        used("double(10%)"),
        pt(40),
        "a percentage becomes a length for a length parameter"
    );
    assert_eq!(
        parse("double(2em)").dependencies(&functions),
        BTreeSet::from([Dependency::Em, Dependency::Function("double".into())])
    );
}

#[test]
fn registry_refuses_bad_names() {
    use crate::function::RegistryError;
    let mut r = FunctionRegistry::empty();
    let f = || {
        Builtin::new(Signature::new(&[], Dim::Number), |_| {
            Ok(Value::Number(Fixed::ONE))
        })
    };
    assert_eq!(
        r.register("Bad", f()),
        Err(RegistryError::BadName("Bad".into()))
    );
    assert_eq!(r.register("", f()), Err(RegistryError::BadName("".into())));
    assert_eq!(
        r.register("a b", f()),
        Err(RegistryError::BadName("a b".into()))
    );
    assert_eq!(
        r.register("frame-width", f()),
        Err(RegistryError::BadName("frame-width".into()))
    );
    for reserved in ["calc", "min", "max", "clamp"] {
        assert_eq!(
            r.register(reserved, f()),
            Err(RegistryError::Reserved(reserved.into()))
        );
    }
    assert!(r.register("one", f()).is_ok());
    assert_eq!(
        r.register("one", f()),
        Err(RegistryError::Duplicate("one".into()))
    );
    assert_eq!(r.names().collect::<Vec<_>>(), ["one"]);
}

#[test]
fn builtins_are_registered() {
    let r = reg();
    assert_eq!(
        r.names().collect::<Vec<_>>(),
        ["ratio", "round-to", "scale"]
    );
}

// --- dependencies ------------------------------------------------------------

#[test]
fn dependencies_are_an_ordered_set() {
    let deps = |s: &str| -> Vec<String> {
        parse(s)
            .dependencies(&reg())
            .iter()
            .map(|d| d.to_string())
            .collect()
    };
    assert_eq!(deps("12pt"), Vec::<String>::new());
    assert_eq!(deps("1.5em"), ["em"]);
    assert_eq!(deps("1lh + 1em"), ["em", "lh"]);
    assert_eq!(deps("50%"), ["percentage basis"]);
    assert_eq!(
        deps("50% * 10pt"),
        Vec::<String>::new(),
        "scaling needs no basis"
    );
    assert_eq!(
        deps("50% * 2"),
        ["percentage basis"],
        "a bare percentage does"
    );
    assert_eq!(
        deps("page-width + frame-width(\"main\") + 2em + scale(1pt, ratio(1, 2)) + 5%"),
        [
            "em",
            "percentage basis",
            "page-width",
            "frame-width(\"main\")",
            "function ratio",
            "function scale"
        ]
    );
    // The same set in any order of writing.
    assert_eq!(deps("1em + 1lh"), deps("1lh + 1em"));
    // An expression that doesn't type-check still reports every term it has.
    assert_eq!(
        deps("1pt * 2pt + 3em + frob(4%)"),
        ["em", "percentage basis", "function frob"]
    );
}

#[test]
fn computed_dependencies_are_what_remains() {
    let scope = Scope {
        em: Term::Bound(ComputedLength::Absolute(pt(10))),
        lh: Term::Cyclic,
        percent: Term::Bound(ComputedLength::Absolute(pt(10))),
    };
    let c = parse("2em + 50% + frame-width")
        .compute(&scope, &reg())
        .unwrap();
    let deps = c.dependencies(&reg());
    assert_eq!(
        deps.iter().map(|d| d.to_string()).collect::<Vec<_>>(),
        ["frame-width"],
        "the em and the percentage are bound; only the frame is left"
    );
}

#[test]
fn random_text_never_panics_and_what_parses_round_trips() {
    let pieces = [
        "(",
        ")",
        "1",
        "2.5",
        ".",
        "pt",
        "em",
        "lh",
        "%",
        "é",
        "\"",
        "\\",
        "-",
        "+",
        "*",
        "/",
        ",",
        " ",
        "min",
        "max",
        "clamp",
        "calc",
        "frame-width",
        "nearest-height",
        "ratio",
        "💥",
        "e",
        "0",
    ];
    let mut rng = Rng(99);
    let mut parsed = 0;
    for _ in 0..20_000 {
        let text: String = (0..rng.below(14))
            .map(|_| pieces[rng.below(pieces.len() as u64) as usize])
            .collect();
        if let Ok(e) = Expr::parse(&text) {
            parsed += 1;
            assert_eq!(parse(&e.to_string()), e, "{text:?}");
            // Checking and evaluating whatever parsed must not panic either.
            let _ = e.check(&reg());
            let _ = e.dependencies(&reg());
            let _ = e.compute(&Scope::known(pt(10), pt(12), Some(pt(100))), &reg());
        }
    }
    assert!(
        parsed > 100,
        "the generator should hit valid text sometimes: {parsed}"
    );
}
