//! FlowHodgeQuota — classify fabric traffic by Hodge component.
//!
//! Interconnect flow on a package is not just "bytes". The Hodge
//! decomposition of a flow on the chiplet graph splits it into:
//!
//! - **Gradient** — tree-like (broadcast, reduce, allreduce-tree). May use
//!   tree offload (a spanning-tree credit path / on-die reduction engine).
//! - **Curl** — a simple cyclic ring. Reserves ring capacity; must not be
//!   collapsed onto a tree.
//! - **Harmonic** — persistent collective cycles (homology). **Must not**
//!   be tree-reduced: mapping a non-trivial cycle onto a spanning tree
//!   identifies distinct edges, so two harmonic collectives can share a
//!   tree buffer and wait for each other (credit deadlock). It also
//!   destroys the homology class the collective's progress depends on.
//!
//! v0.1: one virtual interconnect, integer quotas, real refusal rules.
//! Caps (`CapKind::FlowQuota`) authorize which classes a tenant may send.

use crate::caps::{CapKind, CapRights, Capability};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FlowClass {
    Gradient = 0,
    Curl = 1,
    Harmonic = 2,
}

impl FlowClass {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Gradient),
            1 => Some(Self::Curl),
            2 => Some(Self::Harmonic),
            _ => None,
        }
    }

    pub const fn bit(self) -> u64 {
        1u64 << (self as u8)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gradient => "gradient",
            Self::Curl => "curl",
            Self::Harmonic => "harmonic",
        }
    }
}

pub const CLASS_GRADIENT: u64 = 1;
pub const CLASS_CURL: u64 = 2;
pub const CLASS_HARMONIC: u64 = 4;
pub const CLASS_ALL: u64 = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HodgeError {
    QuotaExceeded,
    /// Harmonic + TREE_OFFLOAD: deadlock / homology collapse (see module docs).
    HarmonicTreeReduce,
    /// Curl is a ring; tree offload is the wrong topology.
    CurlOnTree,
    ClassNotAuthorized,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HodgeQuota {
    pub remain: [u32; 3],
}

impl HodgeQuota {
    pub const fn generous() -> Self {
        Self {
            remain: [64, 64, 64],
        }
    }

    pub const fn empty() -> Self {
        Self { remain: [0, 0, 0] }
    }

    pub fn remain(&self, c: FlowClass) -> u32 {
        self.remain[c as usize]
    }

    /// Policy then quota. Called from `Fabric::send` on the only virtual link.
    pub fn admit(&mut self, flow: FlowClass, tree_offload: bool) -> Result<(), HodgeError> {
        if tree_offload {
            match flow {
                FlowClass::Gradient => {}
                FlowClass::Curl => return Err(HodgeError::CurlOnTree),
                FlowClass::Harmonic => return Err(HodgeError::HarmonicTreeReduce),
            }
        }
        let i = flow as usize;
        if self.remain[i] == 0 {
            return Err(HodgeError::QuotaExceeded);
        }
        self.remain[i] -= 1;
        Ok(())
    }
}

/// Cap surface: `FlowQuota` badge is a class bitmask. WRITE required to send.
pub fn authorize(cap: &Capability, flow: FlowClass) -> Result<(), HodgeError> {
    if cap.kind != CapKind::FlowQuota {
        return Err(HodgeError::ClassNotAuthorized);
    }
    if !cap.rights.contains(CapRights::WRITE) {
        return Err(HodgeError::ClassNotAuthorized);
    }
    if cap.badge & flow.bit() == 0 {
        return Err(HodgeError::ClassNotAuthorized);
    }
    Ok(())
}

/// Host red-team report for HodgeQuota empty-admit refuse.
///
/// Sell line `[redteam] attack=hodge-quota` — existing [`HodgeQuota::admit`]
/// path only. [`HodgeQuota::empty`] then admit → [`HodgeError::QuotaExceeded`];
/// generous admit succeeds. **Not** `hodge-harmonic-tree` / `hodge-curl-tree`,
/// **not** CapTable / [`HodgeError::ClassNotAuthorized`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HodgeQuotaReport {
    /// Empty quota Gradient admit → `QuotaExceeded`.
    pub empty_refused: bool,
    /// Generous quota admits Gradient / Curl / Harmonic (plain, no tree).
    pub generous_ok: bool,
}

impl HodgeQuotaReport {
    pub fn all_ok(&self) -> bool {
        self.empty_refused && self.generous_ok
    }
}

