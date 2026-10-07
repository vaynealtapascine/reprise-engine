//! What a failed oracle looks like.

use std::fmt;

/// An oracle that didn't hold. `oracle` is a stable short name (so a
/// regression test can say which invariant it pins); `op` is the index of the
/// op that was running, when there was one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    pub oracle: &'static str,
    pub op: Option<usize>,
    pub detail: String,
}

impl Violation {
    pub fn new(oracle: &'static str, detail: impl Into<String>) -> Violation {
        Violation {
            oracle,
            op: None,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.op {
            Some(op) => write!(f, "[{}] at op {op}: {}", self.oracle, self.detail),
            None => write!(f, "[{}] {}", self.oracle, self.detail),
        }
    }
}

impl std::error::Error for Violation {}

pub type R<T = ()> = Result<T, Violation>;

/// `ensure!(condition, "oracle-name", "detail {}", args)` returns a
/// [`Violation`] from the enclosing function when the condition is false.
#[macro_export]
macro_rules! ensure {
    ($cond:expr, $oracle:expr, $($arg:tt)*) => {
        if !$cond {
            return Err($crate::violation::Violation::new($oracle, format!($($arg)*)));
        }
    };
}
