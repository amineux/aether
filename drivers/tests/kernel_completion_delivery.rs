//! Test the same completion consumer used by immediate and blocked kernel waits.
#[path = "../../kernel/src/completion_delivery.rs"]
mod completion_delivery;
use aether_core::sysnr::UserCompletion;
use completion_delivery::{deliver_next, submit_if_idle};

fn completion(sequence: u32) -> UserCompletion {
    UserCompletion {
        job_seq: sequence,
        status: 0,
        cycles: 64,
    }
}

#[test]
fn completed_result_is_consumed_once_and_never_replayed_to_a_later_waiter() {
    let expected = completion(7);
    let mut slot = Some(expected);
    assert_eq!(
        deliver_next(&mut slot, |got| {
            assert_eq!(got, expected);
            Ok::<_, ()>(())
        }),
        Some(Ok(()))
    );
    assert_eq!(slot, None);
    let mut called = false;
    assert_eq!(
        deliver_next(&mut slot, |_| {
            called = true;
            Ok::<_, ()>(())
        }),
        None
    );
    assert!(!called);
}

#[test]
fn copy_failure_is_preserved_and_does_not_leave_a_cached_duplicate() {
    let mut slot = Some(completion(7));
    assert_eq!(deliver_next(&mut slot, |_| Err(-3)), Some(Err(-3)));
    assert_eq!(slot, None);
    slot = Some(completion(8));
    assert_eq!(
        deliver_next(&mut slot, |got| {
            assert_eq!(got.job_seq, 8);
            Ok::<_, i32>(())
        }),
        Some(Ok(()))
    );
}

#[test]
fn empty_wait_does_not_run_output_and_late_wait_gets_the_full_record() {
    let mut slot = None;
    assert_eq!(
        deliver_next(&mut slot, |_| -> Result<(), ()> { panic!("no result") }),
        None
    );
    // A result remains cached until an actual waiter consumes it, with status
    // and cycles unchanged even when the device reports an unsuccessful job.
    let expected = UserCompletion {
        job_seq: 9,
        status: -7,
        cycles: 123,
    };
    slot = Some(expected);
    assert_eq!(
        deliver_next(&mut slot, |got| {
            assert_eq!(got, expected);
            Ok::<_, ()>(())
        }),
        Some(Ok(()))
    );
    assert_eq!(slot, None);
}

#[test]
fn busy_pipeline_never_runs_submission_or_changes_its_result() {
    for (running, cached) in [
        (true, None),
        (false, Some(completion(7))),
        (true, Some(completion(7))),
    ] {
        let mut pending = running;
        let mut slot = cached;
        for _ in 0..2 {
            assert_eq!(
                submit_if_idle(&mut pending, &slot, || -> Result<(), ()> {
                    panic!("busy callback must not allocate a fence or submit to device")
                }),
                None
            );
            assert_eq!(pending, running);
            assert_eq!(slot, cached);
        }
        if let Some(expected) = cached {
            assert_eq!(
                deliver_next(&mut slot, |got| {
                    assert_eq!(got, expected);
                    Ok::<_, ()>(())
                }),
                Some(Ok(()))
            );
        }
    }
}

#[test]
fn capacity_is_reusable_only_after_execution_and_completion_consumption() {
    let mut pending = false;
    let mut slot = None;
    let mut submissions = 0;
    assert_eq!(
        submit_if_idle(&mut pending, &slot, || {
            submissions += 1;
            Ok::<_, ()>(())
        }),
        Some(Ok(()))
    );
    assert!(pending);
    assert_eq!(
        submit_if_idle(&mut pending, &slot, || {
            submissions += 1;
            Ok::<_, ()>(())
        }),
        None
    );
    // Device execution finishes, but the unread result still owns capacity.
    pending = false;
    slot = Some(completion(7));
    assert_eq!(
        submit_if_idle(&mut pending, &slot, || {
            submissions += 1;
            Ok::<_, ()>(())
        }),
        None
    );
    assert_eq!(submissions, 1);
    assert_eq!(deliver_next(&mut slot, |_| Ok::<_, ()>(())), Some(Ok(())));
    assert_eq!(
        submit_if_idle(&mut pending, &slot, || {
            submissions += 1;
            Ok::<_, ()>(())
        }),
        Some(Ok(()))
    );
    assert_eq!(submissions, 2);
    assert!(pending);
}

#[test]
fn failed_idle_submission_leaves_capacity_for_retry() {
    let mut pending = false;
    let slot = None;
    assert_eq!(
        submit_if_idle(&mut pending, &slot, || Err(-4)),
        Some(Err(-4))
    );
    assert!(!pending);
    assert_eq!(slot, None);
    assert_eq!(
        submit_if_idle(&mut pending, &slot, || Ok::<_, i32>(())),
        Some(Ok(()))
    );
    assert!(pending);
}

#[test]
fn copy_fault_consumption_also_releases_submission_capacity() {
    let mut pending = false;
    let mut slot = Some(completion(7));
    assert_eq!(
        submit_if_idle(&mut pending, &slot, || Ok::<_, i32>(())),
        None
    );
    assert_eq!(deliver_next(&mut slot, |_| Err(-3)), Some(Err(-3)));
    assert_eq!(
        submit_if_idle(&mut pending, &slot, || Ok::<_, i32>(())),
        Some(Ok(()))
    );
    assert!(pending);
}
