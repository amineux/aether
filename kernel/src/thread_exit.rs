//! Shared-root-only retirement policy, used by the kernel and host regressions.
use aether_core::preempt::{CpuQueue, ThreadState};

/// Caller supplies trusted live-user roots. Refuse the last live thread of a
/// root: no cap-space, page-table or DMA teardown exists for that case yet.
pub fn retire_shared(
    queue: &mut CpuQueue,
    current: u32,
    root: u64,
    mut root_of: impl FnMut(u32) -> Option<u64>,
) -> Option<u32> {
    if root == 0 || queue.current != Some(current) || root_of(current) != Some(root) {
        return None;
    }
    let sibling = queue.slots.iter().any(|slot| {
        slot.id != current && slot.state != ThreadState::Empty && root_of(slot.id) == Some(root)
    });
    if !sibling {
        return None;
    }
    queue.finish_current()
}
