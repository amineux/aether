//! Run the kernel smoke's real selection policy under adversarial CPU orders.
#[path = "../../kernel/src/smp_smoke.rs"]
mod smp_smoke;
use aether_core::sched::TileScheduler;
use smp_smoke::{prepare, take, JOBS};

#[test]
fn ap_first_cannot_consume_the_bsp_hinted_job() {
    let mut s = TileScheduler::new();
    prepare(&mut s);
    let mut stolen = Vec::new();
    while let Some((job, stole)) = take(&mut s, 1) {
        assert!(stole);
        assert_ne!(job.id, 1);
        stolen.push(job.id);
    }
    assert_eq!(stolen.len(), (JOBS - 1) as usize);
    // Retrying an empty steal must still leave the BSP job in the queue.
    assert!(take(&mut s, 1).is_none());
    let (bsp, stole) = take(&mut s, 0).expect("BSP retains its job");
    assert_eq!(bsp.id, 1);
    assert!(!stole);
    assert!(take(&mut s, 0).is_none());
}

#[test]
fn bsp_first_then_ap_still_proves_both_cpus_and_a_steal() {
    let mut s = TileScheduler::new();
    prepare(&mut s);
    let (bsp, stole) = take(&mut s, 0).unwrap();
    assert_eq!(bsp.id, 1);
    assert!(!stole);
    let mut completed = vec![bsp.id];
    while let Some((job, stole)) = take(&mut s, 1) {
        assert!(stole);
        completed.push(job.id);
    }
    completed.sort_unstable();
    assert_eq!(completed, (1..=JOBS).collect::<Vec<_>>());
}

#[test]
fn every_interleaving_consumes_each_job_once_and_leaves_work_for_bsp() {
    // Enumerate CPU orders for the first eight selection attempts, then drain.
    for order in 0..(1u32 << JOBS) {
        let mut s = TileScheduler::new();
        prepare(&mut s);
        let mut completed = Vec::new();
        let mut bsp_jobs = 0;
        for cpu in (0..JOBS).map(|bit| (order >> bit) & 1).chain([0, 1, 0]) {
            if let Some((job, stole)) = take(&mut s, cpu) {
                if cpu == 0 {
                    bsp_jobs += 1;
                } else {
                    assert!(stole);
                    assert_ne!(job.id, 1);
                }
                completed.push(job.id);
            }
        }
        assert!(bsp_jobs > 0, "order {order}");
        completed.sort_unstable();
        assert_eq!(completed, (1..=JOBS).collect::<Vec<_>>(), "order {order}");
        assert!(take(&mut s, 0).is_none());
        assert!(take(&mut s, 1).is_none());
    }
}
