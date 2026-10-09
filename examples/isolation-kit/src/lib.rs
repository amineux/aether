//! Pre-silicon tenant-isolation **conformance kit** (use case B).
//!
//! A chip team describes its backend — command/stream format, accelerator,
//! SMMU, fabric — by implementing the small [`IsolationBackend`] trait. The
//! kit runs Aether's existing named attack classes against that backend and
//! produces a per-backend matrix of **attack class → refused / accepted /
//! not-applicable**, plus a one-line summary.
//!
//! The reference adapter ([`AetherSoftBackend`]) wires each class to the
//! library demo that already exercises Aether's Soft\* refuse path, so the kit
//! adds **no** new isolation mechanism of its own — it only reports what the
//! existing code does. A deliberately weak sample adapter
//! ([`WeakSampleBackend`], example-only) leaves classes open so the matrix
//! shows real accepts; that is how the kit proves it can fail.
//!
//! What this is **not**: not certification, not a partner or customer result,
//! not a hardware-isolation guarantee, and not a performance claim. Every
//! outcome is a host software check.

/// Which part of a backend an attack class exercises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Aspect {
    /// Command / stream program validation.
    Command,
    /// Accelerator job shape / dtype admission.
    Accelerator,
    /// Address / stream mapping (Soft SMMU surface).
    Smmu,
    /// On-package fabric / queue admission.
    Fabric,
    /// Tensor arena ownership and limits.
    Arena,
    /// KV grant rights.
    Kv,
}

impl Aspect {
    pub fn label(self) -> &'static str {
        match self {
            Aspect::Command => "command",
            Aspect::Accelerator => "accelerator",
            Aspect::Smmu => "smmu",
            Aspect::Fabric => "fabric",
            Aspect::Arena => "arena",
            Aspect::Kv => "kv",
        }
    }
}

/// One named attack class. `name` matches the `[redteam] attack=<name>` needle
/// already emitted by the host red-team clip; `expected_error` names the
/// refusal a conformant backend returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttackClass {
    pub name: &'static str,
    pub aspect: Aspect,
    pub expected_error: &'static str,
}

/// The attack classes the kit runs. These reuse existing, named refuse paths;
/// the kit does not invent new ones.
pub const CLASSES: &[AttackClass] = &[
    AttackClass { name: "opkernel-class-mismatch", aspect: Aspect::Command, expected_error: "OpKernelError::ClassMismatch" },
    AttackClass { name: "hodge-quota", aspect: Aspect::Command, expected_error: "HodgeError::QuotaExceeded" },
    AttackClass { name: "accel-shape-overflow", aspect: Aspect::Accelerator, expected_error: "AccelError::BadShape" },
    AttackClass { name: "accel-unsupported-dtype", aspect: Aspect::Accelerator, expected_error: "AccelError::UnsupportedDType" },
    AttackClass { name: "map-user-phys", aspect: Aspect::Smmu, expected_error: "SysError::Inval" },
    AttackClass { name: "space-not-mappable", aspect: Aspect::Smmu, expected_error: "SpaceError::NotMappable" },
    AttackClass { name: "typed-window-sid", aspect: Aspect::Smmu, expected_error: "MapError::WrongStream" },
    AttackClass { name: "smmu-bad-range", aspect: Aspect::Smmu, expected_error: "MapError::BadRange" },
    AttackClass { name: "fabric-queue-full", aspect: Aspect::Fabric, expected_error: "FabricError::QueueFull" },
    AttackClass { name: "fabric-recv-foreign", aspect: Aspect::Fabric, expected_error: "FabricError::NoSuchEndpoint" },
    AttackClass { name: "fabric-send-no-cap", aspect: Aspect::Fabric, expected_error: "FabricError::NoSuchEndpoint" },
    AttackClass { name: "fabric-quota-drain", aspect: Aspect::Fabric, expected_error: "HodgeError::QuotaExceeded" },
    AttackClass { name: "arena-not-owner", aspect: Aspect::Arena, expected_error: "ArenaError::NotOwner" },
    AttackClass { name: "arena-limit-leak", aspect: Aspect::Arena, expected_error: "ArenaError::ArenaLimit" },
    AttackClass { name: "kv-insufficient-rights", aspect: Aspect::Kv, expected_error: "KvError::InsufficientRights" },
];

/// Outcome of running one class against one backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The backend refused the attack (a conformant result).
    Refused,
    /// The backend allowed the attack (a real gap).
    Accepted,
    /// The class does not apply to this backend's feature set.
    NotApplicable,
}

