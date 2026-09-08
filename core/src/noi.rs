//! SoftNoI-IS: Interference Score admit policy on a fake NoI.
//!
//! SpectraScout M5–6 leftover (after SoftChipletSync + SoftCCT).
//! SoftChipletSync advertises a per-tenant IS estimate. Soft-CP /
//! XQueue refuse when the **projected** IS exceeds the budget
//! (canonical 1.5×).
//!
//! **Inspiration (not a port, not a product):**
//! [PARL / NoI](https://arxiv.org/abs/2510.24113) (“Taming the Tail”)
//! defines an Interference Score
//! `IS = max_k T_solo(k) / T_con(k)` — worst-case concurrent/solo
//! slowdown on a Network-on-Interposer. PARL uses IS as a
//! **topology-synthesis** objective. This crate reuses the *metric*
//! as **runtime admit control** on a shared fake NoI.
//!
//! **Not claimed.** This is not PARL topology synthesis, not UniCNet,
//! not optimal NoI design, not a silicon interposer, not UCIe.
//! Host tests measure integer throughput units and admit/refuse —
//! not FLOPs, not partner NoI latency, not a multi-chiplet sim.

use crate::types::TenantId;

/// Two Soft-CP tenants on one fake NoI. Software cap, not a die count.
pub const MAX_NOI_TENANTS: usize = 2;
/// Fake shared-NoI capacity (integer BW units). Not GB/s, not FLOPs.
pub const NOI_CAPACITY: u32 = 1000;
/// Canonical admit budget: refuse when projected IS > 1.5.
pub const IS_BUDGET_MILLI: u32 = 1500;
/// Light demand: two tenants fit under capacity (IS = 1.0).
pub const DEMO_LIGHT_DEMAND: u32 = 400;
/// Heavy demand: two tenants oversubscribe (IS = 1.6 > 1.5).
pub const DEMO_HEAVY_DEMAND: u32 = 800;
/// Milli for 1.0× (solo / under-capacity concurrent).
pub const IS_SOLO_MILLI: u32 = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoiError {
    BadArg,
    /// Projected IS exceeds the budget.
    OverBudget,
    /// Fake NoI already holds [`MAX_NOI_TENANTS`].
    Exhausted,
    Unbound,
}

/// Occupied tenant + injection demand (bytes-per-wave analogue).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoiOccupant {
    pub tenant: TenantId,
    pub demand: u32,
}

/// Throughput clip used to compute IS. Integer units, not FLOPs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoiTput {
    pub demand: u32,
    pub solo: u32,
    pub concurrent: u32,
    /// `solo * 1000 / concurrent`. 1000 = 1.0×.
    pub is_milli: u32,
}

impl NoiTput {
    pub const fn from_demand(demand: u32, sum: u32, cap: u32) -> Self {
        let solo = tput_solo(demand, cap);
        let concurrent = tput_con(demand, sum, cap);
        Self {
            demand,
            solo,
            concurrent,
            is_milli: is_milli(solo, concurrent),
        }
    }
}

/// Per-tenant advertisement SoftChipletSync exposes to Soft-CP / XQueue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IsEstimate {
    pub tenant: TenantId,
    pub demand: u32,
    pub is_milli: u32,
    pub worst_is_milli: u32,
}

/// Shared fake Network-on-Interposer. Software model, not a mesh synth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoftNoI {
    capacity: u32,
    budget_milli: u32,
    enabled: bool,
    slots: [Option<NoiOccupant>; MAX_NOI_TENANTS],
    admitted: u32,
    refused: u32,
}

impl SoftNoI {
    pub const fn new() -> Self {
        Self {
            capacity: NOI_CAPACITY,
            budget_milli: IS_BUDGET_MILLI,
            enabled: false,
            slots: [None; MAX_NOI_TENANTS],
            admitted: 0,
            refused: 0,
        }
    }

    pub const fn capacity(&self) -> u32 {
        self.capacity
    }

