//! Orchestrator review, from the clipboard and font-supply merge: fonts are
//! now identified by their declared family and collection index, not by
//! their bytes alone. Two faces from one collection file, declared under
//! their own families, must both travel in a fragment, keep their identities,
//! and reinstall into another store as the same faces.

use reprise_clipboard::{NativeFragment, ResourceKind, copy_all};
use reprise_doc::{BlockKind, Document, SchemaRegistry, Style};
use reprise_fixtures::fonts::{collection, declaration};
use reprise_fixtures::{PEER, engine};
use reprise_font::FontStore;

#[test]
fn collection_faces_declared_under_aliases_travel_and_reinstall() {
    let mut engine = engine();
    let first = engine
        .fonts
        .register(collection(), declaration("Alias One", 0))
        .unwrap();
    let second = engine
        .fonts
        .register(collection(), declaration("Alias Two", 1))
        .unwrap();
    let doc = Document::new(PEER).unwrap();
    for (name, family) in [("one", "Alias One"), ("two", "Alias Two")] {
        doc.define_style(
            name,
            &Style {
                families: Some(vec![family.into(), "serif".into()]),
                ..Default::default()
            },
        )
        .unwrap();
        doc.append_block(BlockKind::Paragraph, name, "office café")
            .unwrap();
    }
    doc.commit();
    let layout = engine.layout(&doc);
    let schemas = SchemaRegistry::builtin();
    let fragment = copy_all(&doc, "source", &schemas, Some(&layout), Some(&engine.fonts)).unwrap();
    let decoded = NativeFragment::decode(&fragment.encode().unwrap()).unwrap();
    let pins: Vec<_> = decoded
        .resources
        .values()
        .filter_map(|r| match &r.kind {
            ResourceKind::Font { pin, .. } => Some(pin.clone()),
            ResourceKind::Asset => None,
        })
        .collect();
    assert!(pins.contains(&first), "first collection face travels");
    assert!(pins.contains(&second), "second collection face travels");
    let mut fonts = FontStore::default();
    decoded.install_fonts(&mut fonts).unwrap();
    assert!(fonts.get(&first).is_ok() && fonts.get(&second).is_ok());
}