impl Outcome {
    pub fn cell(self) -> &'static str {
        match self {
            Outcome::Refused => "refused",
            Outcome::Accepted => "ACCEPTED",
            Outcome::NotApplicable => "n/a",
        }
    }
}

/// A backend a chip team brings to the kit. Keep the surface tiny: name the
/// backend, say which classes apply to it, and report what it does per class.
pub trait IsolationBackend {
    /// Stable identifier printed in the matrix and summary line.
    fn name(&self) -> &str;

    /// Does this class apply to the backend's feature set? Default: all apply.
    /// Return `false` for a surface the backend does not have (e.g. no KV
    /// fabric) so the class is scored not-applicable rather than a gap.
    fn applies(&self, class: &AttackClass) -> bool {
        let _ = class;
        true
    }

    /// Run `class` against the backend and report whether it was refused.
    /// Only called when [`applies`](Self::applies) is true.
    fn probe(&self, class: &AttackClass) -> Outcome;
}

/// One scored class for one backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub class: AttackClass,
    pub outcome: Outcome,
}

/// A full per-backend matrix over [`CLASSES`].
#[derive(Clone, Debug)]
pub struct Matrix {
    pub backend: String,
    pub rows: Vec<Row>,
}

impl Matrix {
    /// Run every class in [`CLASSES`] against `backend`.
    pub fn run(backend: &dyn IsolationBackend) -> Matrix {
        let rows = CLASSES
            .iter()
            .map(|class| {
                let outcome = if backend.applies(class) {
                    backend.probe(class)
                } else {
                    Outcome::NotApplicable
                };
                Row { class: *class, outcome }
            })
            .collect();
        Matrix { backend: backend.name().to_string(), rows }
    }

    pub fn total(&self) -> usize {
        self.rows.len()
    }

    pub fn count(&self, o: Outcome) -> usize {
        self.rows.iter().filter(|r| r.outcome == o).count()
    }

    pub fn applicable(&self) -> usize {
        self.total() - self.count(Outcome::NotApplicable)
    }

    /// Conformant iff no applicable class was accepted. An all-not-applicable
    /// backend is not conformant (nothing was actually tested).
    pub fn conformant(&self) -> bool {
        self.count(Outcome::Accepted) == 0 && self.applicable() > 0
    }

    /// Grep-able one-line summary. Stable field order for CI and for later
    /// ingestion by an evidence bundle.
    pub fn summary_line(&self) -> String {
        format!(
            "[isolation-kit] backend={} classes={} applicable={} refused={} accepted={} n/a={} result={}",
            self.backend,
            self.total(),
            self.applicable(),
            self.count(Outcome::Refused),
            self.count(Outcome::Accepted),
            self.count(Outcome::NotApplicable),
            if self.conformant() { "conformant" } else { "NONCONFORMANT" },
        )
    }

    /// Human-readable matrix block (aspect, class, expected error, outcome).
    pub fn table(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("[isolation-kit] matrix backend={}\n", self.backend));
        out.push_str(&format!(
            "  {:<12} {:<24} {:<28} {}\n",
            "aspect", "attack-class", "expected-error", "outcome"
        ));
        for r in &self.rows {
            out.push_str(&format!(
                "  {:<12} {:<24} {:<28} {}\n",
                r.class.aspect.label(),
                r.class.name,
                r.class.expected_error,
                r.outcome.cell(),
            ));
        }
        out
    }
}

/// Reference adapter: Aether's own Soft\* backends. Each class calls the
/// existing library demo that drives the refuse path; `all_ok()` true means
/// the refusal held. This adapter should refuse every applicable class.
pub struct AetherSoftBackend;

impl IsolationBackend for AetherSoftBackend {
    fn name(&self) -> &str {
        "aether-soft"
    }

