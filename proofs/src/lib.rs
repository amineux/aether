//! Bounded model-checked properties over Aether's pure core (Kani).
//!
//! Every harness is behind `#[cfg(kani)]`, so a normal `cargo build` compiles
//! this crate to an empty library and leaves every other build unchanged. Run
//! the proofs with `make kani` or
//! `cargo kani --manifest-path proofs/Cargo.toml`.
//!
//! These are **bounded model-checked** properties: each harness documents the
//! input bounds it checks with Kani / CBMC. This is not a whole-system proof
//! and not a hardware guarantee. `docs/DILIGENCE.md` states the bounds,
//! assumptions and scope in full. Never described as "formally verified" or
//! "zero-trust".
#![cfg_attr(kani, allow(clippy::all))]
#![allow(unexpected_cfgs)]

#[cfg(kani)]
mod arena_proofs;
#[cfg(kani)]
mod caps_proofs;
#[cfg(kani)]
mod kv_proofs;
#[cfg(kani)]
mod smmu_proofs;
#[cfg(kani)]
mod sysnr_proofs;
