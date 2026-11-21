//! One-slot accelerator admission and once-only completion consumption.
use aether_core::sysnr::UserCompletion;

/// An absent result never calls output. Copy failure consumes the result and
/// is returned to its waiter, matching immediate SYS_ACCEL_WAIT semantics.
pub fn deliver_next<E>(
    pending: &mut Option<UserCompletion>,
    output: impl FnOnce(UserCompletion) -> Result<(), E>,
) -> Option<Result<(), E>> {
    pending.take().map(output)
}

/// The one-result pipeline can accept work only when neither execution nor an
/// unread completion occupies its capacity. The caller serializes this check,
/// device submission and result consumption (World uses its IRQ-safe lock).
/// None means busy: the submission callback did not run and no state changed.
pub fn submit_if_idle<E>(
    pending: &mut bool,
    completion: &Option<UserCompletion>,
    submit: impl FnOnce() -> Result<(), E>,
) -> Option<Result<(), E>> {
    if *pending || completion.is_some() {
        return None;
    }
    let result = submit();
    if result.is_ok() {
        *pending = true;
    }
    Some(result)
}
