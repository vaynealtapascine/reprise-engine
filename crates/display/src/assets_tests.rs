use super::*;

const RED: &[u8] = include_bytes!("../../../fixtures/images/red-1x1.png");

#[test]
fn headers_are_integer_and_do_not_decode_pixels() {
    let header = image_header(RED).unwrap();
    assert_eq!((header.width, header.height), (1, 1));
    assert_eq!(header.physical_width, Length(768));
    let large = image_header(include_bytes!("../../../fixtures/images/header-65535.png")).unwrap();
    assert_eq!((large.width, large.height), (65535, 65535));
    assert_eq!(large.physical_width, Length(65535 * 768));
    let density = image_header(include_bytes!("../../../fixtures/images/density.png")).unwrap();
    assert_eq!(density.physical_width, Length(1024));
    let jpeg = image_header(include_bytes!("../../../fixtures/images/red-2x1.jpg")).unwrap();
    assert_eq!(
        (jpeg.physical_width, jpeg.physical_height),
        (Length::from_pt(2), Length::from_pt(1))
    );
}

#[test]
fn every_prefix_and_metadata_mutation_is_total() {
    for input in [
        RED,
        include_bytes!("../../../fixtures/images/red-2x1.jpg").as_slice(),
    ] {
        for end in 0..input.len() {
            let _ = image_header(&input[..end]);
        }
        for pos in 0..input.len() {
            let mut corrupt = input.to_vec();
            corrupt[pos] ^= 255;
            let _ = image_header(&corrupt);
        }
    }
    assert_eq!(image_header(&[]), Err(HeaderError::Unsupported));
    let mut bad = RED.to_vec();
    bad[16] = 255;
    assert_eq!(image_header(&bad), Err(HeaderError::Invalid));
}

#[test]
fn scans_have_explicit_limits_and_extreme_sizes_saturate() {
    let mut fills = vec![255, 216];
    fills.resize(MAX_HEADER_BYTES + 4, 255);
    assert_eq!(image_header(&fills), Err(HeaderError::Limit));
    let mut markers = vec![255, 216];
    for _ in 0..=MAX_HEADER_PARTS {
        markers.extend_from_slice(&[255, 1]);
    }
    assert_eq!(image_header(&markers), Err(HeaderError::Limit));
    assert_eq!(length(u32::MAX, u32::MAX, 1), Length::MAX);
    assert_eq!(length(1, 1, u64::MAX), Length::ZERO);
}

#[test]
fn store_hashes_verify_content_and_bytes_stay_host_owned() {
    let mut store = AssetStore::default();
    let hash = store.insert(RED).unwrap();
    assert_eq!(hash.len(), 64);
    assert_eq!(store.get(&hash), Some(RED));
    assert_eq!(store.insert(RED).unwrap(), hash);
    assert_eq!(store.remove(&hash).unwrap().as_ref(), RED);
    assert!(store.get(&hash).is_none());
}

#[test]
fn exif_rational_resolution_handles_both_byte_orders_and_hostile_offsets() {
    for little in [false, true] {
        let mut tiff = Vec::new();
        let short = |v: u16| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let long = |v: u32| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        tiff.extend_from_slice(if little { b"II" } else { b"MM" });
        tiff.extend_from_slice(&short(42));
        tiff.extend_from_slice(&long(8));
        tiff.extend_from_slice(&short(3));
        for (tag, at) in [(0x011a, 50u32), (0x011b, 58)] {
            tiff.extend_from_slice(&short(tag));
            tiff.extend_from_slice(&short(5));
            tiff.extend_from_slice(&long(1));
            tiff.extend_from_slice(&long(at));
        }
        tiff.extend_from_slice(&short(0x0128));
        tiff.extend_from_slice(&short(3));
        tiff.extend_from_slice(&long(1));
        tiff.extend_from_slice(&short(2));
        tiff.extend_from_slice(&short(0));
        tiff.extend_from_slice(&long(0));
        for _ in 0..2 {
            tiff.extend_from_slice(&long(300));
            tiff.extend_from_slice(&long(2));
        }
        assert_eq!(exif_density(&tiff).unwrap(), Some((2, (300, 2), (300, 2))));
        let source = include_bytes!("../../../fixtures/images/red-2x1.jpg");
        let mut jpeg = vec![255, 216, 255, 225];
        jpeg.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
        jpeg.extend_from_slice(b"Exif\0\0");
        jpeg.extend_from_slice(&tiff);
        let app0_end = 4 + u16be(&source[4..]).unwrap() as usize;
        jpeg.extend_from_slice(&source[app0_end..]);
        let header = image_header(&jpeg).unwrap();
        assert_eq!(header.physical_width, Length(983));
        assert_eq!(header.physical_height, Length(492));
        for end in 0..tiff.len() {
            let _ = exif_density(&tiff[..end]);
        }
        tiff[8..10].copy_from_slice(&short(513));
        assert_eq!(exif_density(&tiff), Err(HeaderError::Limit));
        tiff[4..8].copy_from_slice(&long(u32::MAX));
        assert_eq!(exif_density(&tiff), Err(HeaderError::Invalid));
    }
    assert_eq!(density_length(65535, 2, (1, u32::MAX)), Length::MAX);
}
