//! Test the same completion consumer used by immediate and blocked kernel waits.
#[path = "../../kernel/src/completion_delivery.rs"]
mod completion_delivery;
use aether_core::sysnr::UserCompletion;
use completion_delivery::deliver_next;

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
