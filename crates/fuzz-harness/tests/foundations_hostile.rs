//! A hostile peer writes raw operations into the wave-1 stored forms and
//! reaches the victim only through a sync packet (`docs/fuzzing.md`).
//!
//! `crates/reprise/tests/hostile_peer.rs` does this for the roots that existed
//! before wave 1. This one attacks `inline1`, `annot1` and the restore hints,
//! the `lists` tree and the paragraph keys `role1`, `list1`, `level1` and
//! `restart1`, `styles1`, `para1` and `styles2` with their control-prefixed
//! references, and the identity roots.
//!
//! Whatever the packet does, the victim must stay a working document:
//!
//! -   a refused packet changes nothing (every operation byte-identical);
//! -   an accepted one lays out, renders in every backend, exports, saves and
//!     reopens to the same state and the same pages, and `State` stays
//!     coherent;
//! -   it reports only documented diagnostic codes, and an edit still either
//!     works or is refused with a documented one;
//! -   package bits 14, 15, 16 and 18 are declared for whatever is live;
//! -   a second receiver of the same packet reaches the same state.

use std::collections::BTreeSet;

use reprise::*;
use reprise_doc::hostile::{
    Rng, dump_inline_records, scribble, scribble_foundations, write_deep_chain, write_node_entry,
    write_root_string, write_shadow_root,
};
use reprise_fuzz_harness::codes;
use reprise_fuzz_harness::foundations::oracles;

const DOC: &str = "00112233445566778899aabbccddeeff";
const SERIF: &[u8] = include_bytes!("../../../fixtures/fonts/SourceSerifPro-Regular.otf");
const RED_PNG: &[u8] = include_bytes!("../../../fixtures/images/red-1x1.png");

