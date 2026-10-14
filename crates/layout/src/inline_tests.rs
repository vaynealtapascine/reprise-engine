//! Field feedback and keying (`docs/inline.md`): what a pass composes with,
//! when passes stop, and what an unsettled loop reports.

use std::collections::BTreeMap;

use reprise_doc::{Document, InlineId, InlineObject};

use super::*;
use crate::regions::{Feedback, Plan, feedback, finish_feedback};

fn id(n: u32) -> InlineId {
    InlineId::parse(&format!("1@1/{n}@1")).unwrap()
}

fn key(n: u32, instance: FieldInstance) -> FieldKey {
    FieldKey {
        id: id(n),
        instance,
    }
}

fn plan(fields: &[(FieldKey, &str)]) -> Plan {
    Plan {
        fields: fields.iter().map(|(k, v)| (*k, v.to_string())).collect(),
        ..Plan::default()
    }
}

#[test]
fn a_field_prefers_the_value_for_its_own_placement() {
    let page_number = InlineObject::field("page-number");
    let values: FieldValues = BTreeMap::from([
        (key(1, FieldInstance::Page(4)), "5".to_string()),
        (key(1, FieldInstance::Single), "9".to_string()),
    ]);
    let env = FieldEnv {
        page: 0,
        pages: 3,
        instance: FieldInstance::Single,
        values: Some(&values),
    };
    assert_eq!(env.text(id(1), &page_number), "9");
    assert_eq!(
        env.placed_as(FieldInstance::Page(4))
            .text(id(1), &page_number),
        "5"
    );
    // A placement with no value yet is guessed from the page being composed,
    // not borrowed from another placement of the same object.
    assert_eq!(
        env.placed_as(FieldInstance::Page(7))
            .text(id(1), &page_number),
        "1"
    );
    // Another object has no value at all.
    assert_eq!(env.text(id(2), &page_number), "1");
}

#[test]
fn guesses_come_from_the_page_being_composed() {
    let env = |page, pages| FieldEnv {
        page,
        pages,
        ..FieldEnv::UNPLACED
    };
    let guess =
        |function: &str, env: &FieldEnv<'_>| env.text(id(1), &InlineObject::field(function));
    assert_eq!(guess("page-number", &env(6, 7)), "7");
    assert_eq!(guess("page-count", &env(6, 7)), "7");
    // The pages made so far never fall behind the page being composed.
    assert_eq!(guess("page-count", &env(6, 1)), "7");
    assert_eq!(guess("page-of", &env(6, 7)), "?");
    let text = InlineObject::field("text").with("text", "verbatim");
    assert_eq!(env(6, 7).text(id(1), &text), "verbatim");
    // A name no function has composes as nothing.
    assert_eq!(guess("nope", &env(0, 1)), "");
}

#[test]
fn an_instance_is_left_out_of_the_wire_form_unless_it_is_a_page() {
    assert!(FieldInstance::Single.is_single());
    assert!(!FieldInstance::of_page(0).is_single());
    assert_eq!(
        serde_json::to_string(&FieldInstance::Single).unwrap(),
        "\"single\""
    );
    assert_eq!(
        serde_json::to_string(&FieldInstance::Page(3)).unwrap(),
        "{\"page\":3}"
    );
    // A page number that does not fit saturates rather than wrapping.
    assert_eq!(
        FieldInstance::of_page(usize::MAX),
        FieldInstance::Page(u32::MAX)
    );
    // Keys order by object, then placement, so output never depends on
    // hash order.
    assert!(key(1, FieldInstance::Single) < key(1, FieldInstance::Page(0)));
    assert!(key(1, FieldInstance::Page(9)) < key(2, FieldInstance::Single));
}

