use aether_core::admission::{MappingFailure, MappingRequest};
use aether_core::caps::{CapKind, CapRights, Capability};
use aether_core::iommu::{IommuMap, MapError, MapRequest, StreamId, MAX_MAPS};
use aether_core::{PhysAddr, TenantId};

fn cap(tenant: u32) -> Capability {
    Capability::new(
        CapKind::Memory,
        CapRights::MEM_FULL,
        tenant,
        TenantId(tenant),
    )
}

fn request(cap: &Capability, sid: u32, pa: u64) -> MappingRequest<'_> {
    MappingRequest {
        cap,
        mapping: MapRequest::pin_stream(PhysAddr(pa), 4096, sid),
    }
}

// Debug includes every field, including private CD/mm state, ATC counters and
// submit SID omitted from the public observational dump. Compare before any
// translation call can mutate the ATC.
fn snapshot(table: &IommuMap) -> String {
    format!("{table:?}")
}

#[test]
fn admits_complete_pipeline_in_request_order() {
    let a = cap(1);
    let b = cap(2);
    let mut table = IommuMap::new();
    let requests = [
        request(&a, 0x100, 0x1000),
        request(&a, 0x100, 0x4000),
        request(&b, 0x200, 0x9000),
    ];
    let batch = table.map_batch(&requests).unwrap();
    assert_eq!(batch.len(), 3);
    assert!(!batch.is_empty());
    assert_eq!(batch.iter().count(), 3);
    assert_eq!(batch.get(3), None);
    assert_eq!(batch.get(usize::MAX), None);
    for (region, req) in batch.iter().zip(requests) {
        assert_eq!(region.guest_pa, req.mapping.guest_pa);
        assert_eq!(region.tenant, req.cap.tenant);
        assert_eq!(region.object, req.cap.object);
        assert_ne!(region.iova, region.guest_pa);
        assert_eq!(
            table.resolve_stream(region.stream_id, region.iova),
            Some(region.guest_pa)
        );
    }
}

#[test]
fn every_failed_prefix_rolls_back_all_fields() {
    let a = cap(1);
    for index in 0..4 {
        let mut table = IommuMap::new();
        let mut requests = [
            request(&a, 0x100, 0x1000),
            request(&a, 0x100, 0x3000),
            request(&a, 0x200, 0x5000),
            request(&a, 0x200, 0x7000),
        ];
        requests[index].mapping.len = 0;
        let before = snapshot(&table);
        assert_eq!(
            table.map_batch(&requests),
            Err(MappingFailure {
                request_index: index,
                cause: MapError::BadRange
            })
        );
        assert_eq!(snapshot(&table), before);
        requests[index].mapping.len = 4096;
        let retried = table.map_batch(&requests).unwrap();
        let mut fresh = IommuMap::new();
        assert_eq!(retried, fresh.map_batch(&requests).unwrap());
        assert_eq!(snapshot(&table), snapshot(&fresh));
    }
}

#[test]
fn foreign_stream_failure_preserves_live_dma_and_warm_atc() {
    let a = cap(1);
    let b = cap(2);
    let mut table = IommuMap::new();
    let live = table.map(&a, request(&a, 0x100, 0x1000).mapping).unwrap();
    table.set_sid(&a, StreamId::from_raw(0x100)).unwrap();
    table.resolve_ats(live.stream_id, live.iova).unwrap();
    table.resolve_ats(live.stream_id, live.iova).unwrap();
    assert!(table.atc_hits() > 0);
    let before = snapshot(&table);
    assert_eq!(
        table.map_batch(&[request(&b, 0x200, 0x9000), request(&b, 0x100, 0xb000)]),
        Err(MappingFailure {
            request_index: 1,
            cause: MapError::CrossTenant
        })
    );
    assert_eq!(snapshot(&table), before);
    assert_eq!(
        table.resolve_submit(live.stream_id, live.iova, Some(a.tenant)),
        Ok(live.guest_pa)
    );
}

#[test]
fn invalid_capability_after_valid_buffer_cannot_publish_prefix() {
    let a = cap(1);
    for bad in [
        Capability::new(CapKind::Endpoint, CapRights::MEM_FULL, 2, TenantId(1)),
        Capability::new(CapKind::Memory, CapRights(CapRights::READ), 2, TenantId(1)),
    ] {
        let mut table = IommuMap::new();
        let before = snapshot(&table);
        assert_eq!(
            table.map_batch(&[request(&a, 0x100, 0x1000), request(&bad, 0x100, 0x9000)]),
            Err(MappingFailure {
                request_index: 1,
                cause: MapError::NoMemoryCap
            })
        );
        assert_eq!(snapshot(&table), before);
    }
}

