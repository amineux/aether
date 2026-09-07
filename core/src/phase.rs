//! Explicit execution phases (Graphcore / Tenstorrent-inspired).
//!
//! Jobs and fabric messages carry a named phase tag. The kernel does not
//! fuse phases or rewrite graphs — it only admits / accounts them.

/// Named phase on a job or message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    /// Local FLOPs / ISA dispatch on a bound activity.
    Compute = 0,
    /// Explicit DMA / NoC / fabric exchange between places.
    Exchange = 1,
    /// Collective / timeline barrier (no payload compute).
    Barrier = 2,
}

impl Phase {
    pub const fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Compute),
            1 => Some(Self::Exchange),
            2 => Some(Self::Barrier),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Compute => "COMPUTE",
            Self::Exchange => "EXCHANGE",
            Self::Barrier => "BARRIER",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_named_phases() {
        assert_eq!(Phase::Compute.name(), "COMPUTE");
        assert_eq!(Phase::Exchange as u8, 1);
        assert_eq!(Phase::Barrier as u8, 2);
    }
}
