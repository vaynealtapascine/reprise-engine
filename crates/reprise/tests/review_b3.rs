// Section, story and region commands, the page queries and navigation in
// running regions, only through the facade (sections S11).
use reprise::*;
use serde_json::json;

fn session() -> DocumentSession {
    let mut s = Workspace::new()
        .create(&Payload::new(Create {
            document_id: "00112233445566778899aabbccddeeff".into(),
            peer_id: "1".into(),
        }))
        .unwrap();
    s.declare_font(
        &Payload::new(FontDeclaration {
            family: "Source Serif Pro".into(),
            weight: 400,
            style: FontStyle::Normal,
            stretch: 1000,
            face_index: 0,
        }),
        include_bytes!("../../../fixtures/fonts/SourceSerifPro-Regular.otf"),
    )
    .unwrap();
    s
}

fn style() -> Style {
    Style {
        families: Some(vec!["Source Serif Pro".into(), "serif".into()]),
        ..Style::default()
    }
}

fn tx(s: &mut DocumentSession, commands: Vec<Command>) -> Applied {
    s.apply(&Payload::new(Transaction { commands }))
        .unwrap_or_else(|e| panic!("{e}"))
        .data
}

fn paragraph(text: &str, index: u32) -> Command {
    Command::InsertBlock {
        parent: None,
        index,
        block_kind: BlockKind::Paragraph,
        text: text.into(),
        style: style(),
    }
}

fn layout(s: &mut DocumentSession) -> LayoutProgress {
    let mut job = s
        .start_layout(&Payload::new(LayoutOptions::default()))
        .unwrap();
    for _ in 0..100_000 {
        let progress = job.step(s, 8).unwrap().data;
        if progress.complete {
            return progress;
        }
    }
    panic!("layout did not finish")
}

fn story_block(text: &str) -> StoryBlock {
    StoryBlock {
        block_kind: BlockKind::Paragraph,
        text: text.into(),
        style: Some(String::new()),
    }
}

/// Four paragraphs; a header story bound by `first`; a page break before the
/// third; a section starting inside the fourth, in two columns.
fn built() -> (DocumentSession, Vec<String>, String) {
    let mut s = session();
    let blocks = tx(
        &mut s,
        (0..4)
            .map(|i| paragraph(&format!("paragraph number {i}"), i))
            .collect(),
    )
    .blocks;
    let story = tx(
        &mut s,
        vec![Command::CreateStory {
            blocks: vec![story_block("Running head")],
        }],
    )
    .stories
    .remove(0);
    tx(
        &mut s,
        vec![
            Command::SetRegion {
                section: "first".into(),
                slot: "header".into(),
                variant: "default".into(),
                binding: RegionBinding::Story {
                    story: story.clone(),
                },
            },
            Command::InsertBreak {
                at: BreakPosition::Before {
                    node: blocks[2].clone(),
                },
                break_kind: BreakKind::Page,
                patch: None,
            },
        ],
    );
    let applied = tx(
        &mut s,
        vec![
            Command::InsertBreak {
                at: BreakPosition::Position {
                    node: blocks[3].clone(),
                    offset: 10,
                },
                break_kind: BreakKind::Section,
                patch: json!({"columns.count": 2, "numbering.format": "upper-roman"})
                    .as_object()
                    .cloned(),
            },
            Command::SetSection {
                section: "first".into(),
                patch: json!({"numbering.format": "lower-roman"})
                    .as_object()
                    .cloned(),
                clear: None,
            },
        ],
    );
    // The split made the new paragraph, and the section starts at it.
    assert_eq!(applied.blocks.len(), 1);
    (s, blocks, story)
}

#[test]
fn commands_make_sections_stories_and_bindings_that_state_reports() {
    let (s, blocks, story) = built();
    let state = s.state().unwrap().data;
    assert_eq!(state.blocks.len(), 5);
    // `first` and the section the split started.
    assert_eq!(state.sections.len(), 2);
    assert_eq!(state.sections[0].id, "first");
    assert_eq!(
        state.sections[0].own["raw"]["numbering.format"],
        "lower-roman"
    );
    assert_eq!(state.sections[1].own["raw"]["columns.count"], "2");
    let header = state.sections[0]
        .bindings
        .iter()
        .find(|b| b.slot == "header" && b.variant == "default")
        .unwrap();
    assert_eq!(
        (header.authored.as_str(), header.story.as_deref()),
        ("story", Some(story.as_str()))
    );
    // The second section inherits it.
    let inherited = state.sections[1]
        .bindings
        .iter()
        .find(|b| b.slot == "header" && b.variant == "default")
        .unwrap();
    assert_eq!(inherited.authored, "inherit");
    assert_eq!(inherited.resolved.as_deref(), Some(story.as_str()));
    assert_eq!(state.stories.len(), 1);
    assert_eq!(state.stories[0].blocks[0].text, "Running head");
    // The page break is on the third paragraph and the section on the split one.
    assert_eq!(state.blocks[2].starts.as_ref().unwrap().kind, "page");
    assert_eq!(state.blocks[4].starts.as_ref().unwrap().kind, "section");
    let _ = blocks;
}

