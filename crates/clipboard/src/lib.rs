//! Deterministic native clipboard, bounded HTML import and honest exporters (35).
pub mod codes;
mod export;
mod html;
mod image_export;
mod native;
mod pdf_tags;
pub use image_export::{NativeWithAssets, PdfWithAssets};
pub use pdf_tags::{PdfMetadata, cell_role, heading_level};

pub use export::{
    Disposition, ExportOptions, ExportResult, Exporter, Feature, Html, Loss, LossReport, Native,
    Pdf, PlainText, export_pdf,
};
pub use html::{Import, ImportLimits, import_html, import_plain};
pub use native::{
    EmbeddedResource, NativeFragment, ResourceKind, copy_all, copy_blocks, copy_selection,
    resource_key,
};
pub use reprise_doc::fragment::{CopyBlock, Fragment, FragmentError};

#[derive(Debug, thiserror::Error)]
pub enum ClipboardError {
    #[error(transparent)]
    Fragment(#[from] FragmentError),
    #[error("clipboard exceeds the {0} limit")]
    Limit(&'static str),
    #[error("invalid clipboard data: {0}")]
    Invalid(String),
    #[error("resource hash or font identity mismatch")]
    ResourceHash,
    #[error(transparent)]
    Edit(#[from] reprise_edit::EditError),
    #[error("export failed: {0}")]
    Export(String),
}

impl ClipboardError {
    pub fn note(&self) -> reprise_diag::Note {
        if let Self::Edit(error) = self {
            return error.note();
        }
        let code = match self {
            Self::Limit(_) | Self::Fragment(FragmentError::Limit(_)) => codes::LIMIT,
            Self::Fragment(FragmentError::Version(_)) => codes::VERSION,
            Self::ResourceHash => codes::RESOURCE_HASH,
            _ => codes::INVALID,
        };
        reprise_diag::Note::error(code, self.to_string())
    }
}
