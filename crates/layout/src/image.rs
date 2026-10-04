//! Integer image sizing; only metadata reaches layout (19, 24, 37, 38).
use crate::{Diagnostic, Engine, ImageLayout, Subject, codes};
use reprise_diag::Severity;
use reprise_display::assets::{HeaderError, image_header};
use reprise_doc::{Document, NodeId};
use reprise_geom::{Length, Point, Rect};

pub(crate) fn prepare(
    engine: &Engine,
    doc: &Document,
    node: NodeId,
    em: Length,
    width_limit: Length,
    diagnostics: &mut Vec<Diagnostic>,
) -> ImageLayout {
    let alt = doc
        .block(node)
        .map_or_else(|_| String::new(), |b| b.text.to_string());
    let image = doc.image(node).ok();
    let asset = image.as_ref().map_or_else(String::new, |i| i.asset.clone());
    let subject = Subject::Node(node);
    let mut report = |code, message| {
        diagnostics.push(Diagnostic::new(
            Severity::Warning,
            code,
            subject.clone(),
            message,
        ))
    };
    let intrinsic = if image.is_none() {
        report(
            codes::IMAGE_RECORD,
            "unreadable image record retained; using a placeholder",
        );
        None
    } else if let Some(bytes) = engine.assets.get(&asset) {
        match image_header(bytes) {
            Ok(header) => Some(header),
            Err(error) => {
                report(
                    if error == HeaderError::Limit {
                        codes::IMAGE_LIMIT
                    } else {
                        codes::IMAGE_HEADER
                    },
                    "image header unavailable; using a placeholder",
                );
                None
            }
        }
    } else {
        report(
            codes::IMAGE_MISSING,
            "image asset missing; using a placeholder",
        );
        None
    };
    let (natural_w, natural_h) = intrinsic
        .map_or((Length::from_pt(96), Length::from_pt(72)), |h| {
            (h.physical_width, h.physical_height)
        });
    let authored_w = image.as_ref().and_then(|i| i.width).map(|v| v.resolve(em));
    let authored_h = image.as_ref().and_then(|i| i.height).map(|v| v.resolve(em));
    if authored_w.is_some_and(|w| w < Length::ZERO) || authored_h.is_some_and(|h| h < Length::ZERO)
    {
        report(
            codes::IMAGE_SIZE,
            "negative authored image size clamped to zero",
        );
    }
    let (mut width, mut height) = match (authored_w, authored_h) {
        (Some(w), Some(h)) => (w.max(Length::ZERO), h.max(Length::ZERO)),
        (Some(w), None) => {
            let w = w.max(Length::ZERO);
            (
                w,
                intrinsic.map_or_else(
                    || w.mul_ratio(natural_h.0, natural_w.0.max(1)),
                    |h| h.height_for_width(w),
                ),
            )
        }
        (None, Some(h)) => {
            let h = h.max(Length::ZERO);
            (
                intrinsic.map_or_else(
                    || h.mul_ratio(natural_w.0, natural_h.0.max(1)),
                    |header| header.width_for_height(h),
                ),
                h,
            )
        }
        _ => (natural_w, natural_h),
    };
    let limit = width_limit.max(Length::ZERO);
    if width > limit {
        height = height.mul_ratio(limit.0, width.0);
        width = limit;
        report(
            codes::IMAGE_SIZE,
            "image scaled proportionally to the frame's inline extent",
        );
    }
    ImageLayout {
        asset,
        alt,
        frame: 0,
        rect: Rect::new(Point::new(Length::ZERO, Length::ZERO), width, height),
        placeholder: intrinsic.is_none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reprise_doc::image::ImageData;
    use reprise_font::FontStore;

    #[test]
    fn authored_width_preserves_aspect_when_intrinsic_lengths_saturate() {
        let mut bytes = include_bytes!("../../../fixtures/images/red-2x1.jpg").to_vec();
        bytes[14..18].copy_from_slice(&[0, 1, 0, 1]);
        let sof = bytes.windows(2).position(|w| w == [255, 192]).unwrap();
        bytes[sof + 7..sof + 9].copy_from_slice(&u16::MAX.to_be_bytes());
        let mut engine = Engine::new(FontStore::default());
        let mut authored = ImageData::new(engine.assets.insert(bytes).unwrap());
        authored.width = Some(reprise_doc::LengthExpr::Pt(Length::from_pt(65535)));
        let doc = Document::new(1).unwrap();
        let node = doc.append_image("", &authored, "alt").unwrap();
        let mut notes = Vec::new();
        let image = prepare(
            &engine,
            &doc,
            node,
            Length::from_pt(12),
            Length::MAX,
            &mut notes,
        );
        assert_eq!(
            (image.rect.width, image.rect.height),
            (Length::from_pt(65535), Length::from_pt(1))
        );
        assert!(notes.is_empty());
    }

    #[test]
    fn bounded_header_failures_are_warnings_and_missing_assets_keep_authored_sizes() {
        let mut engine = Engine::new(FontStore::default());
        let mut stalled = vec![255, 216];
        stalled.resize(reprise_display::assets::MAX_HEADER_BYTES + 2, 255);
        let hash = engine.assets.insert(stalled).unwrap();
        let doc = Document::new(1).unwrap();
        let node = doc.append_image("", &ImageData::new(hash), "alt").unwrap();
        let mut notes = Vec::new();
        let image = prepare(
            &engine,
            &doc,
            node,
            Length::from_pt(12),
            Length::MAX,
            &mut notes,
        );
        assert!(image.placeholder);
        assert_eq!(notes[0].code, codes::IMAGE_LIMIT);
        assert_eq!(notes[0].severity, Severity::Warning);
        let mut data = ImageData::new("0".repeat(64));
        data.width = Some(reprise_doc::LengthExpr::Em(2000));
        data.height = Some(reprise_doc::LengthExpr::Pt(Length::from_pt(40)));
        doc.set_image(node, &data).unwrap();
        notes.clear();
        let image = prepare(
            &engine,
            &doc,
            node,
            Length::from_pt(12),
            Length::MAX,
            &mut notes,
        );
        assert_eq!(
            (image.rect.width, image.rect.height),
            (Length::from_pt(24), Length::from_pt(40))
        );
        assert_eq!(notes[0].code, codes::IMAGE_MISSING);
    }
}
