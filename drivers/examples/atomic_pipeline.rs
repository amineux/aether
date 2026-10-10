//! Executable batch-admission consumer: two tenants, actual software MatMul.
use aether_core::accel::{AccelJobDesc, DmaView, SliceMem};
use aether_core::admission::{MappingFailure, MappingRequest};
use aether_core::caps::{CapKind, CapRights, Capability};
use aether_core::iommu::{IommuMap, MapError, MapRequest, StreamId};
use aether_core::{PhysAddr, TenantId};
use aether_drivers::SoftCommandProcessor;
use aether_hal::AccelDevice;

fn requests<'a>(cap: &'a Capability, sid: u32, base: u64) -> [MappingRequest<'a>; 3] {
    [base, base + 0x1000, base + 0x2000].map(|pa| MappingRequest {
        cap,
        mapping: MapRequest::pin_stream(PhysAddr(pa), 16, sid),
    })
}

fn write_matrix(mem: &mut SliceMem<'_>, base: u64, values: [i32; 4]) {
    for (i, value) in values.into_iter().enumerate() {
        mem.store_i32(PhysAddr(base + i as u64 * 4), value).unwrap();
    }
}

fn compute(
    cp: &mut SoftCommandProcessor<SliceMem<'_>>,
    tenant: u32,
    tile: u16,
    base: u64,
) -> [i32; 4] {
    let mut job = AccelJobDesc::matmul_i32(
        2,
        2,
        2,
        PhysAddr(base),
        PhysAddr(base + 0x1000),
        PhysAddr(base + 0x2000),
        tenant,
    );
    job.place = job.place.with_tile(tile);
    cp.create_xqueue(0, StreamId::from_raw((u32::from(tile) << 8) | 1), 0)
        .unwrap();
    let id = cp.submit(&job).unwrap();
    let completion = cp.service().expect("software execution completes");
    assert_eq!(completion.status, 0);
    assert_eq!(cp.poll().unwrap().job_seq, id);
    core::array::from_fn(|i| {
        cp.mem
            .load_i32(PhysAddr(base + 0x2000 + i as u64 * 4))
            .unwrap()
    })
}

fn main() {
    let a = Capability::new(CapKind::Memory, CapRights::MEM_FULL, 1, TenantId(1));
    let b = Capability::new(CapKind::Memory, CapRights::MEM_FULL, 2, TenantId(2));
    let mut backing = [0u8; 0x8000];
    let mut cp = SoftCommandProcessor::new(SliceMem {
        base: PhysAddr(0),
        bytes: &mut backing,
    });
    write_matrix(&mut cp.mem, 0x1000, [1, 2, 3, 4]);
    write_matrix(&mut cp.mem, 0x2000, [5, 6, 7, 8]);
    write_matrix(&mut cp.mem, 0x5000, [2, 0, 0, 2]);
    write_matrix(&mut cp.mem, 0x6000, [3, 4, 5, 6]);

    // Admit A's operands and execute through the existing CP/firewall.
    let a_requests = requests(&a, 0x101, 0x1000);
    let a_batch = cp.iommu.map_batch(&a_requests).unwrap();
    assert_eq!(compute(&mut cp, 1, 1, 0x1000), [19, 22, 43, 50]);
    cp.iommu
        .resolve_ats(0x101, a_batch.get(0).unwrap().iova)
        .unwrap();
    let before = format!("{:?}", cp.iommu);

    // B first asks for its own input, then tries to capture A's stream.
    let mut b_requests = requests(&b, 0x201, 0x5000);
    b_requests[1].mapping.stream_id = 0x101;
    assert_eq!(
        cp.iommu.map_batch(&b_requests),
        Err(MappingFailure {
            request_index: 1,
            cause: MapError::CrossTenant,
        })
    );
    assert_eq!(format!("{:?}", cp.iommu), before);
    assert_eq!(compute(&mut cp, 1, 1, 0x1000), [19, 22, 43, 50]);
    println!("[atomic-pipeline] foreign-stream=refused rollback=complete neighbor-output=correct");

    // Correct the request, then execute B's software MatMul.
    b_requests[1].mapping.stream_id = 0x201;
    let b_batch = cp.iommu.map_batch(&b_requests).unwrap();
    assert_eq!(compute(&mut cp, 2, 2, 0x5000), [6, 8, 10, 12]);
    for region in b_batch.iter() {
        cp.iommu.unmap_for(&b, region.iova).unwrap();
    }
    assert_eq!(cp.iommu.len(), 3);
    assert_eq!(compute(&mut cp, 1, 1, 0x1000), [19, 22, 43, 50]);
    assert!(cp.iommu.submit_sid().is_none());
    assert_eq!(
        cp.iommu.stream_state(StreamId::from_raw(0x101)),
        aether_core::iommu::StreamState::Bound
    );
    println!("[atomic-pipeline] retry=admitted output=[6,8,10,12] cleanup=complete neighbor-output=correct");
    println!(
        "[atomic-pipeline] staging-table-bytes={} scope=software-model",
        core::mem::size_of::<IommuMap>()
    );
}
