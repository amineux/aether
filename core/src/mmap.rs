//! Multiboot1 / Multiboot2 memory-map parser.
//!
//! Host-tested, allocation-free. The kernel copies the bootloader
//! structures into a byte slice and feeds them here. This is not a
//! general physical-memory manager: no hotplug, no FDT, no 64-bit
//! windows beyond the trampoline's 4 GiB identity map.

pub const MB1_BOOT_MAGIC: u32 = 0x2BADB002;
pub const MB2_BOOT_MAGIC: u32 = 0x36D7_6289;

/// Multiboot1 info `flags` bit 0: `mem_lower` / `mem_upper` present.
pub const MB1_FLAG_MEM: u32 = 1 << 0;
/// Multiboot1 info `flags` bit 2: cmdline physical pointer present.
pub const MB1_FLAG_CMDLINE: u32 = 1 << 2;
/// Multiboot1 info `flags` bit 6: mmap present.
pub const MB1_FLAG_MMAP: u32 = 1 << 6;

pub const MB1_INFO_MIN: usize = 52;
pub const MB2_INFO_MIN: usize = 8;

/// Multiboot mmap type 1: available RAM.
pub const REGION_AVAILABLE: u32 = 1;

pub const MB2_TAG_END: u32 = 0;
pub const MB2_TAG_BASIC_MEM: u32 = 4;
pub const MB2_TAG_MMAP: u32 = 6;

/// Maximum usable regions we keep. QEMU e820 is typically 4–8 entries.
pub const MAX_REGIONS: usize = 16;

/// 4 KiB frames. Matches the kernel bitmap allocator.
pub const FRAME_SIZE: u64 = 4096;
/// Bitmap cap: 128 MiB of managed frames. Larger maps are parsed and
/// printed; only this many frames are handed to the allocator.
pub const MAX_MANAGED_FRAMES: usize = 32768;
pub const FRAME_CAP_BYTES: u64 = MAX_MANAGED_FRAMES as u64 * FRAME_SIZE;

/// Boot-reserved floor (16 MiB). Type-1 RAM below this is parsed and
/// printed but not given to the frame allocator: trampoline, boot
/// page tables, AP SIPI page, Multiboot blob, and kernel image live
/// there. Documented subset, not a silent hole.
pub const BOOT_RESERVE_FLOOR: u64 = 0x0100_0000;

