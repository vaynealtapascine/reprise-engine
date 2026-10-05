fn main() -> Result<(), Box<dyn std::error::Error>> {
    use reprise::*;
    let mut document = Workspace::new().create(&Payload::new(Create {
        document_id: "00112233445566778899aabbccddeeff".into(),
        peer_id: "11".into(),
    }))?;
    let asset = document
        .register_asset(
            &Payload::new(AssetDeclaration {
                id: "red".into(),
                kind: AssetKind::Image,
            }),
            include_bytes!("../../../fixtures/images/red-1x1.png"),
        )?
        .data;
    document.insert_image(&Payload::new(ImageInsert {
        at: None,
        asset,
        alt: "A red café image".into(),
        width: Some(20 * 1024),
        height: Some(20 * 1024),
        style: Style::default(),
    }))?;
    let mut job = document.start_layout(&Payload::new(LayoutOptions::default()))?;
    for _ in 0..10_000 {
        if job.step(&mut document, 1)?.data.complete {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/images.json");
            std::fs::write(path, document.display_json(0)?.data)?;
            return Ok(());
        }
    }
    Err("explicit image example job bound exceeded".into())
}
