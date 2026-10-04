//! Stable failures. Warning: an extension is substituted or a style layer skipped.
use reprise_diag::Code;
pub const INVALID: Code = Code::new("plugin.invalid");
pub const ABI: Code = Code::new("plugin.abi");
pub const HASH: Code = Code::new("plugin.hash");
pub const CAPABILITY: Code = Code::new("plugin.capability");
pub const LIMIT: Code = Code::new("plugin.limit");
pub const FUEL: Code = Code::new("plugin.fuel");
pub const TRAP: Code = Code::new("plugin.trap");
pub const RESULT: Code = Code::new("plugin.result");
pub const UNAVAILABLE: Code = Code::new("plugin.unavailable");
