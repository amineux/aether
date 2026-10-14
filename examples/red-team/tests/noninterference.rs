//! Two-world noninterference check: zero divergences on the real system,
//! determinism, and every negative control (a re-opened known hole) caught.

#[allow(dead_code)]
#[path = "../examples/noninterference/world.rs"]
mod world;

use world::{check, first_divergence, run_world, Control};

#[test]
fn attacker_world_is_byte_identical_for_a_and_b() {
    let s = check(48, 8, Control::None);
    assert_eq!(s.divergences, 0, "first divergence: {:?}", s.first);
    assert_eq!(s.seeds, 48);
    assert_eq!(s.ops, 48 * 21 * 8);
    // C really did things: some of its own ops and jobs were accepted.
    assert!(s.c.accepted > 0 && s.c.jobs_ok > 0, "{:?}", s.c);
    // C's endpoint churn ran, and some of it was accepted.
    assert_eq!(s.c.fabric_ops, s.ops);
    assert!(s.c.fabric_accepted > 0 && s.c.fabric_accepted < s.c.fabric_ops, "{:?}", s.c);
    assert_eq!(
        s.line(),
        format!("[noninterference] worlds=2 seeds=48 ops={} divergences=0", 48 * 21 * 8)
    );
}

#[test]
fn same_seed_same_world() {
    let (a, sa) = run_world(Some((7, 8, Control::None)));
    let (b, sb) = run_world(Some((7, 8, Control::None)));
    assert_eq!(first_divergence(&a, &b), None);
    assert_eq!(sa, sb);
}

#[test]
fn idle_world_outputs_are_the_honest_results() {
    let (obs, _) = run_world(None);
    // Every honest job and KV attend succeeded, and A's completions are
    // numbered from A's own queue.
    assert!(obs.iter().all(|o| !o.event.contains("Err")), "{obs:#?}");
    assert!(obs[1].event.contains("result=Ok(1)"), "{}", obs[1].event);
}

#[test]
fn negative_controls_are_caught() {
    for ctl in [
        Control::UncheckedUnmap,
        Control::RawMapAddr,
        Control::LeakedCap,
        Control::GlobalSeq,
        Control::UncheckedClose,
        Control::UncheckedRecv,
        Control::ForgedSenderQuota,
    ] {
        let s = check(64, 16, ctl);
        assert!(s.divergences > 0, "control {} not caught", ctl.name());
    }
}

#[test]
fn honest_fabric_traffic_round_trips_in_the_idle_world() {
    let (obs, _) = run_world(None);
    assert!(obs.iter().any(|o| o.event.contains("fabric_send=Ok(())")), "{obs:#?}");
    assert!(obs.iter().any(|o| o.event.contains("fabric_recv=Ok(badge=")), "{obs:#?}");
}
