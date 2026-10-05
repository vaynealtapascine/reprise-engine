//! Bytes in, choices out.
//!
//! Every byte string is a valid scenario: [`Input`] never fails. Once the data
//! runs out it answers zero, which is always a legal choice. That is what lets
//! libFuzzer mutate raw bytes, a seeded generator produce them, and the
//! minimiser delete any slice of them.

/// A cursor over the scenario bytes.
pub struct Input<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Input<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Input { data, pos: 0 }
    }

    /// True once every byte has been consumed.
    pub fn is_empty(&self) -> bool {
        self.pos >= self.data.len()
    }

    pub fn u8(&mut self) -> u8 {
        let byte = self.data.get(self.pos).copied().unwrap_or(0);
        self.pos = self.pos.saturating_add(1);
        byte
    }

    pub fn u16(&mut self) -> u16 {
        u16::from_le_bytes([self.u8(), self.u8()])
    }

    /// A choice in `0..n`; `0` when `n` is zero.
    pub fn below(&mut self, n: usize) -> usize {
        match n {
            0 | 1 => 0,
            2..=256 => usize::from(self.u8()) % n,
            _ => usize::from(self.u16()) % n,
        }
    }

    /// True with probability about `numerator / 256`.
    pub fn chance(&mut self, numerator: u8) -> bool {
        self.u8() < numerator
    }
}

/// A deterministic byte string for a seed (xorshift64*), identical on every
/// platform. Random scenarios are `Input`s over these bytes, so a seed and a
/// length reproduce a scenario exactly.
pub fn seed_bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
    if state == 0 {
        state = 0x2545_F491_4F6C_DD1D;
    }
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let word = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
        for byte in word.to_le_bytes() {
            if out.len() < len {
                out.push(byte);
            }
        }
    }
    out
}

/// 64-bit FNV-1a: a stable digest for the determinism transcript.
pub fn fnv(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhausted_input_answers_zero() {
        let mut input = Input::new(&[7]);
        assert_eq!(input.u8(), 7);
        assert!(input.is_empty());
        assert_eq!(input.u8(), 0);
        assert_eq!(input.u16(), 0);
        assert_eq!(input.below(0), 0);
        assert_eq!(input.below(1000), 0);
        assert!(!input.chance(0));
    }

    #[test]
    fn seeds_are_stable() {
        assert_eq!(seed_bytes(1, 12), seed_bytes(1, 12));
        assert_ne!(seed_bytes(1, 12), seed_bytes(2, 12));
        assert_eq!(seed_bytes(0, 3).len(), 3);
        assert!(seed_bytes(5, 0).is_empty());
    }
}