/// `HodgeQuota::empty().admit(...)` → [`HodgeError::QuotaExceeded`].
/// Quota ceiling — not HarmonicTreeReduce / CurlOnTree / ClassNotAuthorized.
pub fn run_hodge_quota_demo() -> HodgeQuotaReport {
    let mut empty = HodgeQuota::empty();
    let empty_refused = empty.admit(FlowClass::Gradient, false) == Err(HodgeError::QuotaExceeded);

    let mut generous = HodgeQuota::generous();
    let generous_ok = generous.admit(FlowClass::Gradient, false).is_ok()
        && generous.admit(FlowClass::Curl, false).is_ok()
        && generous.admit(FlowClass::Harmonic, false).is_ok();

    HodgeQuotaReport {
        empty_refused,
        generous_ok,
    }
}

/// Host red-team report for FlowQuota authorize class refuse.
///
/// Sell line `[redteam] attack=hodge-class-unauthorized` — existing
/// [`authorize`] path only. FlowQuota badge Gradient|Curl: Gradient+Curl
/// authorize OK; Harmonic → [`HodgeError::ClassNotAuthorized`]. Wrong kind
/// (Memory) → ClassNotAuthorized. FlowQuota without WRITE → ClassNotAuthorized.
/// **Not** QuotaExceeded / CurlOnTree / HarmonicTreeReduce; **not** CapTable
/// milestone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HodgeClassUnauthorizedReport {
    /// FlowQuota Gradient|Curl: Gradient + Curl authorize OK.
    pub badge_ok: bool,
    /// Harmonic on Gradient|Curl badge → `ClassNotAuthorized`.
    pub harmonic_refused: bool,
    /// Wrong kind (Memory) → `ClassNotAuthorized`.
    pub wrong_kind: bool,
    /// FlowQuota without WRITE → `ClassNotAuthorized`.
    pub no_write: bool,
}

impl HodgeClassUnauthorizedReport {
    pub fn all_ok(&self) -> bool {
        self.badge_ok && self.harmonic_refused && self.wrong_kind && self.no_write
    }
}

