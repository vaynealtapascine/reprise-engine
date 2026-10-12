//! The annotation storm hostile fixture, taken through every consumer: two
//! replicas agree, bad envelopes are reported and kept, geometry and link
//! areas are bounded and deterministic, and the formats and the package
//! carry what is readable.

#[path = "../../display/tests/common/checker.rs"]
mod checker;

use reprise_clipboard::{
    ExportOptions, Exporter, Html, NativeFragment, PdfWithAssets, copy_all, import_html,
};
use reprise_doc::annotation::{AnnotationState, Kind, MAX_QUERY_RESULTS, UNREADABLE};
use reprise_doc::{Document, SchemaRegistry};
use reprise_edit::Editor;
use reprise_fixtures::hostile;

fn active_links(doc: &Document) -> usize {
    doc.annotations_of_kind(Kind::Link)
        .annotations
        .iter()
        .filter(|a| matches!(a.state, AnnotationState::Active { .. }))
        .count()
}

#[test]
fn replicas_agree_and_the_two_bad_envelopes_are_reported_and_kept() {
    let f = hostile::annotation_storm().unwrap();
    let replica = f.replica.as_ref().unwrap();
    for kind in [Kind::Link, Kind::Comment] {
        assert_eq!(
            f.doc.annotations_of_kind(kind),
            replica.annotations_of_kind(kind),
            "{kind:?}"
        );
    }
    assert!(f.doc.annotations_of_kind(Kind::Link).annotations.len() > 100);
    assert!(
        f.doc
            .annotations_of_kind(Kind::Comment)
            .annotations
            .iter()
            .any(|a| a.is_orphaned()),
        "the deletion orphaned some comments"
    );
    for doc in [&f.doc, replica] {
        let notes = doc.annotation_notes();
        let unreadable = notes.iter().find(|n| n.code == UNREADABLE).unwrap();
        assert!(
            unreadable.message.starts_with("2 "),
            "{}",
            unreadable.message
        );
        assert!(doc.has_annotations());
    }
    // The hostile `javascript:` link is neither queryable nor applied.
    assert!(
        !f.doc
            .annotations_of_kind(Kind::Link)
            .annotations
            .iter()
            .any(|a| format!("{:?}", a.data).contains("javascript"))
    );
    // Queries are bounded however much overlaps.
    let first = f.doc.blocks()[0];
    let all = f.doc.annotations_in(first, 0..usize::MAX);
    assert!(all.annotations.len() <= MAX_QUERY_RESULTS);
}

#[test]
fn geometry_and_link_areas_are_deterministic_and_inside_the_pages() {
    let f = hostile::annotation_storm().unwrap();
    let a = f.engine.layout(&f.doc);
    let b = f.engine.layout(&f.doc);
    // Links whose text is all gone have no geometry; the rest all do.
    let ids: Vec<_> = f
        .doc
        .annotation_view()
        .iter()
        .filter(|a| a.state != AnnotationState::Gone)
        .map(|a| a.id)
        .collect();
    let batched = a.annotation_geometries(&f.doc, &ids);
    assert_eq!(batched.len(), ids.len());
    assert_eq!(batched, b.annotation_geometries(&f.doc, &ids));
    let mut placed = 0usize;
    for (id, geometry) in &batched {
        assert_eq!(
            geometry,
            &a.annotation_geometry(&f.doc, *id).unwrap(),
            "{id}"
        );
        assert!(!geometry.stale);
        for r in &geometry.rects {
            let page = a.pages[r.page];
            assert!(
                r.bounds.origin.x.0 >= 0 && r.bounds.max_x().0 <= page.width.0,
                "{r:?}"
            );
            assert!(
                r.bounds.origin.y.0 >= 0 && r.bounds.max_y().0 <= page.height.0,
                "{r:?}"
            );
        }
        placed += usize::from(!geometry.rects.is_empty());
    }
    assert!(placed > ids.len() / 2);
    // The links drawn per page add up to the links' rectangles.
    let mut by_page = 0usize;
    for page in 0..a.pages.len() {
        let links = a.page_links(&f.doc, page);
        assert!(!links.stale && !links.truncated);
        let mut seen = std::collections::BTreeSet::new();
        for link in &links.links {
            assert!(seen.insert(link.id), "listed once per page");
            assert!(link.rects.iter().all(|r| r.page == page));
            by_page += link.rects.len();
        }
    }
    let link_rects: usize = batched
        .iter()
        .filter(|(id, _)| {
            f.doc
                .annotation(*id)
                .is_some_and(|a| a.kind() == Kind::Link)
        })
        .map(|(_, g)| g.rects.len())
        .sum();
    assert_eq!(by_page, link_rects);
}