#[test]
fn bindings_change_inherit_none_and_copy() {
    let (mut s, _, story) = built();
    let second = s.state().unwrap().data.sections[1].id.clone();
    // Explicit none for the second section; a copy for a footer; then link back.
    let applied = tx(
        &mut s,
        vec![
            Command::SetRegion {
                section: second.clone(),
                slot: "header".into(),
                variant: "default".into(),
                binding: RegionBinding::None,
            },
            Command::CreateRegion {
                section: second.clone(),
                slot: "footer".into(),
                variant: "default".into(),
                from: StoryFrom::CopyOf {
                    story: story.clone(),
                },
            },
        ],
    );
    assert_eq!(applied.stories.len(), 1);
    let state = s.state().unwrap().data;
    let find = |section: usize, slot: &str| {
        state.sections[section]
            .bindings
            .iter()
            .find(|b| b.slot == slot && b.variant == "default")
            .unwrap()
            .clone()
    };
    assert_eq!(find(1, "header").authored, "none");
    assert_eq!(
        find(1, "footer").story.as_deref(),
        Some(applied.stories[0].as_str())
    );
    assert_eq!(state.stories.len(), 2);
    tx(
        &mut s,
        vec![Command::LinkRegion {
            section: second,
            slot: "header".into(),
            variant: "default".into(),
        }],
    );
    let state = s.state().unwrap().data;
    let header = state.sections[1]
        .bindings
        .iter()
        .find(|b| b.slot == "header" && b.variant == "default")
        .unwrap();
    assert_eq!(header.authored, "inherit");
    assert_eq!(header.resolved.as_deref(), Some(story.as_str()));
    // Deleting a story leaves its bindings, showing nothing.
    tx(&mut s, vec![Command::DeleteStory { story }]);
    assert_eq!(s.state().unwrap().data.stories.len(), 1);
}

#[test]
fn page_queries_say_what_the_layout_made() {
    let (mut s, _, story) = built();
    let progress = layout(&mut s);
    let pages = progress.pages.len() as u32;
    assert!(pages >= 2, "{pages} pages");
    // Labels: the first section counts in lower roman, the second in upper.
    let label = |p: u32| s.page_label(p).unwrap().data;
    assert_eq!(label(0).label, "i");
    assert_eq!(label(0).counter, "1");
    assert_eq!(label(1).label, "ii");
    assert_eq!(label(pages - 1).kind, SectionPageKind::Content);
    // Who owns and occupies what.
    let first = s.section_pages(&Payload::new("first".into())).unwrap().data;
    assert!(first.owned.contains(&0) && first.occupied.contains(&0));
    let second_id = s.state().unwrap().data.sections[1].id.clone();
    let second = s
        .section_pages(&Payload::new(second_id.clone()))
        .unwrap()
        .data;
    assert!(!second.occupied.is_empty());
    let on = s.page_sections(second.occupied[0]).unwrap().data;
    assert!(on.bands.iter().any(|b| b.section == second_id));
    assert_eq!(s.page_sections(0).unwrap().data.owner, "first");
    // The header is a region on every page, shown for the story.
    for p in 0..pages {
        let regions = s.page_regions(p).unwrap().data;
        assert_eq!(regions.len(), 1, "page {p}");
        assert_eq!(
            (regions[0].slot.as_str(), regions[0].story.as_str()),
            ("header", story.as_str())
        );
        assert!(regions[0].rect.width > 0 && regions[0].rect.height > 0);
    }
    let instances = s
        .region_instances(&Payload::new(story.clone()))
        .unwrap()
        .data;
    assert_eq!(instances.len() as u32, pages);
    assert!(instances.iter().all(|i| i.slot == "header"));
    // No fields yet; insert a page number into the header and ask again.
    assert!(s.page_fields(0).unwrap().data.is_empty());
    let head = s.state().unwrap().data.stories[0].blocks[0].id.clone();
    tx(
        &mut s,
        vec![Command::InsertInline {
            node: head.clone(),
            at: 12,
            object: InlineObject {
                kind: "field".into(),
                params: [("field".to_owned(), "page-number".to_owned())].into(),
                align: InlineAlign::Baseline,
            },
        }],
    );
    // The layout is stale until it is made again.
    assert!(matches!(
        s.page_label(0),
        Err(Error::Stale | Error::NoLayout)
    ));
    layout(&mut s);
    let fields = s.page_fields(1).unwrap().data;
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].node, head);
    assert_eq!(fields[0].text, "ii");
    assert!(matches!(
        fields[0].instance,
        FieldPlace::Running { page: 1, ref slot, .. } if slot == "header"
    ));
}

