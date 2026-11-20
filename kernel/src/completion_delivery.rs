//! Consume the one-slot accelerator completion exactly once on either wait path.
use aether_core::sysnr::UserCompletion;

/// An absent result never calls output. Copy failure consumes the result and
/// is returned to its waiter, matching immediate SYS_ACCEL_WAIT semantics.
pub fn deliver_next<E>(
    pending: &mut Option<UserCompletion>,
    output: impl FnOnce(UserCompletion) -> Result<(), E>,
) -> Option<Result<(), E>> {
    pending.take().map(output)
}