fn env_number(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn workspace() -> Workspace {
    let ws = Workspace::new();
    ws.declare_font(
        &Payload::new(FontDeclaration {
            family: "Source Serif Pro".into(),
            weight: 400,
            style: FontStyle::Normal,
            stretch: 1000,
            face_index: 0,
        }),
        SERIF,
    )
    .unwrap();
    ws.set_font_settings(&Payload::new(FontSettings {
        generics: [(GenericFamily::Serif, "Source Serif Pro".to_owned())].into(),
        ..FontSettings::default()
    }))
    .unwrap();
    ws
}

fn tx(s: &mut DocumentSession, commands: Vec<Command>) -> Applied {
    s.apply(&Payload::new(Transaction { commands }))
        .unwrap_or_else(|e| panic!("victim setup: {e} ({})", e.code()))
        .data
}

/// A document with every wave-1 feature in it: a heading, a two-level list
/// with a field and an image in its items, links and a comment, custom styles
/// (one renamed), a `page-of` cross reference.
fn victim(ws: &Workspace) -> DocumentSession {
    let mut s = ws
        .create(&Payload::new(Create {
            document_id: DOC.into(),
            peer_id: "1".into(),
        }))
        .unwrap();
    let hash = s
        .register_asset(
            &Payload::new(AssetDeclaration {
                id: "red".into(),
                kind: AssetKind::Image,
            }),
            RED_PNG,
        )
        .unwrap()
        .data;
    let mut ids = Vec::new();
    for (n, text) in [
        "A title for the hostile document",
        "first item with a field ",
        "second item with an image ",
        "a plain paragraph with a link and a comment on it",
        "the last paragraph, with a cross reference ",
    ]
    .into_iter()
    .enumerate()
    {
        let applied = tx(
            &mut s,
            vec![Command::InsertBlock {
                parent: None,
                index: n as u32,
                block_kind: BlockKind::Paragraph,
                text: text.into(),
                style: Style::default(),
            }],
        );
        ids.push(applied.blocks[0].clone());
    }
    tx(
        &mut s,
        vec![
            Command::SetRole {
                node: ids[0].clone(),
                role: BlockRole::Heading1,
            },
            Command::ApplyStyle {
                node: ids[0].clone(),
                style: Some("Heading 1".into()),
            },
        ],
    );
    tx(
        &mut s,
        vec![
            Command::CreateList {
                definition: ListDefinition {
                    base: None,
                    levels: vec![
                        ListLevel {
                            level: 0,
                            format: Some("decimal".into()),
                            text: Some("%1.".into()),
                            ..ListLevel::default()
                        },
                        ListLevel {
                            level: 1,
                            format: Some("lower-alpha".into()),
                            text: Some("%2)".into()),
                            ..ListLevel::default()
                        },
                    ],
                },
            },
            Command::SetListItem {
                node: ids[1].clone(),
                list: ListRef::New { index: 0 },
                level: 0,
            },
            Command::SetListItem {
                node: ids[2].clone(),
                list: ListRef::New { index: 0 },
                level: 1,
            },
        ],
    );
    let field = |name: &str| InlineObject {
        kind: "field".into(),
        align: InlineAlign::Baseline,
        params: [("field".to_owned(), name.to_owned())].into(),
    };
    tx(
        &mut s,
        vec![
            Command::InsertInline {
                node: ids[1].clone(),
                at: 0,
                object: field("page-number"),
            },
            Command::InsertInline {
                node: ids[2].clone(),
                at: 3,
                object: InlineObject {
                    kind: "image".into(),
                    align: InlineAlign::Middle,
                    params: [
                        ("asset".to_owned(), hash),
                        ("width".to_owned(), "pt:36864".to_owned()),
                        ("alt".to_owned(), "a red dot".to_owned()),
                    ]
                    .into(),
                },
            },
        ],
    );
    let target = s
        .field_target(&Payload::new(FieldTarget {
            node: ids[0].clone(),
            at: 0,
        }))
        .unwrap()
        .data;
    tx(
        &mut s,
        vec![
            Command::InsertInline {
                node: ids[4].clone(),
                at: 0,
                object: InlineObject {
                    kind: "field".into(),
                    align: InlineAlign::Baseline,
                    params: [
                        ("field".to_owned(), "page-of".to_owned()),
                        ("target".to_owned(), target),
                    ]
                    .into(),
                },
            },
            Command::AddAnnotation {
                node: ids[3].clone(),
                start: 0,
                end: 10,
                annotation: AnnotationPayload::Link {
                    target: LinkDestination::Url {
                        url: "https://example.com/doc".into(),
                    },
                },
            },
            Command::AddAnnotation {
                node: ids[3].clone(),
                start: 5,
                end: 25,
                annotation: AnnotationPayload::Comment {
                    thread: "thread-1".into(),
                },
            },
            Command::AddAnnotation {
                node: ids[4].clone(),
                start: 4,
                end: 20,
                annotation: AnnotationPayload::Link {
                    target: LinkDestination::Heading {
                        node: ids[0].clone(),
                    },
                },
            },
        ],
    );
    tx(
        &mut s,
        vec![
            Command::DefineStyle {
                name: "S0".into(),
                parent: Some("Normal".into()),
                style: Style {
                    size: Some("14pt".into()),
                    indent_start: Some("12pt".into()),
                    ..Style::default()
                },
            },
            Command::DefineStyle {
                name: "S1".into(),
                parent: Some("S0".into()),
                style: Style {
                    alignment: Some(Alignment::Centre),
                    ..Style::default()
                },
            },
            Command::ApplyStyle {
                node: ids[3].clone(),
                style: Some("S1".into()),
            },
            Command::RenameStyle {
                from: "S1".into(),
                to: "S2".into(),
            },
        ],
    );
    // The older stored forms too, so that an attack on their roots has
    // something to hide: a direct alignment (`marks1`), a flow break
    // (`breaks1`) and a page setup (`page-setup1`).
    tx(
        &mut s,
        vec![
            Command::SetAlignment {
                node: ids[0].clone(),
                at: None,
                alignment: Some(Alignment::End),
            },
            // A patch of a direct paragraph property (`para1`): `clear` selects patching.
            Command::SetStyle {
                node: ids[4].clone(),
                style: Style {
                    indent_start: Some("6pt".into()),
                    space_after: Some("3pt".into()),
                    ..Style::default()
                },
                clear: Some(vec![]),
            },
            Command::SplitBlock {
                node: ids[4].clone(),
                at: 30,
            },
            Command::SetPageSetup {
                setup: PageSetupPatch {
                    width: Some(612 * 1024),
                    ..PageSetupPatch::default()
                },
            },
        ],
    );
    s
}

fn vector(clocks: &[Clock]) -> Vec<(u64, i32)> {
    clocks
        .iter()
        .map(|c| (c.peer.parse().unwrap(), c.counter))
        .collect()
}

fn packet(bytes: Vec<u8>) -> Payload<SyncPacket> {
    let (header, _) = reprise_doc::sync::read_header(&bytes).unwrap();
    let clocks = |v: &[(u64, i32)]| {
        v.iter()
            .map(|(p, c)| Clock {
                peer: p.to_string(),
                counter: *c,
            })
            .collect()
    };
    Payload::new(SyncPacket {
        document_id: DOC.into(),
        from_peer: "66".into(),
        format: u32::from(header.format),
        features: header.features.names(),
        kind: SyncKind::Delta,
        since: clocks(&header.since),
        vector: clocks(&header.until),
        content: Bytes { bytes },
    })
}

/// Every operation the document holds.
fn operations(s: &DocumentSession) -> Vec<u8> {
    s.sync_export(&Payload::new(SyncRequest {
        since: Some(Vec::new()),
    }))
    .unwrap()
    .data
    .content
    .bytes
}

/// The raw attack as a delta packet from a peer forked from `s`.
fn attack(
    s: &DocumentSession,
    write: impl FnOnce(&reprise_doc::Document),
) -> Option<Payload<SyncPacket>> {
    let snapshot = s
        .sync_export(&Payload::new(SyncRequest { since: None }))
        .unwrap();
    let attacker = reprise_doc::Document::new(66).unwrap();
    attacker.bind_document_lineage(DOC).unwrap();
    attacker.commit();
    attacker
        .import_packet(&snapshot.data.content.bytes)
        .unwrap();
    write(&attacker);
    // An attacker whose own store is unreadable can't frame a packet at all.
    let bytes = attacker
        .export_delta(&vector(&s.sync_info().data.vector))
        .ok()?;
    Some(packet(bytes))
}

fn documented(code: &str, case: &str) {
    assert!(
        codes::is_documented(code),
        "{case}: the code `{code}` isn't in docs/contracts.md"
    );
}

/// Ok, or a refusal with a documented code.
fn tolerated<T>(result: Result<T>, case: &str, what: &str, refused: &mut Vec<String>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            assert_ne!(
                error.code(),
                "bindings.poisoned",
                "{case}: {what} panicked inside the session: {error}"
            );
            documented(error.code(), case);
            refused.push(format!("{what}: {}", error.code()));
            None
        }
    }
}