    fn probe(&self, class: &AttackClass) -> Outcome {
        use aether_core::{accel, arena, fabric, hodge, kvfabric, opkernel, space, sysnr, window};
        let refused = match class.name {
            "opkernel-class-mismatch" => opkernel::run_opkernel_class_mismatch_demo().all_ok(),
            "hodge-quota" => hodge::run_hodge_quota_demo().all_ok(),
            "accel-shape-overflow" => accel::run_accel_shape_overflow_demo().all_ok(),
            "accel-unsupported-dtype" => accel::run_accel_unsupported_dtype_demo().all_ok(),
            "map-user-phys" => sysnr::run_map_user_phys_demo().all_ok(),
            "space-not-mappable" => space::run_space_not_mappable_demo().all_ok(),
            "typed-window-sid" => window::run_typed_window_sid_demo().all_ok(),
            "smmu-bad-range" => window::run_smmu_bad_range_demo().all_ok(),
            "fabric-queue-full" => fabric::run_fabric_queue_full_demo().all_ok(),
            "fabric-recv-foreign" => fabric::run_fabric_recv_foreign_demo().all_ok(),
            "fabric-send-no-cap" => fabric::run_fabric_send_no_cap_demo().all_ok(),
            "fabric-quota-drain" => fabric::run_fabric_quota_drain_demo().all_ok(),
            "arena-not-owner" => arena::run_arena_not_owner_demo().all_ok(),
            "arena-limit-leak" => arena::run_arena_limit_leak_demo().all_ok(),
            "kv-insufficient-rights" => kvfabric::run_kv_insufficient_rights_demo().all_ok(),
            // An unknown class would be a kit bug, not a backend accept.
            other => panic!("reference adapter missing class: {other}"),
        };
        if refused {
            Outcome::Refused
        } else {
            Outcome::Accepted
        }
    }
}

/// **EXAMPLE ONLY** — a deliberately weak sample backend. It is not a real
/// Aether backend and not any third party; it is a teaching stub that models a
/// naive command processor which checks arena ownership and job shapes but
/// trusts caller-supplied addresses and streams and never bounds the fabric.
/// Its matrix therefore shows real accepts, proving the kit can fail.
pub struct WeakSampleBackend;

impl IsolationBackend for WeakSampleBackend {
    fn name(&self) -> &str {
        "weak-sample-example-only"
    }

    fn applies(&self, class: &AttackClass) -> bool {
        // This toy backend has no KV fabric at all, so that class is genuinely
        // not-applicable rather than a gap.
        class.aspect != Aspect::Kv
    }

    fn probe(&self, class: &AttackClass) -> Outcome {
        // Only enforces arena ownership/limits and accelerator job shapes.
        // Everything else (SMMU address pinning, stream ids, fabric flooding,
        // command class) is left open — exactly the gaps the kit should surface.
        let refused = matches!(
            class.name,
            "arena-not-owner" | "arena-limit-leak" | "accel-shape-overflow" | "accel-unsupported-dtype"
        );
        if refused {
            Outcome::Refused
        } else {
            Outcome::Accepted
        }
    }
}

/// Build the shipped adapters: the reference backend and the example-only weak
/// one.
pub fn shipped_backends() -> Vec<Box<dyn IsolationBackend>> {
    vec![Box::new(AetherSoftBackend), Box::new(WeakSampleBackend)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_refuses_every_applicable_class() {
        let m = Matrix::run(&AetherSoftBackend);
        assert_eq!(m.total(), CLASSES.len());
        assert_eq!(m.count(Outcome::Accepted), 0, "reference accepted a class: {}", m.table());
        assert_eq!(m.count(Outcome::NotApplicable), 0);
        assert!(m.conformant());
        assert!(m.summary_line().contains("result=conformant"));
        assert!(m.summary_line().contains("accepted=0"));
    }

    #[test]
    fn weak_sample_shows_real_accepts() {
        let m = Matrix::run(&WeakSampleBackend);
        assert!(m.count(Outcome::Accepted) > 0, "weak sample must show accepts");
        assert!(m.count(Outcome::NotApplicable) > 0, "weak sample has an n/a class");
        assert!(!m.conformant());
        assert!(m.summary_line().contains("result=NONCONFORMANT"));
    }

    #[test]
    fn every_class_name_is_unique_and_nonempty() {
        let mut seen = std::collections::BTreeSet::new();
        for c in CLASSES {
            assert!(!c.name.is_empty());
            assert!(!c.expected_error.is_empty());
            assert!(seen.insert(c.name), "duplicate class {}", c.name);
        }
    }

    #[test]
    fn matrix_runs_over_a_custom_backend() {
        struct AllNa;
        impl IsolationBackend for AllNa {
            fn name(&self) -> &str { "all-na" }
            fn applies(&self, _c: &AttackClass) -> bool { false }
            fn probe(&self, _c: &AttackClass) -> Outcome { Outcome::Refused }
        }
        let m = Matrix::run(&AllNa);
        assert_eq!(m.applicable(), 0);
        assert!(!m.conformant(), "all-not-applicable is not conformant");
    }
}
