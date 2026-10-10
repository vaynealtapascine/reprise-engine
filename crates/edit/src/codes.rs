//! Stable error diagnostics for refused editing operations (37).
use reprise_diag::Code;

pub const LIMIT: Code = Code::new("edit.limit");
pub const INVALID_COMMAND: Code = Code::new("edit.invalid-command");
/// Invalid page dimensions or margins omitted the requested edit.
pub const PAGE_SETUP_INVALID: Code = Code::new("edit.page-setup-invalid");
pub const STORE: Code = Code::new("edit.store");