#[test]
fn conflicting_buffers_and_overflow_are_refused_atomically() {
    let a = cap(1);
    for (pa, cause) in [
        (0x1800, MapError::Overlap),
        (u64::MAX - 8, MapError::BadRange),
    ] {
        let mut table = IommuMap::new();
        let before = snapshot(&table);
        assert_eq!(
            table.map_batch(&[request(&a, 0x100, 0x1000), request(&a, 0x100, pa)]),
            Err(MappingFailure {
                request_index: 1,
                cause
            })
        );
        assert_eq!(snapshot(&table), before);
    }
}

#[test]
fn capacity_failure_does_not_consume_last_slot_or_capture_new_stream() {
    let a = cap(1);
    let mut table = IommuMap::new();
    for i in 0..MAX_MAPS - 1 {
        table
            .map(&a, request(&a, 0x100, 0x1000 + i as u64 * 0x2000).mapping)
            .unwrap();
    }
    let before = snapshot(&table);
    assert_eq!(
        table.map_batch(&[request(&a, 0x200, 0x90000), request(&a, 0x300, 0xb0000)]),
        Err(MappingFailure {
            request_index: 1,
            cause: MapError::TableFull
        })
    );
    assert_eq!(snapshot(&table), before);
    assert!(table.map_batch(&[request(&a, 0x200, 0x90000)]).is_ok());
    assert_eq!(table.len(), MAX_MAPS);
}

#[test]
fn empty_and_oversized_batches_do_not_touch_state() {
    let a = cap(1);
    let mut table = IommuMap::new();
    let before = snapshot(&table);
    assert!(table.map_batch(&[]).unwrap().is_empty());
    assert_eq!(
        table.map_batch(&[request(&a, 0x100, 0x1000); MAX_MAPS + 1]),
        Err(MappingFailure {
            request_index: MAX_MAPS,
            cause: MapError::TableFull
        })
    );
    assert_eq!(snapshot(&table), before);
}

#[test]
fn sequential_negative_control_exposes_partial_admission() {
    let a = cap(1);
    let requests = [request(&a, 0x100, 0x1000), request(&a, 0x100, u64::MAX)];
    let mut sequential = IommuMap::new();
    sequential
        .map(requests[0].cap, requests[0].mapping)
        .unwrap();
    assert_eq!(
        sequential.map(requests[1].cap, requests[1].mapping),
        Err(MapError::BadRange)
    );
    assert_eq!(sequential.len(), 1);
    let mut atomic = IommuMap::new();
    assert!(atomic.map_batch(&requests).is_err());
    assert_eq!(atomic.len(), 0);
    assert_eq!(atomic.ste_count(), 0);
}

#[test]
fn seeded_fail_retry_cycles_preserve_unrelated_tenant_and_reclaim_capacity() {
    let a = cap(1);
    let b = cap(2);
    let mut table = IommuMap::new();
    let live = table.map(&a, request(&a, 0x100, 0x1000).mapping).unwrap();
    let mut seed = 0x5ae7_u64;
    for cycle in 0..256 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let bad_index = (seed >> 32) as usize % 3;
        let base = 0x100000 + cycle * 0x10000;
        let mut requests = [
            request(&b, 0x200, base),
            request(&b, 0x200, base + 0x2000),
            request(&b, 0x200, base + 0x4000),
        ];
        requests[bad_index].mapping.len = 0;
        let before = snapshot(&table);
        assert_eq!(
            table.map_batch(&requests),
            Err(MappingFailure {
                request_index: bad_index,
                cause: MapError::BadRange
            }),
            "cycle={cycle}"
        );
        assert_eq!(snapshot(&table), before, "cycle={cycle}");
        requests[bad_index].mapping.len = 4096;
        let regions = table.map_batch(&requests).unwrap();
        for region in regions.iter() {
            assert_eq!(
                table.resolve_stream(region.stream_id, region.iova),
                Some(region.guest_pa)
            );
            table.unmap_for(&b, region.iova).unwrap();
        }
        assert_eq!(table.len(), 1);
        assert_eq!(
            table.resolve_stream(live.stream_id, live.iova),
            Some(live.guest_pa)
        );
    }
}
