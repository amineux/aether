//! Malformed boot metadata must not publish a valid-prefix memory map.
use aether_core::mmap::{
    parse_multiboot2, MapSource, MmapError, MB2_TAG_BASIC_MEM, MB2_TAG_END, MB2_TAG_MMAP,
    REGION_AVAILABLE,
};

fn tag(kind: u32, payload: &[u8]) -> Vec<u8> {
    let mut bytes = kind.to_le_bytes().to_vec();
    bytes.extend_from_slice(&((payload.len() + 8) as u32).to_le_bytes());
    bytes.extend_from_slice(payload);
    bytes.resize((bytes.len() + 7) & !7, 0);
    bytes
}

fn mmap_tag(stride: usize, version: u32, tail: &[u8]) -> Vec<u8> {
    let mut payload = (stride as u32).to_le_bytes().to_vec();
    payload.extend_from_slice(&version.to_le_bytes());
    let start = payload.len();
    payload.extend_from_slice(&0x0100_0000u64.to_le_bytes());
    payload.extend_from_slice(&0x0200_0000u64.to_le_bytes());
    payload.extend_from_slice(&REGION_AVAILABLE.to_le_bytes());
    payload.resize(start + stride, 0xA5);
    payload.extend_from_slice(tail);
    tag(MB2_TAG_MMAP, &payload)
}

fn info(tags: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = vec![0; 8];
    for tag in tags {
        bytes.extend_from_slice(tag);
    }
    let total = bytes.len() as u32;
    bytes[..4].copy_from_slice(&total.to_le_bytes());
    bytes
}

fn end() -> Vec<u8> {
    tag(MB2_TAG_END, &[])
}

#[test]
fn complete_map_is_accepted() {
    let bytes = info(&[mmap_tag(24, 0, &[]), end()]);
    let map = parse_multiboot2(&bytes).unwrap();
    assert_eq!(map.source, MapSource::Multiboot2);
    assert_eq!(map.n_entries, 1);
    assert_eq!(map.regions()[0].start, 0x0100_0000);
    assert_eq!(map.regions()[0].end, 0x0300_0000);
}

#[test]
fn future_entries_versions_and_reserved_fields_are_accepted() {
    // The specification promises backward-compatible entry extensions and
    // requires the OS to ignore reserved fields, not insist they are zero.
    let mut bytes = info(&[mmap_tag(32, 7, &[]), end()]);
    bytes[4..8].fill(0xFF);
    assert_eq!(parse_multiboot2(&bytes).unwrap().n_entries, 1);
}

#[test]
fn unknown_tags_with_padding_are_skipped() {
    let bytes = info(&[tag(0xFFFF, &[1, 2, 3]), mmap_tag(24, 0, &[]), end()]);
    assert_eq!(parse_multiboot2(&bytes).unwrap().n_entries, 1);
}

#[test]
fn missing_end_tag_is_truncated() {
    let bytes = info(&[mmap_tag(24, 0, &[])]);
    assert_eq!(parse_multiboot2(&bytes), Err(MmapError::Truncated));
}

#[test]
fn oversized_end_tag_is_truncated() {
    let bytes = info(&[mmap_tag(24, 0, &[]), tag(MB2_TAG_END, &[0; 8])]);
    assert_eq!(parse_multiboot2(&bytes), Err(MmapError::Truncated));
}

#[test]
fn early_end_cannot_hide_trailing_tags() {
    let bytes = info(&[mmap_tag(24, 0, &[]), end(), tag(0xFFFF, &[])]);
    assert_eq!(parse_multiboot2(&bytes), Err(MmapError::Truncated));
}

#[test]
fn short_tag_header_after_valid_map_is_truncated() {
    let bytes = info(&[mmap_tag(24, 0, &[]), vec![0; 4]]);
    assert_eq!(parse_multiboot2(&bytes), Err(MmapError::Truncated));
}

#[test]
fn incomplete_mmap_entry_cannot_be_ignored_after_valid_prefix() {
    for tail in 1..24 {
        let bytes = info(&[mmap_tag(24, 0, &vec![0; tail]), end()]);
        assert_eq!(
            parse_multiboot2(&bytes),
            Err(MmapError::Truncated),
            "tail={tail}"
        );
    }
}

#[test]
fn mmap_entry_stride_requires_complete_aligned_record() {
    for stride in [20, 21, 22, 23, 25, 26, 27, 28, 29, 30, 31] {
        let bytes = info(&[mmap_tag(stride, 0, &[]), end()]);
        assert_eq!(
            parse_multiboot2(&bytes),
            Err(MmapError::Truncated),
            "stride={stride}"
        );
    }
}

#[test]
fn short_basic_memory_tag_is_not_hidden_by_valid_mmap() {
    let bytes = info(&[mmap_tag(24, 0, &[]), tag(MB2_TAG_BASIC_MEM, &[]), end()]);
    assert_eq!(parse_multiboot2(&bytes), Err(MmapError::Truncated));
}

#[test]
fn end_outside_declared_total_does_not_complete_map() {
    let mut bytes = info(&[mmap_tag(24, 0, &[])]);
    bytes.extend_from_slice(&end());
    assert_eq!(parse_multiboot2(&bytes), Err(MmapError::Truncated));
}

#[test]
fn bytes_outside_complete_declared_blob_are_ignored() {
    let mut bytes = info(&[mmap_tag(24, 0, &[]), end()]);
    bytes.extend_from_slice(&[0xFF; 17]);
    assert_eq!(parse_multiboot2(&bytes).unwrap().n_entries, 1);
}

#[test]
fn every_truncated_prefix_of_complete_blob_is_rejected() {
    let bytes = info(&[mmap_tag(24, 0, &[]), end()]);
    for len in 0..bytes.len() {
        assert_eq!(
            parse_multiboot2(&bytes[..len]),
            Err(MmapError::Truncated),
            "len={len}"
        );
    }
}
