use aether_core::caps::{CapKind, CapRights, Capability};
use aether_core::iommu::{IommuMap, MapError, MapRequest, StreamId};
use aether_core::types::{ChipletId, PhysAddr, TenantId, TileId};

fn cap() -> Capability {
    Capability::new(CapKind::Memory, CapRights::MEM_FULL, 1, TenantId(1)).with_generation(1)
}

#[test]
fn pinned_iova_coverage_is_not_a_contiguous_guest_pa_translation() {
    for second in [0x2000, 0x4000] {
        let mut map = IommuMap::new();
        let sid = StreamId::accel(ChipletId(0), TileId(0), 1);
        let a = map
            .map(&cap(), MapRequest::pin_accel(PhysAddr(0x1000), 0x1000, sid))
            .unwrap();
        let b = map
            .map(&cap(), MapRequest::pin_accel(PhysAddr(second), 0x1000, sid))
            .unwrap();
        assert_eq!(b.iova.0, a.iova.0 + 0x1000);
        let start = PhysAddr(a.iova.0 + 0xffc);
        assert!(
            map.covers_iova(sid.raw(), start, 8),
            "both IOVA leaves are fully pinned"
        );
        let result = map.resolve_range(sid.raw(), start, 8, Some(TenantId(1)));
        assert_eq!(
            result,
            if second == 0x2000 {
                Ok(PhysAddr(0x1ffc))
            } else {
                Err(MapError::BadRange)
            }
        );
        assert_eq!(
            map.resolve_range(sid.raw(), start, 4, None),
            Ok(PhysAddr(0x1ffc))
        );
        assert!(map
            .resolve_range(sid.raw(), PhysAddr(b.iova.0 + 0xffc), 8, None)
            .is_err());
        assert_eq!(
            map.atc_len(),
            0,
            "table range walks do not populate the ATC"
        );
    }
}

#[test]
fn iova_range_checks_bounds_stream_and_tenant() {
    let mut map = IommuMap::new();
    let sid = StreamId::accel(ChipletId(0), TileId(0), 1);
    let other = StreamId::accel(ChipletId(0), TileId(1), 1);
    let r = map
        .map(&cap(), MapRequest::pin_accel(PhysAddr(0x1000), 16, sid))
        .unwrap();
    assert_eq!(
        map.resolve_range(sid.raw(), r.iova, 16, None),
        Ok(PhysAddr(0x1000))
    );
    assert_eq!(
        map.resolve_range(sid.raw(), r.iova, 0, None),
        Err(MapError::BadRange)
    );
    assert_eq!(
        map.resolve_range(sid.raw(), PhysAddr(u64::MAX), 2, None),
        Err(MapError::BadRange)
    );
    assert_eq!(
        map.resolve_range(sid.raw(), r.iova, 16, Some(TenantId(2))),
        Err(MapError::CrossTenant)
    );
    assert!(map.resolve_range(other.raw(), r.iova, 16, None).is_err());
    map.bind_stream(&cap(), other).unwrap();
    assert!(map.resolve_range(other.raw(), r.iova, 16, None).is_err());
    assert_eq!(
        map.resolve_range(sid.raw(), r.iova, 17, None),
        Err(MapError::NotMapped)
    );
    map.unmap(&cap(), r.iova).unwrap();
    assert!(map.resolve_range(sid.raw(), r.iova, 16, None).is_err());
}