#[test]
fn navigation_in_a_running_region() {
    let (mut s, blocks, story) = built();
    layout(&mut s);
    let head = s.state().unwrap().data.stories[0].blocks[0].id.clone();
    let region = s.page_regions(0).unwrap().data.remove(0);
    let inside = (
        region.rect.x + region.rect.width / 2,
        region.rect.y + region.rect.height / 2,
    );
    let hit = s
        .hit_placed(&Payload::new(HitPlaced {
            page: 0,
            x: inside.0,
            y: inside.1,
            mode: None,
        }))
        .unwrap()
        .data;
    let PlacedHit::Running { position, instance } = hit else {
        panic!("a click in the header is in the header: {hit:?}");
    };
    assert_eq!(position.node, head);
    assert_eq!(
        instance,
        RegionInstance {
            page: 0,
            slot: "header".into()
        }
    );
    // A click in the body is the body's.
    let body = s.caret_rect(&Payload::new(Caret {
        node: blocks[0].clone(),
        offset: 0,
        affinity: Affinity::Downstream,
    }));
    let body = body.unwrap().data;
    let hit = s
        .hit_placed(&Payload::new(HitPlaced {
            page: 0,
            x: body.rect.x + 1024,
            y: body.rect.y + body.rect.height / 2,
            mode: Some(PlacedMode::Background),
        }))
        .unwrap()
        .data;
    assert!(matches!(hit, PlacedHit::Body { .. }), "{hit:?}");
    // Where a position in the header is drawn, and a selection of its text.
    let rect = s
        .caret_in(&Payload::new(CaretIn {
            instance: instance.clone(),
            position: position.clone(),
        }))
        .unwrap()
        .data;
    assert_eq!(rect.page, 0);
    assert!(rect.rect.y >= region.rect.y && rect.rect.y <= region.rect.y + region.rect.height);
    let rects = s
        .selection_rects_in(&Payload::new(SelectionIn {
            instance: instance.clone(),
            selection: Selection {
                anchor: Caret {
                    node: head.clone(),
                    offset: 0,
                    affinity: Affinity::Downstream,
                },
                focus: Caret {
                    node: head.clone(),
                    offset: 7,
                    affinity: Affinity::Downstream,
                },
            },
        }))
        .unwrap()
        .data;
    assert!(!rects.is_empty() && rects.iter().all(|r| r.page == 0));
    // The same story on another page answers for that page when asked about a
    // page the story is not on any more (nearest page that shows it).
    let far = RegionInstance {
        page: 1,
        slot: "header".into(),
    };
    assert!(
        s.caret_in(&Payload::new(CaretIn {
            instance: far,
            position,
        }))
        .is_ok()
    );
    let _ = story;
}

#[test]
fn bad_input_is_refused_with_a_code_and_changes_nothing() {
    let (mut s, blocks, _) = built();
    layout(&mut s);
    let before = s.state().unwrap().data;
    // Queries.
    assert_eq!(s.page_label(999).unwrap_err().code(), "bindings.invalid");
    assert_eq!(s.page_regions(999).unwrap_err().code(), "bindings.invalid");
    assert_eq!(
        s.section_pages(&Payload::new("not an id".into()))
            .unwrap_err()
            .code(),
        "bindings.id"
    );
    assert_eq!(
        s.section_pages(&Payload::new("999@9".into()))
            .unwrap_err()
            .code(),
        "bindings.id"
    );
    assert_eq!(
        s.caret_in(&Payload::new(CaretIn {
            instance: RegionInstance {
                page: 0,
                slot: "margin-note".into()
            },
            position: Caret {
                node: blocks[0].clone(),
                offset: 0,
                affinity: Affinity::Downstream
            },
        }))
        .unwrap_err()
        .code(),
        "bindings.invalid"
    );
    assert!(
        s.caret_in(&Payload::new(CaretIn {
            instance: RegionInstance {
                page: 0,
                slot: "header".into()
            },
            position: Caret {
                node: blocks[0].clone(),
                offset: 9999,
                affinity: Affinity::Downstream
            },
        }))
        .is_err()
    );
    // Commands: a property that is not a scalar, a section that isn't one, a
    // story that isn't a story, too many blocks.
    for commands in [
        vec![Command::SetSection {
            section: "first".into(),
            patch: json!({"columns.count": [1, 2]}).as_object().cloned(),
            clear: None,
        }],
        vec![Command::SetSection {
            section: "not a section".into(),
            patch: None,
            clear: None,
        }],
        vec![Command::DeleteStory {
            story: blocks[0].clone(),
        }],
        vec![Command::InsertBreak {
            at: BreakPosition::Before {
                node: blocks[0].clone(),
            },
            break_kind: BreakKind::Column,
            patch: json!({"a": 1.5}).as_object().cloned(),
        }],
        vec![Command::CreateStory {
            blocks: (0..4097).map(|_| story_block("x")).collect(),
        }],
    ] {
        assert!(
            s.apply(&Payload::new(Transaction { commands })).is_err(),
            "refused"
        );
    }
    assert_eq!(
        s.state().unwrap().data,
        before,
        "a refused command changed nothing"
    );
    // The layout is still current: nothing was written.
    assert!(s.page_label(0).is_ok());
}