    pub const fn budget_milli(&self) -> u32 {
        self.budget_milli
    }

    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    pub const fn admitted(&self) -> u32 {
        self.admitted
    }

    pub const fn refused(&self) -> u32 {
        self.refused
    }

    pub fn enable(&mut self, on: bool) {
        self.enabled = on;
    }

    pub fn set_budget_milli(&mut self, budget: u32) {
        self.budget_milli = budget.max(IS_SOLO_MILLI);
    }

    pub fn occupant(&self, tenant: TenantId) -> Option<NoiOccupant> {
        self.slots
            .iter()
            .copied()
            .flatten()
            .find(|o| o.tenant == tenant)
    }

    pub fn occupancy(&self) -> u32 {
        self.slots.iter().filter(|s| s.is_some()).count() as u32
    }

    pub fn demand_sum(&self) -> u32 {
        self.slots
            .iter()
            .copied()
            .flatten()
            .map(|o| o.demand)
            .fold(0u32, |a, d| a.saturating_add(d))
    }

    /// Current worst-case IS on the occupied mix (1000 if empty).
    pub fn worst_is_milli(&self) -> u32 {
        fabric_is_milli(self.demand_sum(), self.capacity)
    }

    /// Per-tenant IS estimate. `None` if the tenant is not occupying.
    pub fn tenant_is_milli(&self, tenant: TenantId) -> Option<u32> {
        let occ = self.occupant(tenant)?;
        let t = NoiTput::from_demand(occ.demand, self.demand_sum(), self.capacity);
        Some(t.is_milli)
    }

    /// Projected worst-case IS if `tenant` injects `demand`.
    ///
    /// Replaces the tenant's existing demand when already occupying.
    pub fn project_is_milli(&self, tenant: TenantId, demand: u32) -> Result<u32, NoiError> {
        if demand == 0 {
            return Err(NoiError::BadArg);
        }
        let sum = projected_sum(self, tenant, demand)?;
        Ok(fabric_is_milli(sum, self.capacity))
    }

    /// Solo vs concurrent throughput for a demand vector (no occupancy change).
    pub fn measure(&self, demands: &[u32]) -> Result<(u32, NoiTput), NoiError> {
        if demands.is_empty() || demands.len() > MAX_NOI_TENANTS {
            return Err(NoiError::BadArg);
        }
        let mut sum = 0u32;
        for d in demands {
            if *d == 0 {
                return Err(NoiError::BadArg);
            }
            sum = sum.saturating_add(*d);
        }
        let mut worst = IS_SOLO_MILLI;
        let mut first = NoiTput::from_demand(demands[0], sum, self.capacity);
        for (i, d) in demands.iter().enumerate() {
            let t = NoiTput::from_demand(*d, sum, self.capacity);
            if t.is_milli > worst {
                worst = t.is_milli;
            }
            if i == 0 {
                first = t;
            }
        }
        Ok((worst, first))
    }

    /// Admit `tenant` with `demand`. Refuse when projected IS > budget.
    ///
    /// Disabled NoI is a bypass (no occupancy). Not topology synthesis.
    pub fn admit(&mut self, tenant: TenantId, demand: u32) -> Result<IsEstimate, NoiError> {
        if !self.enabled {
            return Ok(IsEstimate {
                tenant,
                demand,
                is_milli: IS_SOLO_MILLI,
                worst_is_milli: IS_SOLO_MILLI,
            });
        }
        if demand == 0 {
            return Err(NoiError::BadArg);
        }
        let projected = match self.project_is_milli(tenant, demand) {
            Ok(p) => p,
            Err(e) => {
                self.refused = self.refused.saturating_add(1);
                return Err(e);
            }
        };
        if projected > self.budget_milli {
            self.refused = self.refused.saturating_add(1);
            return Err(NoiError::OverBudget);
        }
        self.place(tenant, demand)?;
        self.admitted = self.admitted.saturating_add(1);
        let is_milli = self.tenant_is_milli(tenant).unwrap_or(IS_SOLO_MILLI);
        Ok(IsEstimate {
            tenant,
            demand,
            is_milli,
            worst_is_milli: self.worst_is_milli(),
        })
    }