/// The nodes of `s` that are containers, not flow hosts: `docs/inline.md`'s
/// rule about atoms doesn't cover their stored text (see
/// `reprise_doc::hostile::non_hosts`).
fn non_hosts(s: &DocumentSession) -> BTreeSet<String> {
    let Ok(snapshot) = s.sync_export(&Payload::new(SyncRequest { since: None })) else {
        return BTreeSet::new();
    };
    let reader = reprise_doc::Document::new(77).unwrap();
    reader.bind_document_lineage(DOC).unwrap();
    reader.commit();
    match reader.import_packet(&snapshot.data.content.bytes) {
        Ok(_) => reprise_doc::hostile::non_hosts(&reader)
            .into_iter()
            .collect(),
        Err(_) => BTreeSet::new(),
    }
}

struct Seen {
    state: Option<State>,
    pages: Vec<String>,
    refused: Vec<String>,
}

fn layout(s: &mut DocumentSession) -> Option<LayoutProgress> {
    let mut job = s
        .start_layout(&Payload::new(LayoutOptions {
            max_pages: 40,
            ..LayoutOptions::default()
        }))
        .ok()?;
    for _ in 0..100_000 {
        let progress = job.step(s, 64).ok()?.data;
        if progress.complete {
            return Some(progress);
        }
    }
    panic!("a layout job didn't finish in 100,000 steps")
}

