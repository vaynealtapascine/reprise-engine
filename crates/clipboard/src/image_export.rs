//! Asset-aware exports are additive; the frozen ExportOptions stays unchanged.
use crate::{
    ClipboardError, Disposition, ExportOptions, ExportResult, Exporter, Feature, Native, copy_all,
};
use reprise_display::AssetStore;
use reprise_doc::{BlockKind, Document};
use reprise_layout::LayoutSnapshot;

pub struct NativeWithAssets<'a> {
    pub assets: &'a AssetStore,
}
pub struct PdfWithAssets<'a> {
    pub assets: &'a AssetStore,
}

fn set_assets(result: &mut ExportResult, preserved: bool, detail: &str) {
    if let Some(loss) = result
        .losses
        .features
        .iter_mut()
        .find(|f| f.feature == Feature::Assets)
    {
        loss.disposition = if preserved {
            Disposition::Preserved
        } else {
            Disposition::Approximated
        };
        loss.detail = detail.into();
    }
}

impl Exporter for NativeWithAssets<'_> {
    fn export(
        &self,
        doc: &Document,
        layout: Option<&LayoutSnapshot>,
        options: &ExportOptions<'_>,
    ) -> Result<ExportResult, ClipboardError> {
        let mut result = Native.export(doc, layout, options)?;
        let mut fragment = copy_all(
            doc,
            options.source_namespace,
            options.schemas,
            layout,
            options.fonts,
        )?;
        fragment.attach_images(self.assets)?;
        let preserved = doc
            .document_order()
            .into_iter()
            .filter(|&node| doc.kind_of(node) == Some(BlockKind::Image))
            .all(|node| {
                doc.image(node)
                    .is_ok_and(|i| self.assets.get(&i.asset).is_some())
            });
        result.bytes = fragment.encode()?;
        result.losses.notes = fragment.notes;
        set_assets(
            &mut result,
            preserved,
            "authored image records and available image bytes bundled; missing resources reported",
        );
        Ok(result)
    }
}

impl Exporter for PdfWithAssets<'_> {
    fn export(
        &self,
        doc: &Document,
        layout: Option<&LayoutSnapshot>,
        options: &ExportOptions<'_>,
    ) -> Result<ExportResult, ClipboardError> {
        let layout = layout.ok_or_else(|| ClipboardError::Export("PDF requires layout".into()))?;
        let fonts = options
            .fonts
            .ok_or_else(|| ClipboardError::Export("PDF requires fonts".into()))?;
        let mut result = crate::export_pdf(
            doc,
            layout,
            fonts,
            self.assets,
            &crate::PdfMetadata::default(),
        )?;
        let preserved = doc
            .document_order()
            .into_iter()
            .filter(|&node| doc.kind_of(node) == Some(BlockKind::Image))
            .all(|node| {
                layout
                    .block(node)
                    .and_then(|b| b.image.as_ref())
                    .is_some_and(|i| {
                        !i.placeholder
                            && i.rect.width > reprise_geom::Length::ZERO
                            && i.rect.height > reprise_geom::Length::ZERO
                            && self
                                .assets
                                .get(&i.asset)
                                .is_some_and(reprise_display::image_renderable)
                    })
            });
        set_assets(
            &mut result,
            preserved,
            "placed images embedded with ordered ActualText; missing, undecodable or unplaced images use placeholders or are omitted",
        );
        Ok(result)
    }
}
