use super::*;

#[test]
fn graph_cycles_missing_units_and_unified_inputs_are_total() {
    let node = NodeId::parse("1@1").unwrap();
    let range = RangeId::parse("2@1").unwrap();
    let mut graph = DependencyGraph::default();
    assert!(
        graph
            .dependencies(&Computation::Page(usize::MAX))
            .is_empty()
    );
    assert!(graph.dependents(&Dependency::Range(range)).is_empty());
    graph.record(
        Computation::Compose(node),
        BTreeSet::from([
            Dependency::Computed(Computation::Regions),
            Dependency::Node(node),
        ]),
        BTreeSet::new(),
    );
    graph.record(
        Computation::Regions,
        BTreeSet::from([Dependency::Computed(Computation::Compose(node))]),
        BTreeSet::new(),
    );
    assert!(
        graph
            .dependencies(&Computation::Regions)
            .contains(&Dependency::Node(node))
    );
    assert_eq!(
        graph.dependents(&Dependency::Node(node)),
        vec![Computation::Compose(node), Computation::Regions]
    );
    assert_eq!(
        Dependency::from(reprise_doc::relation::Dependency::Node(node)),
        Dependency::Node(node)
    );
    assert_eq!(
        Dependency::from(reprise_doc::relation::Dependency::Range(range)),
        Dependency::Range(range)
    );
    assert!(matches!(
        Dependency::from(reprise_doc::Dependency::Em),
        Dependency::Expression(reprise_doc::Dependency::Em)
    ));
}

#[test]
fn deep_graph_inspection_uses_no_recursive_stack() {
    let mut graph = DependencyGraph::default();
    graph.record(
        Computation::Page(0),
        BTreeSet::from([Dependency::Template]),
        BTreeSet::new(),
    );
    for page in 1..16_384 {
        graph.record(
            Computation::Page(page),
            BTreeSet::from([Dependency::Computed(Computation::Page(page - 1))]),
            BTreeSet::new(),
        );
    }
    assert!(
        graph
            .dependencies(&Computation::Page(16_383))
            .contains(&Dependency::Template)
    );
    assert_eq!(graph.dependents(&Dependency::Template).len(), 16_384);
}

#[test]
fn empty_reversed_and_extreme_viewports_do_not_overflow() {
    assert_eq!(Viewport::Pages(8..8).end(), 0);
    let reverse = std::ops::Range { start: 8, end: 2 };
    assert_eq!(Viewport::Pages(reverse).end(), 0);
    assert_eq!(
        Viewport::Rect {
            page: usize::MAX,
            rect: Rect::new(
                reprise_geom::Point::new(reprise_geom::Length::MIN, reprise_geom::Length::MAX),
                reprise_geom::Length::MIN,
                reprise_geom::Length::MAX
            )
        }
        .end(),
        usize::MAX
    );
}
