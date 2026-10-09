//! Two-world noninterference check (host). See `world.rs`.
//!
//! Run: `make noninterference` or
//! `cargo run --release -p aether-redteam --example noninterference`.
//! Optional args: `SEEDS OPS_PER_GAP` (defaults 512 and 16).

mod world;

use world::{check, Control};

fn main() {
    let mut args = std::env::args().skip(1);
    let seeds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(512);
    let per_gap: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(16);

    let s = check(seeds, per_gap, Control::None);
    println!(
        "[noninterference] tenant=C ops={} accepted={} refused={} own_jobs_ok={}",
        s.c.ops, s.c.accepted, s.c.refused, s.c.jobs_ok
    );
    println!(
        "[noninterference] tenant=C fabric_ops={} fabric_accepted={}",
        s.c.fabric_ops, s.c.fabric_accepted
    );
    if let Some((seed, step, w0, w1)) = &s.first {
        println!("[noninterference] first-divergence seed={seed} step={step} world0=\"{w0}\" world1=\"{w1}\"");
    }
    // Negative controls: each re-opens one known hole and must diverge.
    let mut controls_ok = true;
    for ctl in [
        Control::UncheckedUnmap,
        Control::RawMapAddr,
        Control::LeakedCap,
        Control::GlobalSeq,
        Control::UncheckedClose,
        Control::UncheckedRecv,
        Control::ForgedSenderQuota,
    ] {
        let c = check(seeds.min(64), per_gap, ctl);
        let caught = c.divergences > 0;
        controls_ok &= caught;
        println!(
            "[noninterference] control={} seeds={} divergences={} caught={}",
            ctl.name(),
            c.seeds,
            c.divergences,
            caught
        );
    }
    println!("{}", world::SCOPE_LINE);
    println!("{}", s.line());
    if s.divergences != 0 || !controls_ok {
        std::process::exit(1);
    }
}