    pub fn release(&mut self, tenant: TenantId) -> Result<NoiOccupant, NoiError> {
        for slot in &mut self.slots {
            if let Some(occ) = slot {
                if occ.tenant == tenant {
                    let out = *occ;
                    *slot = None;
                    return Ok(out);
                }
            }
        }
        Err(NoiError::Unbound)
    }

    pub fn clear(&mut self) {
        self.slots = [None; MAX_NOI_TENANTS];
    }

    fn place(&mut self, tenant: TenantId, demand: u32) -> Result<(), NoiError> {
        for slot in &mut self.slots {
            if let Some(occ) = slot {
                if occ.tenant == tenant {
                    occ.demand = demand;
                    return Ok(());
                }
            }
        }
        for slot in &mut self.slots {
            if slot.is_none() {
                *slot = Some(NoiOccupant { tenant, demand });
                return Ok(());
            }
        }
        Err(NoiError::Exhausted)
    }
}

impl Default for SoftNoI {
    fn default() -> Self {
        Self::new()
    }
}

/// Solo throughput: injection, capped by the fake NoI.
pub const fn tput_solo(demand: u32, cap: u32) -> u32 {
    if demand < cap {
        demand
    } else {
        cap
    }
}

/// Concurrent throughput: under capacity, solo; over capacity, proportional share.
pub const fn tput_con(demand: u32, sum: u32, cap: u32) -> u32 {
    if demand == 0 {
        0
    } else if sum <= cap {
        tput_solo(demand, cap)
    } else {
        ((demand as u64 * cap as u64) / sum as u64) as u32
    }
}

/// `IS = T_solo / T_con` in milli. 1000 = 1.0×.
pub const fn is_milli(solo: u32, con: u32) -> u32 {
    if solo == 0 {
        IS_SOLO_MILLI
    } else if con == 0 {
        0
    } else {
        ((solo as u64 * 1000) / con as u64) as u32
    }
}

/// Worst-case IS of a mix: `max(1.0, sum / capacity)`.
pub const fn fabric_is_milli(sum: u32, cap: u32) -> u32 {
    if cap == 0 || sum <= cap {
        IS_SOLO_MILLI
    } else {
        ((sum as u64 * 1000) / cap as u64) as u32
    }
}

fn projected_sum(noi: &SoftNoI, tenant: TenantId, demand: u32) -> Result<u32, NoiError> {
    let mut sum = 0u32;
    let mut found = false;
    let mut used = 0u32;
    for slot in &noi.slots {
        if let Some(occ) = slot {
            used += 1;
            if occ.tenant == tenant {
                sum = sum.saturating_add(demand);
                found = true;
            } else {
                sum = sum.saturating_add(occ.demand);
            }
        }
    }
    if !found {
        if used as usize >= MAX_NOI_TENANTS {
            return Err(NoiError::Exhausted);
        }
        sum = sum.saturating_add(demand);
    }
    Ok(sum)
}

/// Host-identical clip. Kernel prints `[softnoi] …`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoftNoiReport {
    pub light_is_ok: bool,
    pub heavy_is_over: bool,
    pub light_admit: bool,
    pub heavy_refuse: bool,
    pub advertised: bool,
    pub not_synth: bool,
    pub light_is_milli: u32,
    pub heavy_is_milli: u32,
    pub admitted: u32,
    pub refused: u32,
}

impl SoftNoiReport {
    pub fn all_ok(&self) -> bool {
        self.light_is_ok
            && self.heavy_is_over
            && self.light_admit
            && self.heavy_refuse
            && self.advertised
            && self.not_synth
    }
}

