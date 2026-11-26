//! Checked endpoint teardown, shared by World and host regressions.
use crate::cap_spaces::CapSpaces;
use aether_core::caps::{CPtr, CapKind, CapRights};
use aether_core::fabric::{EndpointId, Fabric};

/// Caller serializes this transaction with all table/fabric operations.
/// Preflight ownership before the bounded cap walk; a refused close changes
/// neither subsystem. Fabric may reuse storage slots but allocates fresh IDs.
pub fn close(
    spaces: &mut CapSpaces,
    fabric: &mut Fabric,
    root: u64,
    source: CPtr,
) -> Result<EndpointId, ()> {
    let caps = spaces.get(root).ok_or(())?;
    let cap = caps
        .require(source, CapKind::Endpoint, CapRights::GRANT)
        .map_err(|_| ())?;
    let endpoint = EndpointId(cap.object);
    let owner = caps.owner();
    if fabric.owner(endpoint).map_err(|_| ())? != owner {
        return Err(());
    }
    spaces.remove_endpoint(root, source).map_err(|_| ())?;
    // No fabric mutation occurred since the owner preflight. This cannot fail
    // while World's interrupt-disabled lock serializes endpoint operations.
    fabric
        .close_for(owner, endpoint)
        .expect("preflighted endpoint owner");
    Ok(endpoint)
}

/// Even an independent READ alias cannot keep a waiter on a closed object alive.
pub fn receiver_allowed(spaces: &CapSpaces, closed: EndpointId, root: u64, endpoint: u32) -> bool {
    endpoint != closed.0 && spaces.can_receive(root, endpoint)
}
