//! Complete one receive from the checked FIFO, never from a sender-side copy.
use aether_core::fabric::{EndpointId, Fabric, FabricError, Message};
use aether_core::types::TenantId;

/// No queued message means no output callback. A copy failure is returned to
/// the receiver, not converted to success; as with immediate recv, it consumes
/// the dequeued message. Callers wake only the receiver passed to the callback.
pub fn deliver_next<E>(
    fabric: &mut Fabric,
    owner: TenantId,
    endpoint: EndpointId,
    output: impl FnOnce(&Message) -> Result<(), E>,
) -> Result<Result<(), E>, FabricError> {
    let message = fabric.recv_as(owner, endpoint)?;
    Ok(output(&message))
}