/// Lays out, renders, exports and reads everything a host would.
fn observe(s: &mut DocumentSession, case: &str) -> Seen {
    let mut refused = Vec::new();
    let state = tolerated(s.state(), case, "state", &mut refused).map(|p| p.data);
    if let Some(state) = &state {
        oracles::state_is_coherent_except(state, &non_hosts(s))
            .unwrap_or_else(|v| panic!("{case}: {v}"));
        oracles::documented_diagnostics(state).unwrap_or_else(|v| panic!("{case}: {v}"));
    }
    let mut pages = Vec::new();
    if let Some(progress) = layout(s) {
        for &page in &progress.pages {
            if let Some(json) = tolerated(s.display_json(page), case, "display_json", &mut refused)
            {
                pages.push(json.data);
            }
            tolerated(s.svg(page), case, "svg", &mut refused);
            tolerated(s.png(page, 100), case, "png", &mut refused);
            tolerated(s.page_links(page), case, "page_links", &mut refused);
        }
        tolerated(s.reading_order(), case, "reading_order", &mut refused);
        tolerated(s.outline(), case, "outline", &mut refused);
    }
    for format in [
        ExportFormat::Pdf,
        ExportFormat::Html,
        ExportFormat::PlainText,
        ExportFormat::Native,
    ] {
        tolerated(
            s.export(&Payload::new(format)),
            case,
            "export",
            &mut refused,
        );
    }
    for d in s.diagnostics().data {
        documented(&d.code, case);
    }
    if let Some(state) = &state {
        for a in state.annotations.iter().take(6) {
            tolerated(
                s.annotation(&Payload::new(a.id.clone())),
                case,
                "annotation",
                &mut refused,
            );
            tolerated(
                s.annotation_geometry(&Payload::new(a.id.clone())),
                case,
                "annotation_geometry",
                &mut refused,
            );
        }
    }
    Seen {
        state,
        pages,
        refused,
    }
}

/// An edit still works, or is refused with a documented code; undo and redo
/// too.
fn still_editable(s: &mut DocumentSession, case: &str) {
    let mut refused = Vec::new();
    let Some(state) = tolerated(s.state(), case, "state", &mut refused) else {
        return;
    };
    let Some(first) = state.data.blocks.first() else {
        return;
    };
    let result = s.apply(&Payload::new(Transaction {
        commands: vec![
            Command::InsertText {
                node: first.id.clone(),
                at: 0,
                text: "edited ".into(),
            },
            Command::SetRole {
                node: first.id.clone(),
                role: BlockRole::Heading2,
            },
        ],
    }));
    let applied = tolerated(result, case, "an edit after the attack", &mut refused).is_some();
    if applied {
        tolerated(s.undo(), case, "undo", &mut refused);
        tolerated(s.redo(), case, "redo", &mut refused);
        tolerated(s.undo(), case, "undo", &mut refused);
    }
}

/// Imports `packet` into a fresh victim and runs every check.
fn run_case(ws: &Workspace, case: &str, packet: &Payload<SyncPacket>) -> Outcome {
    let mut s = victim(ws);
    let before = operations(&s);
    // A second receiver holds the document from a snapshot, not live.
    let snapshot_before = s.save().ok().map(|p| p.data.bytes);
    match s.sync_import(packet) {
        Err(error) => {
            assert_ne!(
                error.code(),
                "bindings.poisoned",
                "{case}: the import panicked inside the session: {error}"
            );
            documented(error.code(), case);
            assert_eq!(
                operations(&s),
                before,
                "{case}: a refused packet (`{}`) changed the document",
                error.code()
            );
            Outcome::Refused(error.code().to_owned())
        }
        Ok(_) => {
            let seen = observe(&mut s, case);
            still_editable(&mut s, case);
            // Save and reopen: the same state and the same pages.
            let saved = tolerated(s.save(), case, "save", &mut Vec::new());
            if let Some(saved) = saved {
                let bytes = saved.data.bytes;
                if let Some(state) = &seen.state {
                    // Soundness only: a hostile peer may have declared anything.
                    oracles::bits_match(
                        &bytes,
                        state,
                        oracles::Flags {
                            semantics: true,
                            inline: true,
                            annotations: true,
                            styles: true,
                        },
                    )
                    .unwrap_or_else(|v| panic!("{case}: {v}"));
                }
                let reopened = ws.open(
                    &Payload::new(Open {
                        peer_id: "3".into(),
                    }),
                    &bytes,
                );
                if let Some(mut reopened) = tolerated(reopened, case, "reopen", &mut Vec::new()) {
                    let again = observe(&mut reopened, case);
                    // A peer that deletes the base style leaves a live session
                    // without one; opening a package authors it again.
                    let base_missing = seen
                        .state
                        .as_ref()
                        .is_some_and(|s| !s.styles.iter().any(|n| n.name.is_empty()));
                    if let (Some(a), Some(b), false) = (&seen.state, &again.state, base_missing) {
                        let (a, b) = (oracles::authored(a), oracles::authored(b));
                        assert!(
                            a == b,
                            "{case}: reopening changed the state:\n{}",
                            reprise_fuzz_harness::foundations::describe_difference(&a, &b)
                        );
                    }
                    if seen.refused.is_empty() && again.refused.is_empty() && !base_missing {
                        assert_eq!(
                            seen.pages, again.pages,
                            "{case}: reopening redrew the pages"
                        );
                    }
                }
            }
            // The same packet into a replica that opened a snapshot of the
            // victim: same operations, so the same document. (A root of
            // another type under a document root's name once made the two read
            // different records.)
            if let Some(bytes) = &snapshot_before {
                let opened = ws.open(
                    &Payload::new(Open {
                        peer_id: "2".into(),
                    }),
                    bytes,
                );
                if let Some(mut second) =
                    tolerated(opened, case, "open the snapshot", &mut Vec::new())
                {
                    match second.sync_import(packet) {
                        Ok(_) => {
                            let other = observe(&mut second, case);
                            let base_missing = seen
                                .state
                                .as_ref()
                                .is_some_and(|s| !s.styles.iter().any(|n| n.name.is_empty()));
                            if let (Some(a), Some(b)) = (&seen.state, &other.state) {
                                let (a, b) = (oracles::authored(a), oracles::authored(b));
                                assert!(
                                    a == b || base_missing,
                                    "{case}: a replica that opened a snapshot reads the packet differently:\n{}",
                                    reprise_fuzz_harness::foundations::describe_difference(&a, &b)
                                );
                            }
                            if seen.refused.is_empty() && other.refused.is_empty() && !base_missing
                            {
                                assert_eq!(
                                    seen.pages, other.pages,
                                    "{case}: a replica that opened a snapshot draws the packet differently"
                                );
                            }
                        }
                        Err(error) => panic!(
                            "{case}: a replica that opened a snapshot refused ({}) a packet the live one accepted",
                            error.code()
                        ),
                    }
                }
            }
            Outcome::Accepted(seen.refused)
        }
    }
}

