//! Font supply fixture inputs, using only pinned bundled bytes.
use reprise_font::{Descriptors, FontDeclaration, FontStore, GenericFamily};
pub fn declaration(family: &str, index: u32) -> FontDeclaration {
    FontDeclaration {
        family: family.into(),
        descriptors: Descriptors::default(),
        face_index: index,
    }
}
/// Three scalars forcing three faces: Script -> Serif -> Sans.
pub fn three_face_text() -> String {
    let fonts = FontStore::default();
    let script = fonts.generic(GenericFamily::Script);
    let serif = fonts.generic(GenericFamily::Serif);
    let sans = fonts.generic(GenericFamily::SansSerif);
    let middle = (32..0x3000)
        .filter_map(char::from_u32)
        .find(|c| c.is_alphabetic() && !script.covers(*c) && serif.covers(*c))
        .expect("pinned Serif has a letter absent from Script");
    let last = (32..0x3000)
        .filter_map(char::from_u32)
        .find(|c| c.is_alphabetic() && !script.covers(*c) && !serif.covers(*c) && sans.covers(*c))
        .expect("pinned Sans has a letter absent from Script and Serif");
    format!("a{middle}{last}")
}
/// A deterministic two-face OTC assembled from unchanged upstream tables.
/// Only table-directory offsets are relocated into the collection.
pub fn collection() -> Vec<u8> {
    let fonts = FontStore::default();
    let sources = [
        fonts.generic(GenericFamily::Serif).data(),
        fonts.generic(GenericFamily::SansSerif).data(),
    ];
    let mut bytes = Vec::from(b"ttcf\0\x01\0\0\0\0\0\x02".as_slice());
    bytes.extend_from_slice(&20u32.to_be_bytes());
    let second = 20 + sources[0].len();
    bytes.extend_from_slice(&(second as u32).to_be_bytes());
    for source in sources {
        let base = bytes.len() as u32;
        let mut font = source.to_vec();
        let tables = u16::from_be_bytes([font[4], font[5]]) as usize;
        for table in 0..tables {
            let at = 12 + table * 16 + 8;
            let old = u32::from_be_bytes(font[at..at + 4].try_into().expect("pinned directory"));
            font[at..at + 4].copy_from_slice(&(old + base).to_be_bytes());
        }
        bytes.extend_from_slice(&font);
    }
    bytes
}
