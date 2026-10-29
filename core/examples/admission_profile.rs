//! Host-only admission microbenchmark. No kernel or hardware timing claims.
use aether_core::admission::MappingRequest;
use aether_core::caps::{CapKind, CapRights, Capability};
use aether_core::iommu::{IommuMap, MapRequest};
use aether_core::{PhysAddr, TenantId};
use std::{hint::black_box, time::Instant};

fn main() {
    let samples: usize = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "1000".into())
        .parse()
        .unwrap();
    assert!((10..=100_000).contains(&samples));
    let cap = Capability::new(CapKind::Memory, CapRights::MEM_FULL, 1, TenantId(1));
    let mut table = Box::new(IommuMap::new());
    let mut scratch = Box::new(IommuMap::new());
    println!(
        "{{\"schema\":\"aether-admission-profile/v1\",\"table_bytes\":{},\"cases\":[",
        std::mem::size_of::<IommuMap>()
    );
    let mut first = true;
    for with_scratch in [false, true] {
        for refusal in [false, true] {
            *table = IommuMap::new();
            let requests = [0x1000, 0x3000, 0x5000].map(|pa| MappingRequest {
                cap: &cap,
                mapping: MapRequest::pin_stream(
                    PhysAddr(pa),
                    if refusal && pa == 0x5000 { 0 } else { 4096 },
                    0x100,
                ),
            });
            let mut times = Vec::with_capacity(samples);
            // Warmups are unrecorded; cleanup and assertions are outside timing.
            for index in 0..samples + 100 {
                let start = Instant::now();
                let result = if with_scratch {
                    black_box(&mut *table)
                        .map_batch_with_scratch(black_box(&requests), black_box(&mut *scratch))
                } else {
                    black_box(&mut *table).map_batch(black_box(&requests))
                };
                let elapsed = start.elapsed().as_nanos();
                if index >= 100 {
                    times.push(elapsed);
                }
                if refusal {
                    assert_eq!(result.unwrap_err().request_index, 2);
                    assert!(table.is_empty());
                } else {
                    let batch = result.unwrap();
                    assert_eq!(batch.len(), 3);
                    for region in batch.iter() {
                        assert_eq!(
                            table.resolve_stream(region.stream_id, region.iova),
                            Some(region.guest_pa)
                        );
                        table.unmap_for(&cap, region.iova).unwrap();
                    }
                }
            }
            if !first {
                println!(",");
            }
            first = false;
            print!(
                "{{\"api\":\"{}\",\"outcome\":\"{}\",\"latency_ns\":{:?}}}",
                if with_scratch {
                    "caller_scratch"
                } else {
                    "owned_staging"
                },
                if refusal { "refusal_at_2" } else { "success_3" },
                times
            );
        }
    }
    println!("]}}");
}
