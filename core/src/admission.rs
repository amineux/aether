//! Atomic admission of a workload's DMA buffers to the software SMMU.
//!
//! A failed batch leaves the entire table unchanged, including stream bindings,
//! page tables, address allocation, ATC state and the submit SID. Staging is a
//! bounded copy of [`IommuMap`], with no allocation or undo log. This API needs
//! exclusive access to the table; it is not a hardware transaction. Capabilities
//! must come from the caller's authorized capability lookup, as for `map`.

use crate::caps::Capability;
use crate::iommu::{IommuMap, MapError, MapRequest, MappedRegion, MAX_MAPS};

/// One buffer and the capability authorizing its pin.
#[derive(Clone, Copy, Debug)]
pub struct MappingRequest<'a> {
    pub cap: &'a Capability,
    pub mapping: MapRequest,
}

/// Identifies the first failed request; no successful prefix is published.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MappingFailure {
    pub request_index: usize,
    pub cause: MapError,
}

/// Committed regions in request order. Unused slots are not exposed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MappingBatch {
    regions: [Option<MappedRegion>; MAX_MAPS],
    len: usize,
}

impl MappingBatch {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, index: usize) -> Option<&MappedRegion> {
        self.regions.get(index).and_then(Option::as_ref)
    }

    pub fn iter(&self) -> impl Iterator<Item = &MappedRegion> {
        self.regions[..self.len].iter().flatten()
    }
}

impl IommuMap {
    /// Pin all buffers, publishing the staged table only after every request
    /// passes the existing `map` checks. Returns regions in request order.
    ///
    /// Empty batches are a no-op. More than `MAX_MAPS` requests are rejected
    /// before staging. Other failures identify the offending request. No new
    /// wire format, syscall or hardware driver is introduced. This copies a
    /// full software table; account for that stack/CPU cost before integrating
    /// it into a kernel hot path. Existing single-buffer APIs are unchanged.
    pub fn map_batch(
        &mut self,
        requests: &[MappingRequest<'_>],
    ) -> Result<MappingBatch, MappingFailure> {
        if requests.len() > MAX_MAPS {
            return Err(MappingFailure {
                request_index: MAX_MAPS,
                cause: MapError::TableFull,
            });
        }
        let mut result = MappingBatch {
            regions: [None; MAX_MAPS],
            len: requests.len(),
        };
        if requests.is_empty() {
            return Ok(result);
        }
        let mut staged = self.clone();
        for (index, request) in requests.iter().enumerate() {
            result.regions[index] = Some(staged.map(request.cap, request.mapping).map_err(
                |cause| MappingFailure {
                    request_index: index,
                    cause,
                },
            )?);
        }
        *self = staged;
        Ok(result)
    }
}