/// A raw write into the attacker's store.
type Write = Box<dyn Fn(&reprise_doc::Document)>;

enum Outcome {
    Refused(String),
    Accepted(Vec<String>),
}

#[test]
fn hostile_foundations_never_break_the_pipeline() {
    let ws = workspace();
    let seeds = env_number("REPRISE_FOUNDATIONS_HOSTILE_SEEDS", 12);
    let first = env_number("REPRISE_FOUNDATIONS_HOSTILE_FIRST", 0);
    let mut accepted = 0;
    let mut undeliverable = 0;
    let mut refused = BTreeSet::new();
    let mut reads_refused = BTreeSet::new();
    for seed in first..first + seeds {
        let case = format!("seed {seed}");
        let s = victim(&ws);
        let packet = attack(&s, |attacker| {
            let mut rng = Rng::new(seed);
            scribble_foundations(attacker, &mut rng, env_number("HOSTILE_OPS", 60) as usize);
            if std::env::var_os("DUMP_RECORDS").is_some() {
                for line in dump_inline_records(attacker) {
                    eprintln!("record {line}");
                }
            }
            if seed % 4 == 0 {
                scribble(attacker, &mut rng, 30);
            }
        });
        let Some(packet) = packet else {
            undeliverable += 1;
            continue;
        };
        match run_case(&ws, &case, &packet) {
            Outcome::Accepted(reads) => {
                accepted += 1;
                reads_refused.extend(reads);
            }
            Outcome::Refused(code) => {
                refused.insert(code);
            }
        }
    }
    println!(
        "accepted {accepted} of {seeds}; {undeliverable} undeliverable; refused with {refused:?}; reads refused {reads_refused:?}"
    );
    assert!(
        undeliverable <= seeds / 4,
        "too many attacks couldn't be framed"
    );
    assert!(
        accepted > seeds / 2,
        "most hostile packets should be accepted and survived, not refused: {refused:?}"
    );
}