/// Solo vs concurrent IS, then admit light / refuse heavy over 1.5.
#[inline(never)]
pub fn run_softnoi_demo() -> SoftNoiReport {
    let cap = NOI_CAPACITY;
    let a = TenantId(1);
    let b = TenantId(2);

    let mut noi = SoftNoI::new();
    noi.enable(true);

    let (light_is, light_t) = noi
        .measure(&[DEMO_LIGHT_DEMAND, DEMO_LIGHT_DEMAND])
        .unwrap_or((0, NoiTput::from_demand(0, 0, cap)));
    let light_is_ok = light_is <= IS_BUDGET_MILLI
        && light_is == IS_SOLO_MILLI
        && light_t.solo == DEMO_LIGHT_DEMAND
        && light_t.concurrent == DEMO_LIGHT_DEMAND;

    let (heavy_is, heavy_t) = noi
        .measure(&[DEMO_HEAVY_DEMAND, DEMO_HEAVY_DEMAND])
        .unwrap_or((0, NoiTput::from_demand(0, 0, cap)));
    let heavy_is_over = heavy_is > IS_BUDGET_MILLI
        && heavy_is == 1600
        && heavy_t.solo == DEMO_HEAVY_DEMAND
        && heavy_t.concurrent == 500;

    let ea = noi.admit(a, DEMO_LIGHT_DEMAND);
    let eb = noi.admit(b, DEMO_LIGHT_DEMAND);
    let light_admit = ea.is_ok()
        && eb.is_ok()
        && noi.occupancy() == 2
        && noi.worst_is_milli() == IS_SOLO_MILLI
        && noi.admitted() == 2
        && noi.refused() == 0;
    let advertised = noi.tenant_is_milli(a) == Some(IS_SOLO_MILLI)
        && noi.tenant_is_milli(b) == Some(IS_SOLO_MILLI)
        && ea.map(|e| e.worst_is_milli) == Ok(IS_SOLO_MILLI);

    noi.clear();
    let ha = noi.admit(a, DEMO_HEAVY_DEMAND);
    let hb = noi.admit(b, DEMO_HEAVY_DEMAND);
    let heavy_refuse = ha.is_ok()
        && hb == Err(NoiError::OverBudget)
        && noi.occupancy() == 1
        && noi.tenant_is_milli(a) == Some(IS_SOLO_MILLI)
        && noi.tenant_is_milli(b).is_none()
        && noi.project_is_milli(b, DEMO_HEAVY_DEMAND) == Ok(1600)
        && noi.refused() == 1
        && noi.admitted() == 3;

    // Honesty latch: budget is an admit threshold, not PARL's 1.2× synth target.
    let not_synth = noi.budget_milli() == IS_BUDGET_MILLI && noi.capacity() == NOI_CAPACITY;

    SoftNoiReport {
        light_is_ok,
        heavy_is_over,
        light_admit,
        heavy_refuse,
        advertised,
        not_synth,
        light_is_milli: light_is,
        heavy_is_milli: heavy_is,
        admitted: noi.admitted(),
        refused: noi.refused(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solo_equals_demand_under_capacity() {
        assert_eq!(
            tput_solo(DEMO_LIGHT_DEMAND, NOI_CAPACITY),
            DEMO_LIGHT_DEMAND
        );
        assert_eq!(
            tput_solo(DEMO_HEAVY_DEMAND, NOI_CAPACITY),
            DEMO_HEAVY_DEMAND
        );
        assert_eq!(tput_solo(NOI_CAPACITY, NOI_CAPACITY), NOI_CAPACITY);
        assert_eq!(tput_solo(NOI_CAPACITY + 1, NOI_CAPACITY), NOI_CAPACITY);
    }

    #[test]
    fn light_concurrent_is_one() {
        let n = SoftNoI::new();
        let (is, t) = n.measure(&[DEMO_LIGHT_DEMAND, DEMO_LIGHT_DEMAND]).unwrap();
        assert_eq!(is, IS_SOLO_MILLI);
        assert_eq!(t.solo, t.concurrent);
        assert_eq!(t.is_milli, IS_SOLO_MILLI);
        assert!(is <= IS_BUDGET_MILLI);
    }

    #[test]
    fn heavy_concurrent_is_over_budget() {
        let n = SoftNoI::new();
        let (is, t) = n.measure(&[DEMO_HEAVY_DEMAND, DEMO_HEAVY_DEMAND]).unwrap();
        assert_eq!(t.solo, 800);
        assert_eq!(t.concurrent, 500);
        assert_eq!(is_milli(800, 500), 1600);
        assert_eq!(is, 1600);
        assert!(is > IS_BUDGET_MILLI);
    }

    #[test]
    fn admit_light_two_tenants_refuse_heavy() {
        let r = run_softnoi_demo();
        assert!(r.light_is_ok, "light mix IS ≤ 1.5");
        assert!(r.heavy_is_over, "heavy mix IS > 1.5");
        assert!(r.light_admit, "A+B light admitted");
        assert!(r.heavy_refuse, "heavy B refused");
        assert!(r.advertised, "per-tenant IS estimate");
        assert!(r.not_synth, "admit control, not topology synth");
        assert_eq!(r.light_is_milli, 1000);
        assert_eq!(r.heavy_is_milli, 1600);
        assert_eq!(r.admitted, 3);
        assert_eq!(r.refused, 1);
        assert!(r.all_ok());
    }

    #[test]
    fn disabled_is_bypass() {
        let mut n = SoftNoI::new();
        assert!(!n.enabled());
        n.admit(TenantId(1), DEMO_HEAVY_DEMAND).unwrap();
        n.admit(TenantId(2), DEMO_HEAVY_DEMAND).unwrap();
        assert_eq!(n.occupancy(), 0);
        assert_eq!(n.admitted(), 0);
        assert_eq!(n.refused(), 0);
    }

    #[test]
    fn budget_equal_is_admitted() {
        let mut n = SoftNoI::new();
        n.enable(true);
        n.admit(TenantId(1), 800).unwrap();
        // 800+700 = 1500 → IS = 1.5, not greater.
        n.admit(TenantId(2), 700).unwrap();
        assert_eq!(n.worst_is_milli(), IS_BUDGET_MILLI);
        assert_eq!(n.occupancy(), 2);
    }

    #[test]
    fn third_tenant_exhausted() {
        let mut n = SoftNoI::new();
        n.enable(true);
        n.admit(TenantId(1), DEMO_LIGHT_DEMAND).unwrap();
        n.admit(TenantId(2), DEMO_LIGHT_DEMAND).unwrap();
        assert_eq!(
            n.admit(TenantId(3), DEMO_LIGHT_DEMAND).unwrap_err(),
            NoiError::Exhausted
        );
        assert_eq!(n.refused(), 1);
    }

    #[test]
    fn release_then_heavy_peer_admits() {
        let mut n = SoftNoI::new();
        n.enable(true);
        n.admit(TenantId(1), DEMO_HEAVY_DEMAND).unwrap();
        assert_eq!(
            n.admit(TenantId(2), DEMO_HEAVY_DEMAND).unwrap_err(),
            NoiError::OverBudget
        );
        n.release(TenantId(1)).unwrap();
        n.admit(TenantId(2), DEMO_HEAVY_DEMAND).unwrap();
        assert_eq!(n.occupancy(), 1);
        assert_eq!(n.tenant_is_milli(TenantId(2)), Some(IS_SOLO_MILLI));
    }

    #[test]
    fn not_parl_topology_or_unicnet() {
        // SoftNoI is an admit threshold on a fake shared NoI. PARL
        // synthesizes graphs; UniCNet is another interconnect story.
        let n = SoftNoI::new();
        assert_eq!(n.budget_milli(), 1500);
        assert_eq!(n.capacity(), 1000);
        assert_eq!(MAX_NOI_TENANTS, 2);
        assert!(!n.enabled());
    }
}