/// Trampoline identity map is 4 GiB. Regions above this are ignored.
pub const IDENTITY_LIMIT: u64 = 0x1_0000_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MmapError {
    Truncated,
    BadMagic,
    NoMmap,
    Empty,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapSource {
    /// Multiboot1 mmap (info flags bit 6).
    Multiboot1,
    /// Multiboot1 `mem_upper` only (no mmap tag). Weaker: one range
    /// from 1 MiB for `mem_upper` KiB.
    Multiboot1MemUpper,
    /// Multiboot2 mmap tag (type 6).
    Multiboot2,
    /// Multiboot2 basic memory tag (type 4) only.
    Multiboot2MemUpper,
}

impl MapSource {
    pub fn name(self) -> &'static str {
        match self {
            MapSource::Multiboot1 => "multiboot1",
            MapSource::Multiboot1MemUpper => "multiboot1-mem_upper",
            MapSource::Multiboot2 => "multiboot2",
            MapSource::Multiboot2MemUpper => "multiboot2-mem_upper",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysRegion {
    /// Inclusive start, exclusive end.
    pub start: u64,
    pub end: u64,
}

impl PhysRegion {
    pub fn new(start: u64, end: u64) -> Option<Self> {
        if end > start {
            Some(Self { start, end })
        } else {
            None
        }
    }

    pub fn len(self) -> u64 {
        self.end.saturating_sub(self.start)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryMap {
    pub source: MapSource,
    /// Total mmap entries walked (usable + reserved + ACPI + …).
    pub n_entries: usize,
    pub n_usable: usize,
    pub usable: [PhysRegion; MAX_REGIONS],
}

impl MemoryMap {
    pub fn empty(source: MapSource) -> Self {
        Self {
            source,
            n_entries: 0,
            n_usable: 0,
            usable: [PhysRegion { start: 0, end: 0 }; MAX_REGIONS],
        }
    }

    pub fn regions(&self) -> &[PhysRegion] {
        &self.usable[..self.n_usable]
    }

    pub fn is_empty(&self) -> bool {
        self.n_usable == 0
    }
}

fn r32(b: &[u8], off: usize) -> Result<u32, MmapError> {
    let s = b.get(off..off + 4).ok_or(MmapError::Truncated)?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn r64(b: &[u8], off: usize) -> Result<u64, MmapError> {
    let s = b.get(off..off + 8).ok_or(MmapError::Truncated)?;
    Ok(u64::from_le_bytes([
        s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
    ]))
}

fn push_usable(map: &mut MemoryMap, start: u64, len: u64) {
    let Some(end) = start.checked_add(len) else {
        return;
    };
    let Some(mut r) = PhysRegion::new(start, end) else {
        return;
    };
    if map.n_usable >= MAX_REGIONS {
        return;
    }
    // Merge with an overlapping / adjacent region if we already have one.
    for i in 0..map.n_usable {
        let e = map.usable[i];
        if r.end >= e.start && e.end >= r.start {
            r.start = r.start.min(e.start);
            r.end = r.end.max(e.end);
            map.usable[i] = r;
            // Fold any later regions that now overlap.
            compact_overlaps(map);
            return;
        }
    }
    map.usable[map.n_usable] = r;
    map.n_usable += 1;
}

fn compact_overlaps(map: &mut MemoryMap) {
    let mut i = 0;
    while i < map.n_usable {
        let mut j = i + 1;
        while j < map.n_usable {
            let a = map.usable[i];
            let b = map.usable[j];
            if a.end >= b.start && b.end >= a.start {
                map.usable[i] = PhysRegion {
                    start: a.start.min(b.start),
                    end: a.end.max(b.end),
                };
                map.n_usable -= 1;
                map.usable[j] = map.usable[map.n_usable];
            } else {
                j += 1;
            }
        }
        i += 1;
    }
}

/// Parse Multiboot1 mmap bytes (`mmap_addr` … `mmap_addr + mmap_length`).
/// Each entry is `size` (u32) + `size` bytes (`base` u64, `len` u64, `type` u32, …).
pub fn parse_multiboot1_mmap(mmap: &[u8]) -> Result<MemoryMap, MmapError> {
    if mmap.is_empty() {
        return Err(MmapError::Empty);
    }
    let mut map = MemoryMap::empty(MapSource::Multiboot1);
    let mut off = 0usize;
    while off < mmap.len() {
        let size = r32(mmap, off)? as usize;
        // `size` covers the rest of the entry (not including itself).
        if size < 20 {
            return Err(MmapError::Truncated);
        }
        let need = size.saturating_add(4);
        if off.saturating_add(need) > mmap.len() {
            return Err(MmapError::Truncated);
        }
        let base = r64(mmap, off + 4)?;
        let len = r64(mmap, off + 12)?;
        let ty = r32(mmap, off + 20)?;
        map.n_entries += 1;
        if ty == REGION_AVAILABLE {
            push_usable(&mut map, base, len);
        }
        off = off.saturating_add(need);
    }
    if map.n_usable == 0 {
        return Err(MmapError::Empty);
    }
    Ok(map)
}

fn mem_upper_map(source: MapSource, mem_upper_kb: u32) -> Result<MemoryMap, MmapError> {
    let bytes = (mem_upper_kb as u64).saturating_mul(1024);
    let mut map = MemoryMap::empty(source);
    map.n_entries = 1;
    push_usable(&mut map, 0x0010_0000, bytes);
    if map.n_usable == 0 {
        return Err(MmapError::Empty);
    }
    Ok(map)
}

/// Parse a Multiboot1 info header plus its mmap payload.
///
/// `info` is the Multiboot information structure. `mmap` is the bytes
/// at `mmap_addr` (may be empty if only `mem_upper` is used).
pub fn parse_multiboot1(info: &[u8], mmap: &[u8]) -> Result<MemoryMap, MmapError> {
    if info.len() < MB1_INFO_MIN {
        return Err(MmapError::Truncated);
    }
    let flags = r32(info, 0)?;
    if flags & MB1_FLAG_MMAP != 0 {
        if mmap.is_empty() {
            return Err(MmapError::NoMmap);
        }
        return parse_multiboot1_mmap(mmap);
    }
    if flags & MB1_FLAG_MEM != 0 {
        let mem_upper = r32(info, 8)?;
        return mem_upper_map(MapSource::Multiboot1MemUpper, mem_upper);
    }
    Err(MmapError::NoMmap)
}

/// Parse a Multiboot2 info blob (`total_size` + reserved + tags).
pub fn parse_multiboot2(info: &[u8]) -> Result<MemoryMap, MmapError> {
    if info.len() < MB2_INFO_MIN {
        return Err(MmapError::Truncated);
    }
    let total = r32(info, 0)? as usize;
    if total < MB2_INFO_MIN || total > info.len() {
        return Err(MmapError::Truncated);
    }
    let mut off = 8usize;
    let mut mmap: Option<MemoryMap> = None;
    let mut mem_upper: Option<u32> = None;
    while off + 8 <= total {
        let ty = r32(info, off)?;
        let size = r32(info, off + 4)? as usize;
        if size < 8 || off + size > total {
            return Err(MmapError::Truncated);
        }
        if ty == MB2_TAG_END {
            break;
        }
        if ty == MB2_TAG_MMAP {
            mmap = Some(parse_mb2_mmap_tag(&info[off..off + size])?);
        } else if ty == MB2_TAG_BASIC_MEM && size >= 16 {
            mem_upper = Some(r32(info, off + 12)?);
        }
        off = (off + size + 7) & !7;
    }
    if let Some(m) = mmap {
        return Ok(m);
    }
    if let Some(kb) = mem_upper {
        return mem_upper_map(MapSource::Multiboot2MemUpper, kb);
    }
    Err(MmapError::NoMmap)
}

fn parse_mb2_mmap_tag(tag: &[u8]) -> Result<MemoryMap, MmapError> {
    // type u32, size u32, entry_size u32, entry_version u32, entries…
    if tag.len() < 16 {
        return Err(MmapError::Truncated);
    }
    let entry_size = r32(tag, 8)? as usize;
    if entry_size < 20 {
        return Err(MmapError::Truncated);
    }
    let mut map = MemoryMap::empty(MapSource::Multiboot2);
    let mut off = 16usize;
    while off + entry_size <= tag.len() {
        let base = r64(tag, off)?;
        let len = r64(tag, off + 8)?;
        let ty = r32(tag, off + 16)?;
        map.n_entries += 1;
        if ty == REGION_AVAILABLE {
            push_usable(&mut map, base, len);
        }
        off += entry_size;
    }
    if map.n_usable == 0 {
        return Err(MmapError::Empty);
    }
    Ok(map)
}

/// Dispatch on the boot-protocol magic. `mmap` is only used for Multiboot1.
pub fn parse_boot_mmap(magic: u32, info: &[u8], mmap: &[u8]) -> Result<MemoryMap, MmapError> {
    match magic {
        MB1_BOOT_MAGIC => parse_multiboot1(info, mmap),
        MB2_BOOT_MAGIC => parse_multiboot2(info),
        _ => Err(MmapError::BadMagic),
    }
}

/// Drop usable RAM below `floor` (and empty leftovers).
pub fn clip_below(map: &MemoryMap, floor: u64) -> MemoryMap {
    let mut out = MemoryMap::empty(map.source);
    out.n_entries = map.n_entries;
    for r in map.regions() {
        let start = r.start.max(floor);
        if let Some(c) = PhysRegion::new(start, r.end) {
            if out.n_usable < MAX_REGIONS {
                out.usable[out.n_usable] = c;
                out.n_usable += 1;
            }
        }
    }
    out
}

/// Intersect usable regions with `[lo, hi)`.
pub fn clip_window(map: &MemoryMap, lo: u64, hi: u64) -> MemoryMap {
    let mut out = MemoryMap::empty(map.source);
    out.n_entries = map.n_entries;
    for r in map.regions() {
        let start = r.start.max(lo);
        let end = r.end.min(hi);
        if let Some(c) = PhysRegion::new(start, end) {
            if out.n_usable < MAX_REGIONS {
                out.usable[out.n_usable] = c;
                out.n_usable += 1;
            }
        }
    }
    out
}

/// Documented allocator subset: type-1 RAM above [`BOOT_RESERVE_FLOOR`],
/// inside the 4 GiB identity map, first [`FRAME_CAP_BYTES`] of that span.
///
/// Holes inside the span stay in the map as *absent* regions; the
/// kernel bitmap marks them used.
pub fn plan_frames(map: &MemoryMap) -> MemoryMap {
    let clipped = clip_below(map, BOOT_RESERVE_FLOOR);
    let clipped = clip_window(&clipped, BOOT_RESERVE_FLOOR, IDENTITY_LIMIT);
    if clipped.is_empty() {
        return clipped;
    }
    let lo = clipped.regions().iter().map(|r| r.start).min().unwrap_or(0);
    let cap_end = lo.saturating_add(FRAME_CAP_BYTES);
    clip_window(&clipped, lo, cap_end)
}

/// Inclusive-start / exclusive-end span of a planned map, if any.
pub fn span(map: &MemoryMap) -> Option<(u64, u64)> {
    if map.is_empty() {
        return None;
    }
    let lo = map.regions().iter().map(|r| r.start).min()?;
    let hi = map.regions().iter().map(|r| r.end).max()?;
    Some((lo, hi))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w32(buf: &mut [u8], off: usize, v: u32) {
        buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn w64(buf: &mut [u8], off: usize, v: u64) {
        buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
    }

    /// One Multiboot1 mmap entry at `off`. Returns bytes consumed (`size + 4`).
    fn mb1_entry(buf: &mut [u8], off: usize, base: u64, len: u64, ty: u32) -> usize {
        w32(buf, off, 20);
        w64(buf, off + 4, base);
        w64(buf, off + 12, len);
        w32(buf, off + 20, ty);
        24
    }

    #[test]
    fn mb1_two_usable_skips_reserved() {
        let mut mmap = [0u8; 96];
        let mut o = 0;
        o += mb1_entry(&mut mmap, o, 0, 0x9FC00, REGION_AVAILABLE);
        o += mb1_entry(&mut mmap, o, 0x9FC00, 0x400, 2);
        o += mb1_entry(&mut mmap, o, 0x0010_0000, 0x07F0_0000, REGION_AVAILABLE);
        let map = parse_multiboot1_mmap(&mmap[..o]).unwrap();
        assert_eq!(map.source, MapSource::Multiboot1);
        assert_eq!(map.n_entries, 3);
        assert_eq!(map.n_usable, 2);
        assert_eq!(map.usable[0], PhysRegion::new(0, 0x9FC00).unwrap());
        assert_eq!(
            map.usable[1],
            PhysRegion::new(0x0010_0000, 0x0800_0000).unwrap()
        );
    }

    #[test]
    fn mb1_info_dispatches_mmap() {
        let mut info = [0u8; 64];
        w32(&mut info, 0, MB1_FLAG_MEM | MB1_FLAG_MMAP);
        w32(&mut info, 8, 127 * 1024); // mem_upper KiB (ignored when mmap present)
        w32(&mut info, 44, 24);
        w32(&mut info, 48, 0x1000);
        let mut mmap = [0u8; 24];
        mb1_entry(&mut mmap, 0, 0x0010_0000, 0x07F0_0000, REGION_AVAILABLE);
        let map = parse_multiboot1(&info, &mmap).unwrap();
        assert_eq!(map.source, MapSource::Multiboot1);
        assert_eq!(map.n_usable, 1);
        assert_eq!(map.usable[0].end, 0x0800_0000);
    }

    #[test]
    fn mb1_mem_upper_fallback() {
        let mut info = [0u8; 64];
        w32(&mut info, 0, MB1_FLAG_MEM);
        w32(&mut info, 8, 127 * 1024);
        let map = parse_multiboot1(&info, &[]).unwrap();
        assert_eq!(map.source, MapSource::Multiboot1MemUpper);
        assert_eq!(map.n_usable, 1);
        assert_eq!(map.usable[0].start, 0x0010_0000);
        assert_eq!(map.usable[0].end, 0x0010_0000 + 127 * 1024 * 1024);
    }

    #[test]
    fn mb1_no_flags_is_nommap() {
        let info = [0u8; 64];
        assert_eq!(parse_multiboot1(&info, &[]).unwrap_err(), MmapError::NoMmap);
    }

    #[test]
    fn mb1_truncated_entry() {
        let mut mmap = [0u8; 8];
        w32(&mut mmap, 0, 20);
        assert_eq!(
            parse_multiboot1_mmap(&mmap).unwrap_err(),
            MmapError::Truncated
        );
    }

    #[test]
    fn mb1_merges_adjacent() {
        let mut mmap = [0u8; 48];
        let mut o = 0;
        o += mb1_entry(&mut mmap, o, 0x1000, 0x1000, REGION_AVAILABLE);
        o += mb1_entry(&mut mmap, o, 0x2000, 0x1000, REGION_AVAILABLE);
        let map = parse_multiboot1_mmap(&mmap[..o]).unwrap();
        assert_eq!(map.n_usable, 1);
        assert_eq!(map.usable[0], PhysRegion::new(0x1000, 0x3000).unwrap());
    }

    #[test]
    fn bad_magic() {
        assert_eq!(
            parse_boot_mmap(0xDEAD, &[0u8; 64], &[]).unwrap_err(),
            MmapError::BadMagic
        );
    }

    #[test]
    fn mb2_mmap_tag() {
        let mut b = [0u8; 80];
        w32(&mut b, 0, 80);
        w32(&mut b, 8, MB2_TAG_MMAP);
        w32(&mut b, 12, 64); // 16 header + 2*24
        w32(&mut b, 16, 24);
        w32(&mut b, 20, 0);
        w64(&mut b, 24, 0);
        w64(&mut b, 32, 0x9FC00);
        w32(&mut b, 40, REGION_AVAILABLE);
        w64(&mut b, 48, 0x0010_0000);
        w64(&mut b, 56, 0x07F0_0000);
        w32(&mut b, 64, REGION_AVAILABLE);
        w32(&mut b, 72, MB2_TAG_END);
        w32(&mut b, 76, 8);
        let map = parse_multiboot2(&b).unwrap();
        assert_eq!(map.source, MapSource::Multiboot2);
        assert_eq!(map.n_entries, 2);
        assert_eq!(map.n_usable, 2);
        assert_eq!(map.usable[0].end, 0x9FC00);
        assert_eq!(map.usable[1].end, 0x0800_0000);
    }

    #[test]
    fn mb2_mem_upper_fallback() {
        let mut b = [0u8; 32];
        w32(&mut b, 0, 32);
        w32(&mut b, 8, MB2_TAG_BASIC_MEM);
        w32(&mut b, 12, 16);
        w32(&mut b, 16, 640);
        w32(&mut b, 20, 127 * 1024);
        w32(&mut b, 24, MB2_TAG_END);
        w32(&mut b, 28, 8);
        let map = parse_multiboot2(&b).unwrap();
        assert_eq!(map.source, MapSource::Multiboot2MemUpper);
        assert_eq!(map.usable[0].start, 0x0010_0000);
    }

    #[test]
    fn plan_frames_clips_16mib_and_caps() {
        let mut map = MemoryMap::empty(MapSource::Multiboot1);
        map.n_entries = 2;
        push_usable(&mut map, 0, 0x9FC00);
        push_usable(&mut map, 0x0010_0000, 0x07F0_0000); // 1..128 MiB
        let planned = plan_frames(&map);
        assert_eq!(planned.n_usable, 1);
        assert_eq!(planned.usable[0].start, BOOT_RESERVE_FLOOR);
        assert_eq!(planned.usable[0].end, 0x0800_0000);
        assert_eq!(span(&planned), Some((0x0100_0000, 0x0800_0000)));
    }

    #[test]
    fn plan_frames_caps_span_not_total_ram() {
        let mut map = MemoryMap::empty(MapSource::Multiboot1);
        push_usable(&mut map, BOOT_RESERVE_FLOOR, 256 * 1024 * 1024);
        let planned = plan_frames(&map);
        assert_eq!(planned.n_usable, 1);
        assert_eq!(planned.usable[0].start, BOOT_RESERVE_FLOOR);
        assert_eq!(
            planned.usable[0].end,
            BOOT_RESERVE_FLOOR + FRAME_CAP_BYTES
        );
    }

    #[test]
    fn plan_frames_empty_below_floor() {
        let mut map = MemoryMap::empty(MapSource::Multiboot1);
        push_usable(&mut map, 0, 0x9FC00);
        let planned = plan_frames(&map);
        assert!(planned.is_empty());
    }

    #[test]
    fn plan_frames_drops_above_4gib() {
        let mut map = MemoryMap::empty(MapSource::Multiboot1);
        push_usable(&mut map, IDENTITY_LIMIT, 0x1000_0000);
        let planned = plan_frames(&map);
        assert!(planned.is_empty());
    }

    #[test]
    fn parse_boot_mmap_mb1_magic() {
        let mut info = [0u8; 64];
        w32(&mut info, 0, MB1_FLAG_MEM);
        w32(&mut info, 8, 64 * 1024);
        let map = parse_boot_mmap(MB1_BOOT_MAGIC, &info, &[]).unwrap();
        assert_eq!(map.source, MapSource::Multiboot1MemUpper);
    }
}