#[test]
fn the_wire_forms_are_stable_and_strict() {
    let command: Command = serde_json::from_value(json!({
        "kind": "insert-break",
        "at": {"position": "before", "node": "1@1"},
        "break_kind": "page"
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_value(&command).unwrap(),
        json!({
            "kind": "insert-break",
            "at": {"position": "before", "node": "1@1"},
            "break_kind": "page",
            "patch": null
        })
    );
    for (wire, ok) in [
        (
            json!({"kind": "set-region", "section": "first", "slot": "header", "variant": "default", "binding": {"state": "inherit"}}),
            true,
        ),
        (
            json!({"kind": "set-region", "section": "first", "slot": "header", "variant": "default", "binding": {"state": "story", "story": "2@1"}}),
            true,
        ),
        (
            json!({"kind": "create-region", "section": "first", "slot": "footer", "variant": "first", "from": {"source": "copy-of", "story": "2@1"}}),
            true,
        ),
        (
            json!({"kind": "link-region", "section": "first", "slot": "header", "variant": "even"}),
            true,
        ),
        (json!({"kind": "delete-story", "story": "2@1"}), true),
        (
            json!({"kind": "delete-story", "story": "2@1", "surprise": 1}),
            false,
        ),
        (
            json!({"kind": "set-region", "section": "first", "slot": "header", "variant": "default", "binding": {"state": "weird"}}),
            false,
        ),
        (
            json!({"kind": "insert-break", "at": {"position": "nowhere"}, "break_kind": "page"}),
            false,
        ),
    ] {
        assert_eq!(
            serde_json::from_value::<Command>(wire.clone()).is_ok(),
            ok,
            "{wire}"
        );
    }
}

#[test]
fn caret_survives_removal_of_its_last_page() {
    let (mut s, _, story) = built();
    layout(&mut s);
    let old = s
        .region_instances(&Payload::new(story.clone()))
        .unwrap()
        .data
        .last()
        .unwrap()
        .clone();
    assert!(old.page > 0);
    let state = s.state().unwrap().data;
    let head = state.stories[0].blocks[0].id.clone();
    let commands = state
        .blocks
        .iter()
        .filter(|b| b.starts.is_some())
        .map(|b| Command::RemoveBreak {
            block: b.id.clone(),
        })
        .collect();
    tx(&mut s, commands);
    layout(&mut s);
    let remaining = s.region_instances(&Payload::new(story)).unwrap().data;
    assert!(remaining.iter().all(|i| i.page < old.page));
    let result = s.caret_in(&Payload::new(CaretIn {
        instance: old.clone(),
        position: Caret {
            node: head,
            offset: 0,
            affinity: Affinity::Downstream,
        },
    }));
    eprintln!(
        "old page={}, remaining={remaining:?}, caret={result:?}",
        old.page
    );
    assert!(
        result.is_ok(),
        "facade refuses old page before nearest surviving placement fallback"
    );
}
#[test]
fn failed_section_batch_and_multibyte_break_preserve_state() {
    let mut s = session();
    let id = tx(&mut s, vec![paragraph("😀éx", 0)]).blocks.remove(0);
    let before = s.state().unwrap().data;
    let result = s.apply(&Payload::new(Transaction {
        commands: vec![
            Command::CreateStory {
                blocks: vec![story_block("staged")],
            },
            Command::InsertBreak {
                at: BreakPosition::Position {
                    node: id.clone(),
                    offset: 1,
                },
                break_kind: BreakKind::Section,
                patch: None,
            },
        ],
    }));
    assert!(result.is_err());
    assert_eq!(before, s.state().unwrap().data);
    tx(
        &mut s,
        vec![Command::InsertBreak {
            at: BreakPosition::Position {
                node: id,
                offset: 4,
            },
            break_kind: BreakKind::Section,
            patch: None,
        }],
    );
    let state = s.state().unwrap().data;
    assert_eq!(
        state
            .blocks
            .iter()
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>(),
        ["😀", "éx"]
    );
}