/// Named attacks, one stored form each, so a failure reads without a seed.
fn run_named_attacks(shadows_only: bool) {
    let ws = workspace();
    let s = victim(&ws);
    let state = s.state().unwrap().data;
    let list = state.lists[0].id.clone();
    let heading = state.blocks[0].id.clone();
    let item = state.blocks[1].id.clone();
    let linked = state.blocks[3].id.clone();
    let annotation = state.annotations[0].id.clone();
    let object = state
        .blocks
        .iter()
        .flat_map(|b| &b.inline)
        .next()
        .unwrap()
        .id
        .clone();
    let long = "x".repeat(40_000);
    let deep = "[".repeat(5_000);
    let attacks: Vec<(&str, Write)> = vec![
        (
            "inline: version a string",
            Box::new({
                let key = object.clone();
                move |d| write_inline(d, &key, "v", "one")
            }),
        ),
        (
            "inline: version two",
            Box::new({
                let key = object.clone();
                move |d| write_inline(d, &key, "v", "i64:2")
            }),
        ),
        (
            "inline: active a string",
            Box::new({
                let key = object.clone();
                move |d| write_inline(d, &key, "active", "yes")
            }),
        ),
        (
            "inline: kind empty",
            Box::new({
                let key = object.clone();
                move |d| write_inline(d, &key, "kind", "")
            }),
        ),
        (
            "inline: huge width",
            Box::new({
                let key = object.clone();
                move |d| write_inline(d, &key, "p:width", "pt:9223372036854775807")
            }),
        ),
        (
            "inline: oversize parameter",
            Box::new({
                let (key, long) = (object.clone(), long.clone());
                move |d| write_inline(d, &key, "p:alt", &long)
            }),
        ),
        (
            "inline: a record for no character",
            Box::new({
                let host = heading.clone();
                move |d| write_inline(d, &format!("{host}/99999@1"), "kind", "field")
            }),
        ),
        (
            "annotation: oversize envelope",
            Box::new({
                let (id, long) = (annotation.clone(), long.clone());
                move |d| {
                    let _ = write_node_entry(d, "ranges", &id, "annot1", &long);
                }
            }),
        ),
        (
            "annotation: deeply nested envelope",
            Box::new({
                let (id, deep) = (annotation.clone(), deep.clone());
                move |d| {
                    let _ = write_node_entry(d, "ranges", &id, "annot1", &deep);
                }
            }),
        ),
        (
            "annotation: a javascript link",
            Box::new({
                let id = annotation.clone();
                move |d| {
                    let _ = write_node_entry(
                        d,
                        "ranges",
                        &id,
                        "annot1",
                        r#"{"version":1,"kind":"link","payload":{"type":"url","url":"javascript:alert(1)"}}"#,
                    );
                }
            }),
        ),
        (
            "annotation: a link to the annotation itself",
            Box::new({
                let id = annotation.clone();
                move |d| {
                    let raw = format!(
                        r#"{{"version":1,"kind":"link","payload":{{"type":"range","range":"{id}"}}}}"#
                    );
                    let _ = write_node_entry(d, "ranges", &id, "annot1", &raw);
                }
            }),
        ),
        (
            "annotation: restore hints that lie",
            Box::new({
                let id = annotation.clone();
                move |d| {
                    let _ = write_node_entry(
                        d,
                        "ranges",
                        &id,
                        "annot-restore1",
                        r#"[{"version":1,"base":null,"count":4294967295,"digest":"00","peer":"1","after":0,"start":null,"end":null}]"#,
                    );
                }
            }),
        ),
        (
            "list: a base that is itself",
            Box::new({
                let list = list.clone();
                move |d| {
                    let _ = write_node_entry(d, "lists", &list, "base", &list);
                }
            }),
        ),
        (
            "list: a base that is a paragraph",
            Box::new({
                let (list, item) = (list.clone(), item.clone());
                move |d| {
                    let _ = write_node_entry(d, "lists", &list, "base", &item);
                }
            }),
        ),
        (
            "list: version two",
            Box::new({
                let list = list.clone();
                move |d| {
                    let _ = write_node_entry(d, "lists", &list, "v", "i64:2");
                }
            }),
        ),
        (
            "list: an overflowing start",
            Box::new({
                let list = list.clone();
                move |d| {
                    let _ = write_node_entry(d, "lists", &list, "0.start", "9223372036854775808");
                }
            }),
        ),
        (
            "list: an unknown format",
            Box::new({
                let list = list.clone();
                move |d| {
                    let _ = write_node_entry(d, "lists", &list, "1.format", "BOGUS");
                }
            }),
        ),
        (
            "list: a template of every counter",
            Box::new({
                let list = list.clone();
                move |d| {
                    let _ = write_node_entry(
                        d,
                        "lists",
                        &list,
                        "0.text",
                        "%1.%2.%3.%4.%5.%6.%7.%8.%9.%0.%%.%",
                    );
                }
            }),
        ),
        (
            "list: a level past the deepest",
            Box::new({
                let list = list.clone();
                move |d| {
                    let _ = write_node_entry(d, "lists", &list, "11.format", "decimal");
                }
            }),
        ),
        (
            "paragraph: level 255",
            Box::new({
                let item = item.clone();
                move |d| {
                    let _ = write_node_entry(d, "content", &item, "level1", "i64:255");
                }
            }),
        ),
        (
            "paragraph: negative level",
            Box::new({
                let item = item.clone();
                move |d| {
                    let _ = write_node_entry(d, "content", &item, "level1", "i64:-1");
                }
            }),
        ),
        (
            "paragraph: a list that is a paragraph",
            Box::new({
                let (item, linked) = (item.clone(), linked.clone());
                move |d| {
                    let _ = write_node_entry(d, "content", &item, "list1", &linked);
                }
            }),
        ),
        (
            "paragraph: a list that is gone",
            Box::new({
                let item = item.clone();
                move |d| {
                    let _ = write_node_entry(d, "content", &item, "list1", "999@999");
                }
            }),
        ),
        (
            "paragraph: restart beyond i64",
            Box::new({
                let item = item.clone();
                move |d| {
                    let _ =
                        write_node_entry(d, "content", &item, "restart1", "9223372036854775808");
                }
            }),
        ),
        (
            "paragraph: heading seven",
            Box::new({
                let heading = heading.clone();
                move |d| {
                    let _ = write_node_entry(d, "content", &heading, "role1", "heading-7");
                }
            }),
        ),
        (
            "paragraph: a role from the future",
            Box::new({
                let heading = heading.clone();
                move |d| {
                    let _ = write_node_entry(d, "content", &heading, "role1", "pull-quote");
                }
            }),
        ),
        (
            "style: a rename onto itself",
            Box::new(|d| {
                let _ = write_root_string(d, "styles1", "state/S0", "renamed:S0");
            }),
        ),
        (
            "style: a rename cycle",
            Box::new(|d| {
                let _ = write_root_string(d, "styles1", "state/S0", "renamed:S2");
                let _ = write_root_string(d, "styles1", "state/S2", "renamed:S0");
            }),
        ),
        (
            "style: garbage state",
            Box::new(|d| {
                let _ = write_root_string(d, "styles1", "state/S0", "garbage");
            }),
        ),
        (
            "style: an absurd length",
            Box::new(|d| {
                let _ = write_root_string(d, "styles1", "indent-start/S0", "-9999999999pt");
                let _ = write_root_string(d, "styles1", "size/S0", "calc(1/0)");
            }),
        ),
        (
            "style: a block patch for no block",
            Box::new(|d| {
                let _ = write_root_string(d, "para1", "999@999/size", "12pt");
            }),
        ),
        (
            "style: an owner that doesn't exist",
            Box::new({
                let linked = linked.clone();
                move |d| {
                    let _ = write_node_entry(d, "content", &linked, "style", "\u{1}style1:ffff");
                }
            }),
        ),
        (
            "style: a built-in reference to nothing",
            Box::new({
                let linked = linked.clone();
                move |d| {
                    let _ =
                        write_node_entry(d, "content", &linked, "style", "\u{2}builtin1:Nonesuch");
                }
            }),
        ),
        (
            "identity: a clock that isn't a number",
            Box::new(|d| {
                let _ = write_root_string(d, "styles2", "clock/1", "later");
            }),
        ),
        (
            "identity: a label for no owner",
            Box::new(|d| {
                let _ = write_root_string(d, "styles2", "label/\u{1}style1:ffff", "Ghost");
            }),
        ),
        (
            "identity: another document",
            Box::new(|d| {
                let _ = write_root_string(d, "document1", "id", "ffffffffffffffffffffffffffffffff");
            }),
        ),
        (
            "deep: 200 nested paragraphs",
            Box::new(|d| {
                let _ = write_deep_chain(d, 200);
            }),
        ),
    ];
    // A root is identified by name and type, so a peer can add one of another
    // type under a document root's name. Replicas must still read the same
    // records, whichever of the two their store registered last.
    let held = {
        let snapshot = s
            .sync_export(&Payload::new(SyncRequest { since: None }))
            .unwrap();
        let reader = reprise_doc::Document::new(77).unwrap();
        reader.bind_document_lineage(DOC).unwrap();
        reader.commit();
        reader.import_packet(&snapshot.data.content.bytes).unwrap();
        reprise_doc::hostile::held_roots(&reader)
    };
    println!("the victim holds {held:?}");
    let mut shadows: Vec<(String, String, &'static str)> = Vec::new();
    for entry in &held {
        // `name:Type`, where the name may itself hold a colon.
        let (root, kind) = entry.rsplit_once(':').unwrap();
        for shadow in if kind == "Tree" {
            vec!["text", "map"]
        } else {
            vec!["text"]
        } {
            shadows.push((
                format!("shadow: a {shadow} root named {root}"),
                root.to_owned(),
                shadow,
            ));
        }
    }
    for must in [
        "inline1",
        "ranges",
        "lists",
        "breaks1",
        "marks1",
        "page-setup1",
        "para1",
    ] {
        assert!(
            held.iter().any(|h| h.starts_with(&format!("{must}:"))),
            "the victim must hold {must}, or an attack on it proves nothing: {held:?}"
        );
    }
    let attacks: Vec<(String, Write)> = attacks
        .into_iter()
        .filter(|_| !shadows_only)
        .map(|(name, write)| (name.to_owned(), write))
        .chain(
            shadows
                .into_iter()
                .filter(|_| shadows_only)
                .map(|(name, root, kind)| {
                    let write: Write = Box::new(move |d| {
                        let _ = write_shadow_root(d, &root, kind);
                    });
                    (name, write)
                }),
        )
        .collect();
    let mut refused = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    let mut undeliverable: Vec<String> = Vec::new();
    for (name, write) in &attacks {
        let Some(packet) = attack(&s, |d| write(d)) else {
            undeliverable.push(name.clone());
            continue;
        };
        // Every attack runs, so one run lists everything that fails.
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_case(&ws, name, &packet)
        }));
        match run {
            Ok(Outcome::Refused(code)) => refused.push(format!("{name}: {code}")),
            Ok(Outcome::Accepted(_)) => {}
            Err(_) => failed.push(name.clone()),
        }
    }
    println!(
        "refused outright: {refused:#?}
undeliverable: {undeliverable:#?}"
    );
    assert!(
        failed.is_empty(),
        "attacks that broke the pipeline: {failed:#?}"
    );
    // Only the attacks on the identity root corrupt the attacker's own store.
    assert!(
        undeliverable
            .iter()
            .all(|name| name.starts_with("identity") || name.contains("named document1")),
        "attacks that couldn't be framed: {undeliverable:?}"
    );
}

