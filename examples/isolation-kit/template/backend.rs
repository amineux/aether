//! **Bring-your-own-backend template** for the isolation kit.
//!
//! Copy this file into your own crate (or edit it in place), rename
//! [`TemplateBackend`], and replace each `NotApplicable` arm in `probe` with a
//! call into your runtime that attempts the described attack and reports
//! whether your stack refused it. Until you do, every class scores `n/a` and
//! the summary line is `applicable=0 ... result=NONCONFORMANT`, because the kit
//! never counts an untested class as a pass.
//!
//! Rules of thumb for each arm:
//! - Return [`Outcome::Refused`] only if your stack returned an error and left
//!   the victim's state unchanged (no partial write, no quota charged).
//! - Return [`Outcome::Accepted`] if the attack went through. That is a real
//!   gap; the kit is meant to show it.
//! - Keep [`Outcome::NotApplicable`] (or return `false` from `applies`) only
//!   for a surface your hardware or runtime genuinely does not have.
//!
//! The `expected-error` column names the refusal Aether's reference adapter
//! returns; your stack can use its own error type. This template integrates
//! no runtime: it is a starting point, not a result.

use crate::{AttackClass, IsolationBackend, Outcome};

/// Starting point for a team's own adapter. Every class is unimplemented.
pub struct TemplateBackend;

impl IsolationBackend for TemplateBackend {
    fn name(&self) -> &str {
        "template-unimplemented"
    }

    fn probe(&self, class: &AttackClass) -> Outcome {
        match class.name {
            // command: submit work whose operator class differs from the class
            // the tenant's handle was bound to. Needs: a per-handle class check
            // before any work or quota is spent.
            "opkernel-class-mismatch" => Outcome::NotApplicable,
            // command: submit flow work with an empty per-tenant quota. Needs:
            // a per-tenant budget checked at admission, not after execution.
            "hodge-quota" => Outcome::NotApplicable,
            // accelerator: launch a job with a zero or oversized m/n/k (or an
            // arithmetic overflow). Needs: shape validation before the job
            // touches device memory.
            "accel-shape-overflow" => Outcome::NotApplicable,
            // accelerator: request a dtype the device or the buffer view does
            // not support. Needs: dtype admission against device capabilities.
            "accel-unsupported-dtype" => Outcome::NotApplicable,
            // smmu: ask the mapping path to pin a caller-chosen physical address
            // (kernel memory or another tenant's arena). Needs: physical
            // addresses derived only from the tenant's own allocation.
            "map-user-phys" => Outcome::NotApplicable,
            // smmu: map memory of a kind that must never be device-visible
            // (streaming / scratch space in Aether's model). Needs: a memory
            // kind check before the IOMMU entry is written.
            "space-not-mappable" => Outcome::NotApplicable,
            // smmu: map a window under a stream id that does not match the one
            // the window was created for. Needs: stream-id binding on windows.
            "typed-window-sid" => Outcome::NotApplicable,
            // smmu: map a zero-length or address-overflowing window. Needs:
            // range validation that rejects wraparound.
            "smmu-bad-range" => Outcome::NotApplicable,
            // fabric: flood another tenant's receive queue. Needs: a bounded
            // queue that refuses (not drops or overwrites) when full.
            "fabric-queue-full" => Outcome::NotApplicable,
            // fabric: receive from an endpoint owned by another tenant. Needs:
            // ownership check that looks identical to "no such endpoint".
            "fabric-recv-foreign" => Outcome::NotApplicable,
            // fabric: send without a capability or right to the target
            // endpoint. Needs: send-side authorization with no side effect.
            "fabric-send-no-cap" => Outcome::NotApplicable,
            // fabric: one sender spends its whole message budget. Needs:
            // per-sender quota so other tenants keep their full budgets.
            "fabric-quota-drain" => Outcome::NotApplicable,
            // arena: hand off or free a buffer the caller does not own. Needs:
            // owner check on every transfer and free.
            "arena-not-owner" => Outcome::NotApplicable,
            // arena: allocate past the allocation-table limit. Needs: limit
            // checked before any span is split, so refusals leak nothing.
            "arena-limit-leak" => Outcome::NotApplicable,
            // kv: attend to or pin a KV cache through a grant that lacks the
            // read / map right. Needs: rights checked per operation.
            "kv-insufficient-rights" => Outcome::NotApplicable,
            // A class added to the kit later shows up as n/a here until you
            // write an arm for it.
            _ => Outcome::NotApplicable,
        }
    }
}