#[test]
fn every_format_and_the_package_carry_what_is_readable() {
    let f = hostile::annotation_storm().unwrap();
    let schemas = SchemaRegistry::builtin();
    let options = ExportOptions {
        source_namespace: "storm",
        schemas: &schemas,
        fonts: Some(&f.engine.fonts),
    };
    let layout = f.engine.layout(&f.doc);

    // HTML: valid nesting, and re-importing it keeps links (not the comments).
    let html =
        String::from_utf8(Html.export(&f.doc, Some(&layout), &options).unwrap().bytes).unwrap();
    let mut depth = 0i32;
    for part in html.split('<') {
        if part.starts_with("a ") {
            depth += 1;
            assert!(depth <= 1);
        } else if part.starts_with("/a>") {
            depth -= 1;
            assert!(depth >= 0);
        }
    }
    assert_eq!(depth, 0);
    assert!(!html.contains("javascript"));
    // Overlapping links are flattened (the first keeps the shared text), and
    // each anchor comes back as one link.
    let reimported = import_html(&html, Default::default()).unwrap();
    let anchors = html.matches("<a ").count();
    assert!(anchors >= 20, "{anchors}");
    assert_eq!(
        reimported
            .fragment
            .fragment
            .ranges
            .iter()
            .filter(|r| r.annotation.is_some())
            .count(),
        anchors
    );

    // PDF: every link annotation is tagged.
    let pdf = PdfWithAssets {
        assets: &f.engine.assets,
    }
    .export(&f.doc, Some(&layout), &options)
    .unwrap();
    let report = checker::check(&pdf.bytes);
    assert!(report.links > 0);
    assert_eq!(report.links, report.tagged_links);
    assert_eq!(report.links, report.link_uris.len());
    assert!(report.link_uris.iter().all(|u| {
        u.starts_with("https://storm.example/") || u.starts_with("https://edited.example/")
    }));

    // Native copy and paste: links duplicate, comments stay, the bad ones are
    // reported as left out.
    let copied: NativeFragment = copy_all(&f.doc, "storm", &schemas, Some(&layout), None).unwrap();
    assert!(copied.fragment.notes.iter().any(|n| n.code == UNREADABLE));
    let mut editor = Editor::new(Document::new(9).unwrap(), SchemaRegistry::builtin());
    copied.paste(&mut editor, None, "elsewhere").unwrap();
    assert_eq!(active_links(editor.document()), active_links(&f.doc));
    assert!(
        editor
            .document()
            .annotations_of_kind(Kind::Comment)
            .annotations
            .is_empty()
    );
}

#[test]
fn the_package_round_trips_the_annotations_in_both_modes() {
    use reprise_doc::PersistenceMode;
    let f = hostile::annotation_storm().unwrap();
    for mode in [PersistenceMode::History, PersistenceMode::Shallow] {
        let bytes = reprise_format::Package::new(&f.doc, reprise_format::DocumentId([3; 16]), mode)
            .unwrap()
            .save()
            .unwrap();
        let opened = reprise_format::Package::open(
            &bytes,
            5,
            reprise_format::Limits::default(),
            &reprise_format::MigrationRegistry::builtin(),
        )
        .unwrap()
        .into_document()
        .unwrap();
        for kind in [Kind::Link, Kind::Comment] {
            let original = f.doc.annotations_of_kind(kind);
            let back = opened.annotations_of_kind(kind);
            if mode == PersistenceMode::History {
                assert_eq!(back, original, "{mode:?} {kind:?}");
                continue;
            }
            // A shallow package keeps no history: a character deleted before
            // it cannot be found again, so an annotation that was already
            // rebound onto other text (its end was deleted) reports as rebound
            // at the start of the text, as every range does. Every other
            // annotation comes back exactly.
            for a in &original.annotations {
                let AnnotationState::Active { rebound, .. } = &a.state else {
                    continue;
                };
                let found = back.annotations.iter().find(|b| b.id == a.id);
                if !rebound {
                    assert_eq!(found, Some(a), "{mode:?} {kind:?} {}", a.id);
                }
                if let Some(b) = found {
                    assert_eq!(b.data, a.data);
                }
            }
            assert!(back.annotations.len() * 10 >= original.annotations.len() * 9);
        }
        assert!(
            opened
                .annotation_notes()
                .iter()
                .any(|n| n.code == UNREADABLE)
        );
    }
}