#[test]
fn equal_field_plans_converge_and_a_seen_plan_is_an_oscillation() {
    let a = plan(&[(key(1, FieldInstance::Single), "1")]);
    let b = plan(&[(key(1, FieldInstance::Single), "2")]);
    assert_eq!(feedback(&a, &a, &[], 0), Feedback::Converged);
    assert_eq!(feedback(&a, &b, &[], 0), Feedback::Continue);
    assert_eq!(
        feedback(&b, &a, std::slice::from_ref(&a), 1),
        Feedback::Oscillated
    );
    // Two placements of one object are two entries: both must agree.
    let one = plan(&[
        (key(1, FieldInstance::Page(0)), "1"),
        (key(1, FieldInstance::Page(1)), "2"),
    ]);
    let other = plan(&[
        (key(1, FieldInstance::Page(0)), "1"),
        (key(1, FieldInstance::Page(1)), "3"),
    ]);
    assert_eq!(feedback(&one, &other, &[], 0), Feedback::Continue);
    // The shared limit bounds the loop whatever the texts do.
    assert_eq!(
        feedback(&a, &b, &[], crate::regions::MAX_REGION_ITERATIONS - 1),
        Feedback::Exhausted
    );
}

fn empty_snapshot() -> LayoutSnapshot {
    let engine = crate::Engine::new(reprise_font::FontStore::default());
    engine.layout(&Document::new(1).unwrap())
}

#[test]
fn an_unsettled_field_loop_publishes_the_last_input_plan_with_a_warning() {
    let a = plan(&[(key(1, FieldInstance::Single), "1")]);
    let b = plan(&[(key(1, FieldInstance::Single), "2")]);
    for outcome in [Feedback::Oscillated, Feedback::Exhausted] {
        let mut snapshot = empty_snapshot();
        finish_feedback(&mut snapshot, &a, b.clone(), outcome);
        let codes: Vec<_> = snapshot
            .diagnostics
            .iter()
            .filter(|d| d.severity != Severity::Info)
            .map(|d| d.code.as_str())
            .collect();
        // Only the fields did not settle: the regions agree, so no region
        // code is reported for them.
        assert_eq!(codes, ["layout.field-unsettled"], "{outcome:?}");
        assert!(
            snapshot
                .diagnostics
                .iter()
                .all(|d| d.severity != Severity::Error)
        );
    }
}

#[test]
fn fields_that_settled_report_nothing_even_when_the_regions_did_not() {
    let mut a = plan(&[(key(1, FieldInstance::Single), "1")]);
    let mut b = a.clone();
    a.pages = 1;
    b.pages = 2;
    let mut snapshot = empty_snapshot();
    finish_feedback(&mut snapshot, &a, b, Feedback::Oscillated);
    let codes: Vec<_> = snapshot
        .diagnostics
        .iter()
        .map(|d| d.code.as_str())
        .collect();
    assert_eq!(codes, ["layout.region-cycle"]);
    // Both unsettled: both are said.
    let mut a = plan(&[(key(1, FieldInstance::Single), "1")]);
    let mut b = plan(&[(key(1, FieldInstance::Single), "2")]);
    a.pages = 1;
    b.pages = 2;
    let mut snapshot = empty_snapshot();
    finish_feedback(&mut snapshot, &a, b, Feedback::Exhausted);
    let codes: Vec<_> = snapshot
        .diagnostics
        .iter()
        .map(|d| d.code.as_str())
        .collect();
    assert_eq!(codes, ["layout.region-limit", "layout.field-unsettled"]);
}

#[test]
fn size_atoms_carries_the_placement_into_each_box() {
    // The same field composed for two pages shapes two different texts.
    let engine = crate::Engine::new(reprise_font::FontStore::default());
    let object = InlineObject::field("page-number");
    let atoms = vec![PreparedAtom {
        id: id(1),
        at: 0,
        object,
        fixed: None,
    }];
    let values: FieldValues = BTreeMap::from([
        (key(1, FieldInstance::Page(0)), "1".to_string()),
        (key(1, FieldInstance::Page(1)), "22".to_string()),
    ]);
    for (page, text) in [(0, "1"), (1, "22")] {
        let env = FieldEnv {
            page,
            pages: 2,
            instance: FieldInstance::of_page(page),
            values: Some(&values),
        };
        let mut notes = Vec::new();
        let sized = size_atoms(
            &engine,
            &atoms,
            &[],
            reprise_geom::Length::from_pt(10),
            AtomFrame::default(),
            &env,
            0,
            &crate::Subject::Document,
            &mut notes,
        );
        let one = sized.get(&0).unwrap();
        assert_eq!(one.instance, FieldInstance::of_page(page));
        // With no face to shape in, the box is empty but still carries the
        // instance; the text the plan chose is what `FieldEnv` returned.
        assert_eq!(env.text(id(1), &atoms[0].object), text);
    }
}
