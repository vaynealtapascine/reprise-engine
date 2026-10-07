//! Shrinking a failing byte string.
//!
//! Every byte string is a valid scenario, so any subsequence of a failing
//! input is a candidate. This is delta debugging (ddmin) over the bytes: it
//! removes ever smaller chunks while the failure persists, then zeroes bytes
//! that don't matter so the result is easy to read.

use crate::violation::Violation;

/// Shrinks `data` while `fails` keeps returning the same oracle.
///
/// `fails` runs a candidate and returns the violation it produced, if any.
/// Only candidates that violate `oracle` count: a smaller input that fails
/// differently is a different bug.
pub fn minimize(
    data: &[u8],
    oracle: &str,
    mut fails: impl FnMut(&[u8]) -> Option<Violation>,
) -> Vec<u8> {
    let mut same = |candidate: &[u8]| fails(candidate).is_some_and(|v| v.oracle == oracle);
    let mut best = data.to_vec();
    if !same(&best) {
        return best;
    }
    let mut chunk = best.len().div_ceil(2).max(1);
    loop {
        let mut start = 0;
        let mut shrunk = false;
        while start < best.len() {
            let end = (start + chunk).min(best.len());
            let mut candidate = best[..start].to_vec();
            candidate.extend_from_slice(&best[end..]);
            if same(&candidate) {
                best = candidate;
                shrunk = true;
            } else {
                start = end;
            }
        }
        if chunk == 1 && !shrunk {
            break;
        }
        if !shrunk || chunk > 1 {
            chunk = (chunk / 2).max(1);
        }
    }
    // Zero what doesn't matter: zeros are the decoder's "first choice".
    for at in 0..best.len() {
        if best[at] != 0 {
            let kept = best[at];
            best[at] = 0;
            if !same(&best) {
                best[at] = kept;
            }
        }
    }
    while best.last() == Some(&0) {
        best.pop();
        if !same(&best) {
            best.push(0);
            break;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn violation(oracle: &'static str) -> Violation {
        Violation::new(oracle, "test")
    }

    #[test]
    fn shrinks_to_the_bytes_that_matter() {
        let mut data = vec![1u8; 200];
        data[77] = 9;
        data[150] = 7;
        let minimal = minimize(&data, "x", |bytes| {
            (bytes.contains(&9) && bytes.contains(&7)).then(|| violation("x"))
        });
        assert_eq!(minimal, vec![9, 7]);
    }

    #[test]
    fn a_different_failure_is_not_a_reduction() {
        let data = vec![5u8, 6, 7];
        let minimal = minimize(&data, "x", |bytes| {
            Some(violation(if bytes.len() == 3 { "x" } else { "y" }))
        });
        // Byte normalisation is allowed when it retains oracle x, but no
        // length reduction may switch to oracle y.
        assert_eq!(minimal, vec![0, 0, 0]);
    }

    #[test]
    fn a_passing_input_comes_back_unchanged() {
        assert_eq!(minimize(&[1, 2], "x", |_| None), vec![1, 2]);
    }
}
