use crate::{Diagnostic, ErrorPayload, Payload, Severity};
/// One typed boundary error. Internal concrete error types never escape.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("unsupported payload version {0}")]
    Version(u32),
    #[error("invalid payload: {0}")]
    Invalid(String),
    #[error("boundary limit: {0}")]
    Limit(String),
    #[error("invalid or absent ID: {0}")]
    InvalidId(String),
    #[error("layout result belongs to an older document/configuration")]
    Stale,
    #[error("layout job cancelled")]
    Cancelled,
    #[error("complete current layout required")]
    NoLayout,
    #[error("newer package is read-only")]
    ReadOnly,
    #[error("{code}: {message}")]
    Core {
        code: String,
        severity: Severity,
        message: String,
        command: Option<u32>,
    },
}
pub type Result<T> = std::result::Result<T, Error>;
impl Error {
    pub fn code(&self) -> &str {
        match self {
            Self::Version(_) => "bindings.version",
            Self::Invalid(_) => "bindings.invalid",
            Self::Limit(_) => "bindings.limit",
            Self::InvalidId(_) => "bindings.id",
            Self::Stale => "bindings.stale",
            Self::Cancelled => "bindings.cancelled",
            Self::NoLayout => "bindings.layout-required",
            Self::ReadOnly => "bindings.read-only",
            Self::Core { code, .. } => code,
        }
    }
    pub fn payload(&self) -> Payload<ErrorPayload> {
        Payload::new(ErrorPayload {
            code: self.code().into(),
            severity: match self {
                Self::Core { severity, .. } => *severity,
                _ => Severity::Error,
            },
            message: self.to_string(),
            command: match self {
                Self::Core { command, .. } => *command,
                _ => None,
            },
        })
    }
    pub(crate) fn note(note: reprise_diag::Note) -> Self {
        Self::Core {
            code: note.code.to_string(),
            severity: severity(note.severity),
            message: note.message,
            command: None,
        }
    }
}
pub(crate) fn severity(s: reprise_diag::Severity) -> Severity {
    match s {
        reprise_diag::Severity::Info => Severity::Info,
        reprise_diag::Severity::Warning => Severity::Warning,
        reprise_diag::Severity::Error => Severity::Error,
    }
}
pub(crate) fn diagnostic(n: reprise_diag::Note) -> Diagnostic {
    Diagnostic {
        code: n.code.to_string(),
        severity: severity(n.severity),
        message: n.message,
        subject: None,
        start: n.bytes.as_ref().map(|r| r.start as u32),
        end: n.bytes.as_ref().map(|r| r.end as u32),
    }
}
impl From<reprise_edit::EditError> for Error {
    fn from(e: reprise_edit::EditError) -> Self {
        let command = e.command.map(|i| i as u32);
        let mut error = Self::note(e.note());
        if let Self::Core { command: c, .. } = &mut error {
            *c = command;
        }
        error
    }
}
impl From<reprise_format::FormatError> for Error {
    fn from(e: reprise_format::FormatError) -> Self {
        Self::note(e.note())
    }
}
impl From<reprise_clipboard::ClipboardError> for Error {
    fn from(e: reprise_clipboard::ClipboardError) -> Self {
        Self::note(e.note())
    }
}
impl From<reprise_doc::DocError> for Error {
    fn from(e: reprise_doc::DocError) -> Self {
        Self::Core {
            code: "bindings.store".into(),
            severity: Severity::Error,
            message: e.to_string(),
            command: None,
        }
    }
}
impl From<reprise_layout::incremental::JobError> for Error {
    fn from(e: reprise_layout::incremental::JobError) -> Self {
        match e {
            reprise_layout::incremental::JobError::Stale => Self::Stale,
            reprise_layout::incremental::JobError::Cancelled => Self::Cancelled,
        }
    }
}
