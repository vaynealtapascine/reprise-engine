fn main() -> Result<(), Box<dyn std::error::Error>> {
    use reprise::*;
    let mut s = Workspace::new().create(&Payload::new(Create {
        document_id: "00112233445566778899aabbccddeeff".into(),
        peer_id: "1".into(),
    }))?;
    s.apply(&Payload::new(Transaction {
        commands: vec![Command::InsertBlock {
            parent: None,
            index: 0,
            block_kind: BlockKind::Paragraph,
            text: "office ffi \u{05d0}\u{05d1}\u{05d2}".into(),
            style: Style {
                families: Some(vec!["Source Serif Pro".into(), "serif".into()]),
                ..Style::default()
            },
        }],
    }))?;
    let mut job = s.start_layout(&Payload::new(LayoutOptions::default()))?;
    for _ in 0..10_000 {
        if job.step(&mut s, 1)?.data.complete {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/display.json");
            std::fs::write(path, s.display_json(0)?.data)?;
            return Ok(());
        }
    }
    Err("explicit example job bound exceeded".into())
}