/// `authorize` badge / kind / WRITE refuses → [`HodgeError::ClassNotAuthorized`].
/// Cap class surface — not QuotaExceeded / CurlOnTree / HarmonicTreeReduce / CapTable.
pub fn run_hodge_class_unauthorized_demo() -> HodgeClassUnauthorizedReport {
    use crate::types::TenantId;

    let cap = Capability::new(CapKind::FlowQuota, CapRights::HODGE_FULL, 1, TenantId(1))
        .with_badge(CLASS_GRADIENT | CLASS_CURL)
        .with_generation(1);
    let badge_ok =
        authorize(&cap, FlowClass::Gradient).is_ok() && authorize(&cap, FlowClass::Curl).is_ok();
    let harmonic_refused =
        authorize(&cap, FlowClass::Harmonic) == Err(HodgeError::ClassNotAuthorized);

    let mem =
        Capability::new(CapKind::Memory, CapRights::MEM_FULL, 2, TenantId(1)).with_generation(1);
    let wrong_kind = authorize(&mem, FlowClass::Gradient) == Err(HodgeError::ClassNotAuthorized);

    let no_write_cap = Capability::new(
        CapKind::FlowQuota,
        CapRights(CapRights::READ),
        3,
        TenantId(1),
    )
    .with_badge(CLASS_ALL)
    .with_generation(1);
    let no_write =
        authorize(&no_write_cap, FlowClass::Gradient) == Err(HodgeError::ClassNotAuthorized);

    HodgeClassUnauthorizedReport {
        badge_ok,
        harmonic_refused,
        wrong_kind,
        no_write,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::Capability;
    use crate::types::TenantId;

    #[test]
    fn gradient_tree_ok_curl_tree_refused() {
        let mut q = HodgeQuota::generous();
        assert!(q.admit(FlowClass::Gradient, true).is_ok());
        assert_eq!(
            q.admit(FlowClass::Curl, true).unwrap_err(),
            HodgeError::CurlOnTree
        );
        assert_eq!(
            q.admit(FlowClass::Harmonic, true).unwrap_err(),
            HodgeError::HarmonicTreeReduce
        );
    }

    #[test]
    fn curl_ring_and_harmonic_plain_ok() {
        let mut q = HodgeQuota::generous();
        q.admit(FlowClass::Curl, false).unwrap();
        q.admit(FlowClass::Harmonic, false).unwrap();
        assert_eq!(q.remain(FlowClass::Curl), 63);
        assert_eq!(q.remain(FlowClass::Harmonic), 63);
    }

    #[test]
    fn quota_zero() {
        let mut q = HodgeQuota::empty();
        assert_eq!(
            q.admit(FlowClass::Gradient, false).unwrap_err(),
            HodgeError::QuotaExceeded
        );
    }

    #[test]
    fn cap_authorizes_classes() {
        let cap = Capability::new(CapKind::FlowQuota, CapRights::HODGE_FULL, 1, TenantId(1))
            .with_badge(CLASS_GRADIENT | CLASS_CURL)
            .with_generation(1);
        assert!(authorize(&cap, FlowClass::Gradient).is_ok());
        assert!(authorize(&cap, FlowClass::Curl).is_ok());
        assert_eq!(
            authorize(&cap, FlowClass::Harmonic).unwrap_err(),
            HodgeError::ClassNotAuthorized
        );
    }

    #[test]
    fn hodge_quota_demo_empty_refuses() {
        let r = run_hodge_quota_demo();
        assert!(r.empty_refused, "empty admit → QuotaExceeded");
        assert!(r.generous_ok, "generous Gradient/Curl/Harmonic admit");
        assert!(r.all_ok());
    }

    #[test]
    fn hodge_class_unauthorized_demo_refuses() {
        let r = run_hodge_class_unauthorized_demo();
        assert!(r.badge_ok, "Gradient|Curl badge admits Gradient+Curl");
        assert!(r.harmonic_refused, "Harmonic → ClassNotAuthorized");
        assert!(r.wrong_kind, "Memory kind → ClassNotAuthorized");
        assert!(r.no_write, "FlowQuota without WRITE → ClassNotAuthorized");
        assert!(r.all_ok());
    }

    #[test]
    fn flow_class_tag_round_trip_and_refusal() {
        for c in [FlowClass::Gradient, FlowClass::Curl, FlowClass::Harmonic] {
            assert_eq!(FlowClass::from_u8(c as u8), Some(c));
        }
        for v in [3u8, 4, 7, 0x80, u8::MAX] {
            assert_eq!(FlowClass::from_u8(v), None, "tag {v} must not decode");
        }
        assert_eq!(FlowClass::Gradient.as_str(), "gradient");
        assert_eq!(FlowClass::Curl.as_str(), "curl");
        assert_eq!(FlowClass::Harmonic.as_str(), "harmonic");
    }

    #[test]
    fn class_bits_match_badge_constants() {
        assert_eq!(FlowClass::Gradient.bit(), CLASS_GRADIENT);
        assert_eq!(FlowClass::Curl.bit(), CLASS_CURL);
        assert_eq!(FlowClass::Harmonic.bit(), CLASS_HARMONIC);
        assert_eq!(CLASS_GRADIENT | CLASS_CURL | CLASS_HARMONIC, CLASS_ALL);
        // Bits are disjoint, so one badge bit never authorizes two classes.
        assert_eq!(CLASS_GRADIENT & CLASS_CURL, 0);
        assert_eq!(CLASS_GRADIENT & CLASS_HARMONIC, 0);
        assert_eq!(CLASS_CURL & CLASS_HARMONIC, 0);
    }

    #[test]
    fn topology_policy_checked_before_quota_and_refusal_consumes_nothing() {
        // Policy refusals win over an exhausted quota: the error names the
        // topology mistake, not the ceiling.
        let mut empty = HodgeQuota::empty();
        assert_eq!(
            empty.admit(FlowClass::Curl, true),
            Err(HodgeError::CurlOnTree)
        );
        assert_eq!(
            empty.admit(FlowClass::Harmonic, true),
            Err(HodgeError::HarmonicTreeReduce)
        );
        assert_eq!(
            empty.admit(FlowClass::Gradient, true),
            Err(HodgeError::QuotaExceeded)
        );

        // A policy refusal on a funded quota leaves every counter untouched.
        let mut q = HodgeQuota::generous();
        let before = q;
        assert!(q.admit(FlowClass::Curl, true).is_err());
        assert!(q.admit(FlowClass::Harmonic, true).is_err());
        assert_eq!(q, before);

        // Gradient tree offload is admitted and charged like a plain send.
        q.admit(FlowClass::Gradient, true).unwrap();
        assert_eq!(q.remain(FlowClass::Gradient), 63);
        assert_eq!(q.remain(FlowClass::Curl), 64);
        assert_eq!(q.remain(FlowClass::Harmonic), 64);
    }

    #[test]
    fn per_class_quota_exhausts_independently() {
        let mut q = HodgeQuota::generous();
        for i in 0..64u32 {
            q.admit(FlowClass::Gradient, false).unwrap();
            assert_eq!(q.remain(FlowClass::Gradient), 63 - i);
        }
        assert_eq!(
            q.admit(FlowClass::Gradient, false),
            Err(HodgeError::QuotaExceeded)
        );
        assert_eq!(
            q.admit(FlowClass::Gradient, true),
            Err(HodgeError::QuotaExceeded)
        );
        // Refusal at zero does not wrap the counter.
        assert_eq!(q.remain(FlowClass::Gradient), 0);
        // Other classes keep their full budget.
        assert_eq!(q.remain(FlowClass::Curl), 64);
        assert_eq!(q.remain(FlowClass::Harmonic), 64);
        q.admit(FlowClass::Curl, false).unwrap();
        q.admit(FlowClass::Harmonic, false).unwrap();
    }

    #[test]
    fn single_credit_quota_admits_exactly_once() {
        let mut q = HodgeQuota { remain: [0, 1, 0] };
        assert_eq!(
            q.admit(FlowClass::Gradient, false),
            Err(HodgeError::QuotaExceeded)
        );
        q.admit(FlowClass::Curl, false).unwrap();
        assert_eq!(
            q.admit(FlowClass::Curl, false),
            Err(HodgeError::QuotaExceeded)
        );
        assert_eq!(q, HodgeQuota::empty());
    }

    #[test]
    fn authorize_needs_kind_write_and_class_bit() {
        let t = TenantId(1);
        // WRITE alone (no READ / GRANT) is sufficient with the class bit.
        let write_only = Capability::new(CapKind::FlowQuota, CapRights(CapRights::WRITE), 1, t)
            .with_badge(CLASS_HARMONIC)
            .with_generation(1);
        assert!(authorize(&write_only, FlowClass::Harmonic).is_ok());
        assert_eq!(
            authorize(&write_only, FlowClass::Gradient),
            Err(HodgeError::ClassNotAuthorized)
        );

        // Zero badge authorizes no class, even with full rights.
        let no_classes = Capability::new(CapKind::FlowQuota, CapRights::HODGE_FULL, 2, t)
            .with_badge(0)
            .with_generation(1);
        for c in [FlowClass::Gradient, FlowClass::Curl, FlowClass::Harmonic] {
            assert_eq!(
                authorize(&no_classes, c),
                Err(HodgeError::ClassNotAuthorized)
            );
        }

        // Full badge + full rights authorizes every class.
        let all = Capability::new(CapKind::FlowQuota, CapRights::HODGE_FULL, 3, t)
            .with_badge(CLASS_ALL)
            .with_generation(1);
        for c in [FlowClass::Gradient, FlowClass::Curl, FlowClass::Harmonic] {
            assert!(authorize(&all, c).is_ok());
        }

        // Wrong kind is refused even when rights and badge would pass.
        let wrong_kind = Capability::new(CapKind::Memory, CapRights::HODGE_FULL, 4, t)
            .with_badge(CLASS_ALL)
            .with_generation(1);
        assert_eq!(
            authorize(&wrong_kind, FlowClass::Gradient),
            Err(HodgeError::ClassNotAuthorized)
        );
    }

    #[test]
    fn hodge_reports_all_ok_requires_every_flag() {
        let q = HodgeQuotaReport {
            empty_refused: true,
            generous_ok: true,
        };
        assert!(q.all_ok());
        assert!(!HodgeQuotaReport {
            empty_refused: false,
            ..q
        }
        .all_ok());
        assert!(!HodgeQuotaReport {
            generous_ok: false,
            ..q
        }
        .all_ok());

        let c = HodgeClassUnauthorizedReport {
            badge_ok: true,
            harmonic_refused: true,
            wrong_kind: true,
            no_write: true,
        };
        assert!(c.all_ok());
        assert!(!HodgeClassUnauthorizedReport {
            badge_ok: false,
            ..c
        }
        .all_ok());
        assert!(!HodgeClassUnauthorizedReport {
            harmonic_refused: false,
            ..c
        }
        .all_ok());
        assert!(!HodgeClassUnauthorizedReport {
            wrong_kind: false,
            ..c
        }
        .all_ok());
        assert!(!HodgeClassUnauthorizedReport {
            no_write: false,
            ..c
        }
        .all_ok());
    }
}