fn write_inline(doc: &reprise_doc::Document, key: &str, entry: &str, raw: &str) {
    let _ = reprise_doc::hostile::write_inline_entry(doc, key, entry, raw);
}

/// Trees as deep as `State` allows (1,024 levels) and just past it. The engine's
/// cost here is quadratic in depth (`state()` goes 0.18 s, 0.67 s, 2.6 s at
/// depths 125, 250, 500 in a debug build), so each of these takes minutes:
/// run it with `--ignored`. Astra 4 R2 was a stack overflow at this depth in
/// `list_numbering` and `has_block_semantics`; nothing may overflow now.
#[test]
#[ignore = "slow in debug (quadratic in tree depth): about four minutes per depth"]
fn trees_at_and_past_the_depth_limit_are_survived() {
    let ws = workspace();
    let s = victim(&ws);
    for depth in [1_000usize, 1_030] {
        let case = format!("{depth} nested paragraphs");
        let packet = attack(&s, |d| {
            let _ = write_deep_chain(d, depth);
        })
        .expect("a deep chain doesn't touch the attacker's lineage");
        run_case(&ws, &case, &packet);
    }
}

/// Named attacks, one stored form each, so a failure reads without a seed.
#[test]
fn named_attacks_are_survived() {
    run_named_attacks(false);
}

/// A root of another type under the name of every root the victim holds.
/// Replicas must read the same records whichever of the two their store
/// registered last.
#[test]
fn shadow_roots_are_survived() {
    run_named_attacks(true);
}
