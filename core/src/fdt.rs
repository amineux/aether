//! Flattened device tree (DTB) memory-map parser.
//!
//! Host-tested, allocation-free, `forbid(unsafe_code)` like [`crate::mmap`].
//! riscv64 (OpenSBI hands the DTB in `a1`) and aarch64 (QEMU virt) feed the
//! blob here so the frame allocator is sized from the firmware's `/memory`
//! nodes instead of a fixed fallback window.
//!
//! What it reads, and nothing else:
//! - the 40-byte header (magic `0xd00dfeed`, `totalsize`, version 17, block
//!   offsets and sizes inside `totalsize`);
//! - the memory-reservation block (`/memreserve/` pairs up to the `(0, 0)`
//!   terminator);
//! - the structure block: root `#address-cells` / `#size-cells`, every root
//!   child named `memory` / `memory@…` or with `device_type = "memory"`, and
//!   `reg` of `/reserved-memory` children (with that node's own cells).
//!
//! Usable RAM = memory `reg` minus memreserve minus `/reserved-memory` `reg`.
//! `/reserved-memory` children with only `size` (dynamic pools) are not placed
//! yet, so they are not subtracted; `status` is not consulted.
//!
//! Every malformed input refuses with a named [`FdtError`]. Nothing is
//! clamped or silently dropped: too many regions is
//! [`FdtError::TooManyRegions`], not a truncated list. This is a parser with
//! refuse paths and a mutation test, not a verified one.

use crate::mmap::{push_usable, MapSource, MemoryMap, PhysRegion, FRAME_SIZE, MAX_REGIONS};

pub const FDT_MAGIC: u32 = 0xd00d_feed;
pub const FDT_HEADER_LEN: usize = 40;
/// Structure-block format we read (v17 carries `size_dt_struct`).
pub const FDT_VERSION: u32 = 17;
/// QEMU virt (aarch64) hands a 1 MiB, unpacked blob; refuse anything past 2 MiB.
pub const FDT_MAX_TOTALSIZE: usize = 2 * 1024 * 1024;
/// Node nesting bound (root = depth 1). QEMU virt is 4 deep.
pub const FDT_MAX_DEPTH: usize = 16;
/// Memreserve + `/reserved-memory` entries kept.
pub const FDT_MAX_RESERVED: usize = 16;

pub const FDT_BEGIN_NODE: u32 = 1;
pub const FDT_END_NODE: u32 = 2;
pub const FDT_PROP: u32 = 3;
pub const FDT_NOP: u32 = 4;
pub const FDT_END: u32 = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FdtError {
    /// Blob shorter than the header or `totalsize`, or a token / property /
    /// memreserve entry runs past its block.
    Truncated,
    /// Header magic is not `0xd00dfeed`.
    BadMagic,
    /// `version` < 17 or `last_comp_version` > 17.
    BadVersion,
    /// `totalsize` > [`FDT_MAX_TOTALSIZE`].
    TooLarge,
    /// A block offset / size is misaligned, overlaps the header, or ends
    /// past `totalsize`; or `totalsize` is smaller than the header.
    BadOffset,
    /// Unknown token, property outside a node, unbalanced `END_NODE`, or
    /// `END` before the root closes.
    BadToken,
    /// Node name or property-name offset not NUL-terminated inside its block.
    BadString,
    /// Nesting deeper than [`FDT_MAX_DEPTH`].
    TooDeep,
    /// `#address-cells` / `#size-cells` not 1 or 2 (or not 4 bytes), or a
    /// `reg` length that is not a non-zero multiple of the entry size.
    BadCells,
    /// `base + size` overflows u64.
    BadRegion,
    /// No memory node, or no usable byte left after reservations.
    NoMemory,
    /// More than [`MAX_REGIONS`] memory ranges or [`FDT_MAX_RESERVED`]
    /// reservations, or subtracting a reservation would split past
    /// [`MAX_REGIONS`].
    TooManyRegions,
}

impl FdtError {
    pub fn name(self) -> &'static str {
        match self {
            FdtError::Truncated => "Truncated",
            FdtError::BadMagic => "BadMagic",
            FdtError::BadVersion => "BadVersion",
            FdtError::TooLarge => "TooLarge",
            FdtError::BadOffset => "BadOffset",
            FdtError::BadToken => "BadToken",
            FdtError::BadString => "BadString",
            FdtError::TooDeep => "TooDeep",
            FdtError::BadCells => "BadCells",
            FdtError::BadRegion => "BadRegion",
            FdtError::NoMemory => "NoMemory",
            FdtError::TooManyRegions => "TooManyRegions",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FdtHeader {
    pub totalsize: usize,
    pub off_dt_struct: usize,
    pub off_dt_strings: usize,
    pub off_mem_rsvmap: usize,
    pub version: u32,
    pub last_comp_version: u32,
    pub boot_cpuid_phys: u32,
    pub size_dt_strings: usize,
    pub size_dt_struct: usize,
}

fn be32(b: &[u8], off: usize) -> Result<u32, FdtError> {
    let end = off.checked_add(4).ok_or(FdtError::Truncated)?;
    let s = b.get(off..end).ok_or(FdtError::Truncated)?;
    Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn be64(b: &[u8], off: usize) -> Result<u64, FdtError> {
    let hi = be32(b, off)? as u64;
    let lo = be32(b, off.checked_add(4).ok_or(FdtError::Truncated)?)? as u64;
    Ok((hi << 32) | lo)
}

fn block_ok(off: usize, size: usize, align: usize, total: usize) -> Result<(), FdtError> {
    if off < FDT_HEADER_LEN || off % align != 0 {
        return Err(FdtError::BadOffset);
    }
    match off.checked_add(size) {
        Some(end) if end <= total => Ok(()),
        _ => Err(FdtError::BadOffset),
    }
}

/// Validate the 40-byte header. Needs only the first [`FDT_HEADER_LEN`]
/// bytes, so the kernel can size the blob from `totalsize` before it builds
/// the full slice. Block bounds are checked against `totalsize`.
pub fn parse_header(b: &[u8]) -> Result<FdtHeader, FdtError> {
    if b.len() < FDT_HEADER_LEN {
        return Err(FdtError::Truncated);
    }
    if be32(b, 0)? != FDT_MAGIC {
        return Err(FdtError::BadMagic);
    }
    let h = FdtHeader {
        totalsize: be32(b, 4)? as usize,
        off_dt_struct: be32(b, 8)? as usize,
        off_dt_strings: be32(b, 12)? as usize,
        off_mem_rsvmap: be32(b, 16)? as usize,
        version: be32(b, 20)?,
        last_comp_version: be32(b, 24)?,
        boot_cpuid_phys: be32(b, 28)?,
        size_dt_strings: be32(b, 32)? as usize,
        size_dt_struct: be32(b, 36)? as usize,
    };
    if h.version < FDT_VERSION || h.last_comp_version > FDT_VERSION {
        return Err(FdtError::BadVersion);
    }
    if h.totalsize > FDT_MAX_TOTALSIZE {
        return Err(FdtError::TooLarge);
    }
    if h.totalsize < FDT_HEADER_LEN {
        return Err(FdtError::BadOffset);
    }
    // Memreserve: 8-aligned, at least one 16-byte (terminator) entry.
    block_ok(h.off_mem_rsvmap, 16, 8, h.totalsize)?;
    block_ok(h.off_dt_struct, h.size_dt_struct, 4, h.totalsize)?;
    block_ok(h.off_dt_strings, h.size_dt_strings, 1, h.totalsize)?;
    Ok(h)
}

/// Parsed memory view of a DTB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FdtMemory {
    /// Usable RAM (memory `reg` minus every reservation), merged.
    pub map: MemoryMap,
    /// Non-empty `reg` ranges across memory nodes, before merging.
    pub n_memory: usize,
    /// Sum of those ranges.
    pub memory_bytes: u64,
    /// Memreserve pairs followed by `/reserved-memory` `reg` ranges.
    pub reserved: [PhysRegion; FDT_MAX_RESERVED],
    pub n_reserved: usize,
    pub n_memreserve: usize,
    pub totalsize: usize,
    pub boot_cpuid_phys: u32,
}

impl FdtMemory {
    pub fn reserved(&self) -> &[PhysRegion] {
        &self.reserved[..self.n_reserved]
    }

    pub fn usable_bytes(&self) -> u64 {
        self.map.regions().iter().map(|r| r.len()).sum()
    }
}

/// Remove `[r.start, r.end)` from `map`. A region split in two needs a free
/// slot; with none left this refuses instead of dropping RAM or keeping the
/// reserved bytes.
pub fn subtract(map: &mut MemoryMap, r: PhysRegion) -> Result<(), FdtError> {
    let mut i = 0;
    while i < map.n_usable {
        let e = map.usable[i];
        if r.end <= e.start || r.start >= e.end {
            i += 1;
            continue;
        }
        let left = PhysRegion::new(e.start, r.start.max(e.start).min(e.end));
        let right = PhysRegion::new(r.end.min(e.end).max(e.start), e.end);
        match (left, right) {
            (Some(l), Some(rr)) => {
                if map.n_usable >= MAX_REGIONS {
                    return Err(FdtError::TooManyRegions);
                }
                map.usable[i] = l;
                map.usable[map.n_usable] = rr;
                map.n_usable += 1;
                i += 1;
            }
            (Some(one), None) | (None, Some(one)) => {
                map.usable[i] = one;
                i += 1;
            }
            (None, None) => {
                map.n_usable -= 1;
                map.usable[i] = map.usable[map.n_usable];
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Other,
    MemoryName,
    ReservedMemory,
    ReservedChild,
}

#[derive(Clone, Copy)]
struct Node {
    kind: Kind,
    /// `device_type`: None = absent, Some(true) = "memory".
    device_memory: Option<bool>,
    /// `reg` value as (offset, len) into the blob.
    reg: Option<(usize, usize)>,
}

const NODE0: Node = Node { kind: Kind::Other, device_memory: None, reg: None };

struct Walk {
    root_ac: u32,
    root_sc: u32,
    rsv_ac: Option<u32>,
    rsv_sc: Option<u32>,
    mem: [PhysRegion; MAX_REGIONS],
    n_mem: usize,
    reserved: [PhysRegion; FDT_MAX_RESERVED],
    n_reserved: usize,
}

fn cells_value(b: &[u8], off: usize, len: usize) -> Result<u32, FdtError> {
    if len != 4 {
        return Err(FdtError::BadCells);
    }
    be32(b, off)
}

fn check_cells(ac: u32, sc: u32) -> Result<(), FdtError> {
    if (1..=2).contains(&ac) && (1..=2).contains(&sc) {
        Ok(())
    } else {
        Err(FdtError::BadCells)
    }
}

fn read_cells(b: &[u8], off: usize, n: u32) -> Result<u64, FdtError> {
    if n == 2 {
        be64(b, off)
    } else {
        Ok(be32(b, off)? as u64)
    }
}

/// Decode `reg` into `out[*n..]`. Zero-size entries carry no bytes and are
/// skipped (firmware placeholders); overflow refuses.
fn decode_reg(
    b: &[u8],
    (off, len): (usize, usize),
    ac: u32,
    sc: u32,
    out: &mut [PhysRegion],
    n: &mut usize,
) -> Result<(), FdtError> {
    check_cells(ac, sc)?;
    let entry = ((ac + sc) * 4) as usize;
    if len == 0 || len % entry != 0 {
        return Err(FdtError::BadCells);
    }
    let mut o = off;
    let end = off + len;
    while o < end {
        let base = read_cells(b, o, ac)?;
        let size = read_cells(b, o + ac as usize * 4, sc)?;
        o += entry;
        if size == 0 {
            continue;
        }
        let top = base.checked_add(size).ok_or(FdtError::BadRegion)?;
        if *n >= out.len() {
            return Err(FdtError::TooManyRegions);
        }
        out[*n] = PhysRegion { start: base, end: top };
        *n += 1;
    }
    Ok(())
}

/// NUL-terminated string at `off`, inside `[off, end)`.
fn cstr(b: &[u8], off: usize, end: usize) -> Result<&[u8], FdtError> {
    let s = b.get(off..end).ok_or(FdtError::BadString)?;
    let n = s.iter().position(|&c| c == 0).ok_or(FdtError::BadString)?;
    Ok(&s[..n])
}

fn align4(x: usize) -> Option<usize> {
    x.checked_add(3).map(|v| v & !3)
}

/// Parse a whole DTB. `blob` may be longer than `totalsize`; only
/// `blob[..totalsize]` is read.
pub fn parse_fdt_memory(blob: &[u8]) -> Result<FdtMemory, FdtError> {
    let h = parse_header(blob)?;
    if blob.len() < h.totalsize {
        return Err(FdtError::Truncated);
    }
    let b = &blob[..h.totalsize];

    let mut w = Walk {
        root_ac: 2,
        root_sc: 1,
        rsv_ac: None,
        rsv_sc: None,
        mem: [PhysRegion { start: 0, end: 0 }; MAX_REGIONS],
        n_mem: 0,
        reserved: [PhysRegion { start: 0, end: 0 }; FDT_MAX_RESERVED],
        n_reserved: 0,
    };

    // Memory-reservation block.
    let mut o = h.off_mem_rsvmap;
    let mut n_memreserve = 0usize;
    loop {
        let base = be64(b, o)?;
        let size = be64(b, o.checked_add(8).ok_or(FdtError::Truncated)?)?;
        o = o.checked_add(16).ok_or(FdtError::Truncated)?;
        if base == 0 && size == 0 {
            break;
        }
        if size == 0 {
            continue;
        }
        let top = base.checked_add(size).ok_or(FdtError::BadRegion)?;
        if w.n_reserved >= FDT_MAX_RESERVED {
            return Err(FdtError::TooManyRegions);
        }
        w.reserved[w.n_reserved] = PhysRegion { start: base, end: top };
        w.n_reserved += 1;
        n_memreserve += 1;
    }

    // Structure block.
    let s_end = h.off_dt_struct + h.size_dt_struct;
    let str_lo = h.off_dt_strings;
    let str_hi = h.off_dt_strings + h.size_dt_strings;
    let mut stack = [NODE0; FDT_MAX_DEPTH];
    let mut depth = 0usize;
    let mut root_seen = false;
    let mut o = h.off_dt_struct;
    loop {
        if o.checked_add(4).map_or(true, |e| e > s_end) {
            return Err(FdtError::Truncated);
        }
        let tok = be32(b, o)?;
        o += 4;
        match tok {
            FDT_BEGIN_NODE => {
                if depth == 0 && root_seen {
                    return Err(FdtError::BadToken);
                }
                let name = cstr(b, o, s_end)?;
                o = align4(o + name.len() + 1).ok_or(FdtError::Truncated)?;
                if depth >= FDT_MAX_DEPTH {
                    return Err(FdtError::TooDeep);
                }
                let kind = match depth {
                    1 if name == b"reserved-memory" => Kind::ReservedMemory,
                    1 if name == b"memory" || name.starts_with(b"memory@") => Kind::MemoryName,
                    2 if stack[1].kind == Kind::ReservedMemory => Kind::ReservedChild,
                    _ => Kind::Other,
                };
                stack[depth] = Node { kind, ..NODE0 };
                depth += 1;
                root_seen = true;
            }
            FDT_END_NODE => {
                if depth == 0 {
                    return Err(FdtError::BadToken);
                }
                depth -= 1;
                let n = stack[depth];
                if depth == 1 {
                    let is_memory = match n.device_memory {
                        Some(m) => m,
                        None => n.kind == Kind::MemoryName,
                    };
                    if is_memory {
                        if let Some(reg) = n.reg {
                            decode_reg(b, reg, w.root_ac, w.root_sc, &mut w.mem, &mut w.n_mem)?;
                        }
                    }
                } else if depth == 2 && n.kind == Kind::ReservedChild {
                    if let Some(reg) = n.reg {
                        let ac = w.rsv_ac.unwrap_or(w.root_ac);
                        let sc = w.rsv_sc.unwrap_or(w.root_sc);
                        decode_reg(b, reg, ac, sc, &mut w.reserved, &mut w.n_reserved)?;
                    }
                }
            }
            FDT_PROP => {
                if depth == 0 {
                    return Err(FdtError::BadToken);
                }
                let len = be32(b, o)? as usize;
                let nameoff = be32(b, o + 4)? as usize;
                let val = o + 8;
                let val_end = val.checked_add(len).ok_or(FdtError::Truncated)?;
                if val_end > s_end {
                    return Err(FdtError::Truncated);
                }
                let name_at = str_lo.checked_add(nameoff).ok_or(FdtError::BadString)?;
                if name_at >= str_hi {
                    return Err(FdtError::BadString);
                }
                let name = cstr(b, name_at, str_hi)?;
                o = align4(val_end).ok_or(FdtError::Truncated)?;
                let node = &mut stack[depth - 1];
                match (depth, name) {
                    (1, b"#address-cells") => w.root_ac = cells_value(b, val, len)?,
                    (1, b"#size-cells") => w.root_sc = cells_value(b, val, len)?,
                    (2, b"#address-cells") if node.kind == Kind::ReservedMemory => {
                        w.rsv_ac = Some(cells_value(b, val, len)?)
                    }
                    (2, b"#size-cells") if node.kind == Kind::ReservedMemory => {
                        w.rsv_sc = Some(cells_value(b, val, len)?)
                    }
                    (2, b"device_type") => {
                        node.device_memory = Some(b.get(val..val_end).map_or(false, |v| v == b"memory\0"))
                    }
                    (2, b"reg") | (3, b"reg") => node.reg = Some((val, len)),
                    _ => {}
                }
            }
            FDT_NOP => {}
            FDT_END => {
                if depth != 0 || !root_seen {
                    return Err(FdtError::BadToken);
                }
                break;
            }
            _ => return Err(FdtError::BadToken),
        }
    }

    if w.n_mem == 0 {
        return Err(FdtError::NoMemory);
    }
    let mut map = MemoryMap::empty(MapSource::Fdt);
    map.n_entries = w.n_mem;
    let mut memory_bytes = 0u64;
    for r in &w.mem[..w.n_mem] {
        memory_bytes = memory_bytes.saturating_add(r.len());
        // n_mem <= MAX_REGIONS and merging never grows the list, so this
        // cannot hit push_usable's full-table drop.
        push_usable(&mut map, r.start, r.len());
    }
    for r in &w.reserved[..w.n_reserved] {
        subtract(&mut map, *r)?;
    }
    if map.is_empty() {
        return Err(FdtError::NoMemory);
    }
    Ok(FdtMemory {
        map,
        n_memory: w.n_mem,
        memory_bytes,
        reserved: w.reserved,
        n_reserved: w.n_reserved,
        n_memreserve,
        totalsize: h.totalsize,
        boot_cpuid_phys: h.boot_cpuid_phys,
    })
}

/// Kernel frame plan from a parsed DTB: usable RAM minus the DTB blob's own
/// pages (so it survives frame handout), clipped to `[floor, limit)` and the
/// 128 MiB bitmap cap (see [`crate::mmap::plan_frames_window`]).
pub fn plan_fdt_frames(
    mem: &FdtMemory,
    dtb_pa: u64,
    floor: u64,
    limit: u64,
) -> Result<MemoryMap, FdtError> {
    let mut map = mem.map;
    let lo = dtb_pa & !(FRAME_SIZE - 1);
    let hi = dtb_pa
        .checked_add(mem.totalsize as u64)
        .and_then(|e| e.checked_add(FRAME_SIZE - 1))
        .ok_or(FdtError::BadRegion)?
        & !(FRAME_SIZE - 1);
    if let Some(r) = PhysRegion::new(lo, hi) {
        subtract(&mut map, r)?;
    }
    Ok(crate::mmap::plan_frames_window(&map, floor, limit))
}

// ---------------------------------------------------------------------------
// Fixture writer (also used by the red-team demos; host and kernel safe).
// ---------------------------------------------------------------------------

/// Output buffer size for [`build_virt_fixture`].
pub const FIXTURE_CAP: usize = 2048;
const FIXTURE_STR_CAP: usize = 256;

/// Knobs for a QEMU-virt-shaped DTB. [`VirtFixture::riscv`] mirrors
/// `qemu-system-riscv64 -machine virt -m 128M` after OpenSBI: root cells
/// 2/2, `memory@80000000`, cpus with `#size-cells = 0`, a soc UART, and a
/// `/reserved-memory/mmode_resources@80000000` child.
#[derive(Clone, Copy)]
pub struct VirtFixture<'a> {
    pub root_address_cells: u32,
    pub root_size_cells: u32,
    /// `reg` entries of `memory@…` (base, size).
    pub memory: &'a [(u64, u64)],
    pub with_memory_node: bool,
    pub reserved_memory: &'a [(u64, u64)],
    pub memreserve: &'a [(u64, u64)],
    /// Extra nested nodes under `/soc` (to probe the depth bound).
    pub extra_depth: usize,
}

impl VirtFixture<'static> {
    pub const RISCV_MEMORY: [(u64, u64); 1] = [(0x8000_0000, 0x0800_0000)];
    pub const RISCV_RESERVED: [(u64, u64); 1] = [(0x8000_0000, 0x0004_0000)];

    pub fn riscv() -> Self {
        VirtFixture {
            root_address_cells: 2,
            root_size_cells: 2,
            memory: &Self::RISCV_MEMORY,
            with_memory_node: true,
            reserved_memory: &Self::RISCV_RESERVED,
            memreserve: &[],
            extra_depth: 0,
        }
    }
}

struct Writer<'a> {
    s: &'a mut [u8],
    sl: usize,
    strs: [u8; FIXTURE_STR_CAP],
    tl: usize,
    ok: bool,
}

impl Writer<'_> {
    fn bytes(&mut self, v: &[u8]) {
        if self.sl + v.len() > self.s.len() {
            self.ok = false;
            return;
        }
        self.s[self.sl..self.sl + v.len()].copy_from_slice(v);
        self.sl += v.len();
    }
    fn u32(&mut self, v: u32) {
        self.bytes(&v.to_be_bytes());
    }
    fn pad(&mut self) {
        while self.sl % 4 != 0 && self.ok {
            self.bytes(&[0]);
        }
    }
    fn begin(&mut self, name: &[u8]) {
        self.u32(FDT_BEGIN_NODE);
        self.bytes(name);
        self.bytes(&[0]);
        self.pad();
    }
    fn end(&mut self) {
        self.u32(FDT_END_NODE);
    }
    fn stroff(&mut self, name: &[u8]) -> u32 {
        // Reuse an existing entry (dtc dedups names).
        let mut i = 0;
        while i < self.tl {
            let n = self.strs[i..self.tl].iter().position(|&c| c == 0).unwrap_or(0);
            if &self.strs[i..i + n] == name {
                return i as u32;
            }
            i += n + 1;
        }
        let off = self.tl;
        if off + name.len() + 1 > FIXTURE_STR_CAP {
            self.ok = false;
            return 0;
        }
        self.strs[off..off + name.len()].copy_from_slice(name);
        self.strs[off + name.len()] = 0;
        self.tl += name.len() + 1;
        off as u32
    }
    fn prop(&mut self, name: &[u8], val: &[u8]) {
        let off = self.stroff(name);
        self.u32(FDT_PROP);
        self.u32(val.len() as u32);
        self.u32(off);
        self.bytes(val);
        self.pad();
    }
    fn prop_u32(&mut self, name: &[u8], v: u32) {
        self.prop(name, &v.to_be_bytes());
    }
    fn prop_reg(&mut self, name: &[u8], ac: u32, sc: u32, regs: &[(u64, u64)]) {
        let mut v = [0u8; 16 * 18];
        let mut n = 0;
        for &(base, size) in regs {
            for (val, cells) in [(base, ac), (size, sc)] {
                for c in (0..cells).rev() {
                    let word = if c >= 2 { 0 } else { (val >> (32 * c)) as u32 };
                    if n + 4 > v.len() {
                        self.ok = false;
                        return;
                    }
                    v[n..n + 4].copy_from_slice(&word.to_be_bytes());
                    n += 4;
                }
            }
        }
        self.prop(name, &v[..n]);
    }
}

/// Build a byte-honest DTB into `out` (header, memreserve, struct, strings
/// in the dtc / QEMU order). Returns `totalsize`, or 0 if `out` is too small.
pub fn build_virt_fixture(f: &VirtFixture, out: &mut [u8]) -> usize {
    let rsv_off = FDT_HEADER_LEN;
    let rsv_len = (f.memreserve.len() + 1) * 16;
    let st_off = rsv_off + rsv_len;
    if out.len() < st_off {
        return 0;
    }
    let (_, body) = out.split_at_mut(st_off);
    let mut w = Writer { s: body, sl: 0, strs: [0; FIXTURE_STR_CAP], tl: 0, ok: true };
    let (ac, sc) = (f.root_address_cells, f.root_size_cells);

    w.begin(b"");
    w.prop_u32(b"#address-cells", ac);
    w.prop_u32(b"#size-cells", sc);
    w.prop(b"compatible", b"riscv-virtio\0");
    w.prop(b"model", b"riscv-virtio,qemu\0");
    w.begin(b"chosen");
    w.prop(b"stdout-path", b"/soc/serial@10000000\0");
    w.end();
    if f.with_memory_node {
        // dtc emits name then props; QEMU's aarch64 virt puts reg before
        // device_type, riscv the other way. Use the aarch64 order here so the
        // parser's "decide at END_NODE" path is the one exercised.
        w.begin(b"memory@80000000");
        w.prop_reg(b"reg", ac, sc, f.memory);
        w.prop(b"device_type", b"memory\0");
        w.end();
    }
    w.begin(b"cpus");
    w.prop_u32(b"#address-cells", 1);
    w.prop_u32(b"#size-cells", 0);
    w.prop_u32(b"timebase-frequency", 10_000_000);
    w.begin(b"cpu@0");
    w.prop(b"device_type", b"cpu\0");
    w.prop_u32(b"reg", 0);
    w.end();
    w.end();
    if !f.reserved_memory.is_empty() {
        w.begin(b"reserved-memory");
        w.prop_u32(b"#address-cells", ac);
        w.prop_u32(b"#size-cells", sc);
        w.prop(b"ranges", b"");
        w.begin(b"mmode_resources@80000000");
        w.prop_reg(b"reg", ac, sc, f.reserved_memory);
        w.prop(b"no-map", b"");
        w.end();
        w.end();
    }
    w.begin(b"soc");
    w.prop_u32(b"#address-cells", 2);
    w.prop_u32(b"#size-cells", 2);
    w.prop(b"ranges", b"");
    w.begin(b"serial@10000000");
    w.prop_reg(b"reg", 2, 2, &[(0x1000_0000, 0x100)]);
    w.prop(b"compatible", b"ns16550a\0");
    w.end();
    for _ in 0..f.extra_depth {
        w.begin(b"bus");
    }
    for _ in 0..f.extra_depth {
        w.end();
    }
    w.end();
    w.end();
    w.u32(FDT_END);
    if !w.ok {
        return 0;
    }
    let st_len = w.sl;
    let strs = w.strs;
    let tl = w.tl;
    let str_off = st_off + st_len;
    let total = str_off + tl;
    if total > out.len() {
        return 0;
    }
    let (_, rest) = out.split_at_mut(str_off);
    rest[..tl].copy_from_slice(&strs[..tl]);
    let hdr: [u32; 10] = [
        FDT_MAGIC,
        total as u32,
        st_off as u32,
        str_off as u32,
        rsv_off as u32,
        FDT_VERSION,
        16,
        0,
        tl as u32,
        st_len as u32,
    ];
    for (i, v) in hdr.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    let mut o = rsv_off;
    for &(a, s) in f.memreserve.iter().chain(core::iter::once(&(0, 0))) {
        out[o..o + 8].copy_from_slice(&a.to_be_bytes());
        out[o + 8..o + 16].copy_from_slice(&s.to_be_bytes());
        o += 16;
    }
    total
}

fn put32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_be_bytes());
}

fn get32(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// Red-team report: one malformed blob per [`FdtError`] variant, each refused
/// with exactly that variant, plus the valid riscv fixture admitted with the
/// expected usable range. Sell lines `[redteam] attack=fdt-* result=refused`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FdtRefusalReport {
    pub valid_ok: bool,
    pub bad_magic: bool,
    pub truncated: bool,
    pub bad_version: bool,
    pub too_large: bool,
    pub bad_offset: bool,
    pub bad_token: bool,
    pub bad_string: bool,
    pub too_deep: bool,
    pub bad_cells: bool,
    pub bad_region: bool,
    pub no_memory: bool,
    pub too_many_regions: bool,
}

impl FdtRefusalReport {
    pub fn all_ok(&self) -> bool {
        self.valid_ok
            && self.bad_magic
            && self.truncated
            && self.bad_version
            && self.too_large
            && self.bad_offset
            && self.bad_token
            && self.bad_string
            && self.too_deep
            && self.bad_cells
            && self.bad_region
            && self.no_memory
            && self.too_many_regions
    }
}

fn refuses_with(f: &VirtFixture, edit: impl Fn(&mut [u8], usize) -> usize, want: FdtError) -> bool {
    let mut buf = [0u8; FIXTURE_CAP];
    let n = build_virt_fixture(f, &mut buf);
    if n == 0 {
        return false;
    }
    let len = edit(&mut buf, n);
    parse_fdt_memory(&buf[..len]) == Err(want)
}

/// Run every FDT refuse path against a QEMU-virt-shaped fixture.
pub fn run_fdt_refusal_demo() -> FdtRefusalReport {
    let rv = VirtFixture::riscv();
    let same = |_: &mut [u8], n: usize| n;

    let valid_ok = {
        let mut buf = [0u8; FIXTURE_CAP];
        let n = build_virt_fixture(&rv, &mut buf);
        match parse_fdt_memory(&buf[..n]) {
            Ok(m) => {
                m.n_memory == 1
                    && m.memory_bytes == 0x0800_0000
                    && m.n_reserved == 1
                    && m.map.regions() == [PhysRegion { start: 0x8004_0000, end: 0x8800_0000 }]
            }
            Err(_) => false,
        }
    };
    let bad_magic = refuses_with(&rv, |b, n| { b[0] ^= 0xff; n }, FdtError::BadMagic);
    let truncated = refuses_with(&rv, |_, n| n - 1, FdtError::Truncated);
    let bad_version = refuses_with(&rv, |b, n| { put32(b, 20, 16); n }, FdtError::BadVersion);
    let too_large = refuses_with(
        &rv,
        |b, n| { put32(b, 4, (FDT_MAX_TOTALSIZE + 4) as u32); n },
        FdtError::TooLarge,
    );
    // Strings block pushed past totalsize.
    let bad_offset = refuses_with(&rv, |b, n| { put32(b, 12, n as u32); n }, FdtError::BadOffset);
    // FDT_END (last struct word) replaced by an undefined token.
    let bad_token = refuses_with(
        &rv,
        |b, n| {
            let end = (get32(b, 8) + get32(b, 36)) as usize;
            put32(b, end - 4, 5);
            n
        },
        FdtError::BadToken,
    );
    // Root's first property (#address-cells) names a string past the block.
    let bad_string = refuses_with(
        &rv,
        |b, n| {
            let st = get32(b, 8) as usize;
            put32(b, st + 16, 0x00ff_0000);
            n
        },
        FdtError::BadString,
    );
    let too_deep = refuses_with(
        &VirtFixture { extra_depth: FDT_MAX_DEPTH, ..rv },
        same,
        FdtError::TooDeep,
    );
    let bad_cells = refuses_with(&VirtFixture { root_address_cells: 3, ..rv }, same, FdtError::BadCells);
    let bad_region = refuses_with(
        &VirtFixture { memory: &[(0xffff_ffff_ffff_f000, 0x2000)], ..rv },
        same,
        FdtError::BadRegion,
    );
    let no_memory = refuses_with(&VirtFixture { with_memory_node: false, ..rv }, same, FdtError::NoMemory);
    let many = [
        (0x8000_0000u64, 0x1000u64), (0x8001_0000, 0x1000), (0x8002_0000, 0x1000),
        (0x8003_0000, 0x1000), (0x8004_0000, 0x1000), (0x8005_0000, 0x1000),
        (0x8006_0000, 0x1000), (0x8007_0000, 0x1000), (0x8008_0000, 0x1000),
        (0x8009_0000, 0x1000), (0x800a_0000, 0x1000), (0x800b_0000, 0x1000),
        (0x800c_0000, 0x1000), (0x800d_0000, 0x1000), (0x800e_0000, 0x1000),
        (0x800f_0000, 0x1000), (0x8010_0000, 0x1000),
    ];
    let too_many_regions = refuses_with(
        &VirtFixture { memory: &many, reserved_memory: &[], ..rv },
        same,
        FdtError::TooManyRegions,
    );
    FdtRefusalReport {
        valid_ok,
        bad_magic,
        truncated,
        bad_version,
        too_large,
        bad_offset,
        bad_token,
        bad_string,
        too_deep,
        bad_cells,
        bad_region,
        no_memory,
        too_many_regions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(f: &VirtFixture) -> ([u8; FIXTURE_CAP], usize) {
        let mut buf = [0u8; FIXTURE_CAP];
        let n = build_virt_fixture(f, &mut buf);
        assert!(n > FDT_HEADER_LEN, "fixture fits");
        (buf, n)
    }

    #[test]
    fn fixture_is_byte_honest() {
        let (b, n) = fixture(&VirtFixture::riscv());
        let h = parse_header(&b[..n]).unwrap();
        assert_eq!(get32(&b, 0), FDT_MAGIC);
        assert_eq!(h.totalsize, n);
        assert_eq!(h.off_mem_rsvmap, 40);
        assert_eq!(h.off_dt_struct, 56, "memreserve terminator only, like QEMU riscv virt");
        assert_eq!(h.off_dt_strings, h.off_dt_struct + h.size_dt_struct);
        assert_eq!(h.off_dt_strings + h.size_dt_strings, n);
        assert_eq!((h.version, h.last_comp_version), (17, 16));
        // First struct word is BEGIN_NODE for the root, last is END.
        assert_eq!(get32(&b, h.off_dt_struct), FDT_BEGIN_NODE);
        assert_eq!(get32(&b, h.off_dt_struct + h.size_dt_struct - 4), FDT_END);
        // Strings block is NUL-separated and starts with the first prop name.
        assert!(b[h.off_dt_strings..n].starts_with(b"#address-cells\0#size-cells\0"));
    }

    #[test]
    fn riscv_virt_fixture_parses() {
        let (b, n) = fixture(&VirtFixture::riscv());
        let m = parse_fdt_memory(&b[..n]).unwrap();
        assert_eq!(m.map.source, MapSource::Fdt);
        assert_eq!(m.n_memory, 1);
        assert_eq!(m.memory_bytes, 128 << 20);
        assert_eq!(m.reserved(), &[PhysRegion { start: 0x8000_0000, end: 0x8004_0000 }]);
        assert_eq!(m.n_memreserve, 0);
        assert_eq!(m.map.regions(), &[PhysRegion { start: 0x8004_0000, end: 0x8800_0000 }]);
        assert_eq!(m.usable_bytes(), (128 << 20) - 0x4_0000);
        // Trailing bytes past totalsize are ignored.
        let mut big = [0u8; FIXTURE_CAP];
        big[..n].copy_from_slice(&b[..n]);
        assert_eq!(parse_fdt_memory(&big).unwrap(), m);
    }

    #[test]
    fn aarch64_virt_shape_parses_with_cells_1_1() {
        let f = VirtFixture {
            root_address_cells: 1,
            root_size_cells: 1,
            memory: &[(0x4000_0000, 0x0800_0000)],
            reserved_memory: &[],
            ..VirtFixture::riscv()
        };
        let (b, n) = fixture(&f);
        let m = parse_fdt_memory(&b[..n]).unwrap();
        assert_eq!(m.map.regions(), &[PhysRegion { start: 0x4000_0000, end: 0x4800_0000 }]);
        assert_eq!(m.n_reserved, 0);
    }

    #[test]
    fn memreserve_and_multi_bank_split() {
        let f = VirtFixture {
            memory: &[(0x8000_0000, 0x0400_0000), (0x8400_0000, 0x0400_0000), (0x1_0000_0000, 0x1000_0000)],
            memreserve: &[(0x8100_0000, 0x1000)],
            ..VirtFixture::riscv()
        };
        let (b, n) = fixture(&f);
        let m = parse_fdt_memory(&b[..n]).unwrap();
        assert_eq!(m.n_memory, 3);
        assert_eq!(m.n_memreserve, 1);
        assert_eq!(m.n_reserved, 2);
        let mut r = [PhysRegion { start: 0, end: 0 }; 3];
        r.copy_from_slice(m.map.regions());
        r.sort_by_key(|x| x.start);
        assert_eq!(
            r,
            [
                PhysRegion { start: 0x8004_0000, end: 0x8100_0000 },
                PhysRegion { start: 0x8100_1000, end: 0x8800_0000 },
                PhysRegion { start: 0x1_0000_0000, end: 0x1_1000_0000 },
            ]
        );
    }

    #[test]
    fn kernel_plan_matches_riscv_fallback_window_minus_dtb() {
        let (b, n) = fixture(&VirtFixture::riscv());
        let m = parse_fdt_memory(&b[..n]).unwrap();
        // OpenSBI's Next Arg1 on QEMU 10 riscv virt -m 128M.
        let plan = plan_fdt_frames(&m, 0x87e0_0000, 0x8100_0000, 0x1_0000_0000).unwrap();
        let mut r = [PhysRegion { start: 0, end: 0 }; 2];
        r.copy_from_slice(plan.regions());
        r.sort_by_key(|x| x.start);
        assert_eq!(
            r,
            [
                PhysRegion { start: 0x8100_0000, end: 0x87e0_0000 },
                PhysRegion { start: 0x87e0_1000, end: 0x8800_0000 },
            ]
        );
        // aarch64: DTB at RAM base sits below the floor; limit is the 2 GiB
        // top of the Normal identity block; a 2 GiB guest is clipped there.
        let f = VirtFixture {
            memory: &[(0x4000_0000, 0x8000_0000)],
            reserved_memory: &[],
            ..VirtFixture::riscv()
        };
        let (b, n) = fixture(&f);
        let m = parse_fdt_memory(&b[..n]).unwrap();
        let plan = plan_fdt_frames(&m, 0x4000_0000, 0x4100_0000, 0x8000_0000).unwrap();
        assert_eq!(
            plan.regions(),
            &[PhysRegion { start: 0x4100_0000, end: 0x4100_0000 + crate::mmap::FRAME_CAP_BYTES }]
        );
    }

    #[test]
    fn subtract_refuses_split_past_capacity() {
        let mut map = MemoryMap::empty(MapSource::Fdt);
        for i in 0..MAX_REGIONS as u64 {
            push_usable(&mut map, i * 0x10_0000, 0x8_0000);
        }
        assert_eq!(map.n_usable, MAX_REGIONS);
        let before = map;
        assert_eq!(
            subtract(&mut map, PhysRegion { start: 0x1000, end: 0x2000 }),
            Err(FdtError::TooManyRegions)
        );
        assert_eq!(map, before, "refused split leaves the map untouched");
        // Trimming an edge or removing a whole region needs no slot.
        subtract(&mut map, PhysRegion { start: 0, end: 0x1000 }).unwrap();
        subtract(&mut map, PhysRegion { start: 0x10_0000, end: 0x18_0000 }).unwrap();
        assert_eq!(map.n_usable, MAX_REGIONS - 1);
    }

    // ---- one test per refusal -------------------------------------------

    fn edit(f: &VirtFixture, e: impl Fn(&mut [u8], usize) -> usize) -> Result<FdtMemory, FdtError> {
        let (mut b, n) = fixture(f);
        let len = e(&mut b, n);
        parse_fdt_memory(&b[..len])
    }

    #[test]
    fn refuses_bad_magic() {
        let r = edit(&VirtFixture::riscv(), |b, n| { b[3] = 0xee; n });
        assert_eq!(r, Err(FdtError::BadMagic));
    }

    #[test]
    fn refuses_truncated() {
        assert_eq!(parse_fdt_memory(&[]), Err(FdtError::Truncated));
        assert_eq!(parse_header(&[0xd0, 0x0d, 0xfe, 0xed]), Err(FdtError::Truncated));
        let r = edit(&VirtFixture::riscv(), |_, n| n - 1);
        assert_eq!(r, Err(FdtError::Truncated));
        // size_dt_struct cut short: the walk runs off the struct block.
        let r = edit(&VirtFixture::riscv(), |b, n| {
            let s = get32(b, 36);
            put32(b, 36, s - 8);
            n
        });
        assert_eq!(r, Err(FdtError::Truncated));
    }

    #[test]
    fn refuses_bad_version() {
        assert_eq!(edit(&VirtFixture::riscv(), |b, n| { put32(b, 20, 16); n }), Err(FdtError::BadVersion));
        assert_eq!(edit(&VirtFixture::riscv(), |b, n| { put32(b, 24, 18); n }), Err(FdtError::BadVersion));
    }

    #[test]
    fn refuses_too_large() {
        let r = edit(&VirtFixture::riscv(), |b, n| { put32(b, 4, u32::MAX); n });
        assert_eq!(r, Err(FdtError::TooLarge));
    }

    #[test]
    fn refuses_bad_offset() {
        let rv = VirtFixture::riscv();
        assert_eq!(edit(&rv, |b, n| { put32(b, 12, n as u32); n }), Err(FdtError::BadOffset));
        assert_eq!(edit(&rv, |b, n| { put32(b, 8, 57); n }), Err(FdtError::BadOffset), "misaligned struct");
        assert_eq!(edit(&rv, |b, n| { put32(b, 16, 8); n }), Err(FdtError::BadOffset), "rsvmap in header");
        assert_eq!(edit(&rv, |b, n| { put32(b, 4, 16); n }), Err(FdtError::BadOffset), "totalsize < header");
        assert_eq!(edit(&rv, |b, n| { put32(b, 36, u32::MAX); n }), Err(FdtError::BadOffset));
    }

    #[test]
    fn refuses_bad_token() {
        let rv = VirtFixture::riscv();
        // Unknown token in place of FDT_END.
        let r = edit(&rv, |b, n| {
            let end = (get32(b, 8) + get32(b, 36)) as usize;
            put32(b, end - 4, 0x7);
            n
        });
        assert_eq!(r, Err(FdtError::BadToken));
        // FDT_END in place of the root's first property: END before root closes.
        let r = edit(&rv, |b, n| {
            let st = get32(b, 8) as usize;
            put32(b, st + 8, FDT_END);
            n
        });
        assert_eq!(r, Err(FdtError::BadToken));
        // END_NODE as the very first token: unbalanced.
        let r = edit(&rv, |b, n| {
            let st = get32(b, 8) as usize;
            put32(b, st, FDT_END_NODE);
            n
        });
        assert_eq!(r, Err(FdtError::BadToken));
    }

    #[test]
    fn refuses_bad_string() {
        let rv = VirtFixture::riscv();
        let r = edit(&rv, |b, n| {
            let st = get32(b, 8) as usize;
            put32(b, st + 16, 0x00ff_0000);
            n
        });
        assert_eq!(r, Err(FdtError::BadString));
        // Strings block shrunk so the last name loses its NUL.
        let r = edit(&rv, |b, n| {
            let s = get32(b, 32);
            put32(b, 32, s - 1);
            n
        });
        assert_eq!(r, Err(FdtError::BadString));
    }

    #[test]
    fn refuses_too_deep() {
        let ok = VirtFixture { extra_depth: FDT_MAX_DEPTH - 2, ..VirtFixture::riscv() };
        assert!(edit(&ok, |_, n| n).is_ok(), "exactly at the bound admits");
        let deep = VirtFixture { extra_depth: FDT_MAX_DEPTH - 1, ..VirtFixture::riscv() };
        assert_eq!(edit(&deep, |_, n| n), Err(FdtError::TooDeep));
    }

    #[test]
    fn refuses_bad_cells() {
        let rv = VirtFixture::riscv();
        assert_eq!(edit(&VirtFixture { root_address_cells: 3, ..rv }, |_, n| n), Err(FdtError::BadCells));
        assert_eq!(edit(&VirtFixture { root_size_cells: 0, ..rv }, |_, n| n), Err(FdtError::BadCells));
        // Root #size-cells patched 2 -> 1: the 16-byte memory reg is no
        // longer a multiple of (2 + 1) * 4.
        let r = edit(&rv, |b, n| {
            let st = get32(b, 8) as usize;
            assert_eq!(get32(b, st + 36), 2, "root #size-cells value");
            put32(b, st + 36, 1);
            n
        });
        assert_eq!(r, Err(FdtError::BadCells));
        // #address-cells property that is not 4 bytes.
        let r = edit(&rv, |b, n| {
            let st = get32(b, 8) as usize;
            put32(b, st + 12, 0);
            n
        });
        assert_eq!(r, Err(FdtError::BadCells));
    }

    #[test]
    fn refuses_bad_region() {
        let f = VirtFixture { memory: &[(u64::MAX - 0xfff, 0x2000)], ..VirtFixture::riscv() };
        assert_eq!(edit(&f, |_, n| n), Err(FdtError::BadRegion));
        let f = VirtFixture { memreserve: &[(u64::MAX, 2)], ..VirtFixture::riscv() };
        assert_eq!(edit(&f, |_, n| n), Err(FdtError::BadRegion));
    }

    #[test]
    fn refuses_no_memory() {
        let rv = VirtFixture::riscv();
        assert_eq!(edit(&VirtFixture { with_memory_node: false, ..rv }, |_, n| n), Err(FdtError::NoMemory));
        // Memory present but entirely reserved.
        let f = VirtFixture { reserved_memory: &[(0x8000_0000, 0x0800_0000)], ..rv };
        assert_eq!(edit(&f, |_, n| n), Err(FdtError::NoMemory));
        // Zero-size reg entries only.
        let f = VirtFixture { memory: &[(0x8000_0000, 0)], ..rv };
        assert_eq!(edit(&f, |_, n| n), Err(FdtError::NoMemory));
    }

    #[test]
    fn refuses_too_many_regions() {
        let mut regs = [(0u64, 0u64); MAX_REGIONS + 1];
        for (i, r) in regs.iter_mut().enumerate() {
            *r = (0x8000_0000 + (i as u64) * 0x10_0000, 0x1000);
        }
        let f = VirtFixture { memory: &regs, reserved_memory: &[], ..VirtFixture::riscv() };
        assert_eq!(edit(&f, |_, n| n), Err(FdtError::TooManyRegions));
        let f = VirtFixture { memory: &regs[..MAX_REGIONS], reserved_memory: &[], ..VirtFixture::riscv() };
        assert_eq!(edit(&f, |_, n| n).unwrap().map.n_usable, MAX_REGIONS, "exactly MAX_REGIONS admits");
        let mut rsv = [(0u64, 0u64); FDT_MAX_RESERVED + 1];
        for (i, r) in rsv.iter_mut().enumerate() {
            *r = (0x9000_0000 + (i as u64) * 0x1000, 0x1000);
        }
        let f = VirtFixture { memreserve: &rsv, ..VirtFixture::riscv() };
        assert_eq!(edit(&f, |_, n| n), Err(FdtError::TooManyRegions));
    }

    #[test]
    fn red_team_demo_all_refused() {
        let r = run_fdt_refusal_demo();
        assert!(r.all_ok(), "{r:?}");
    }

    #[test]
    fn names_are_stable() {
        for (e, s) in [
            (FdtError::Truncated, "Truncated"),
            (FdtError::BadMagic, "BadMagic"),
            (FdtError::TooManyRegions, "TooManyRegions"),
        ] {
            assert_eq!(e.name(), s);
        }
    }

    // ---- mutation sweep -------------------------------------------------

    /// Every truncation and every single-bit flip of the fixture: never a
    /// panic (debug overflow checks are on under `cargo test`), every
    /// truncation refuses as Truncated / header error, and every accepted
    /// mutant still yields a well-formed map (non-empty, ordered ranges,
    /// within MAX_REGIONS).
    #[test]
    fn mutation_sweep_never_panics() {
        let (b, n) = fixture(&VirtFixture::riscv());
        let mut accepted = 0usize;
        let mut refused = 0usize;
        for len in 0..n {
            match parse_fdt_memory(&b[..len]) {
                Err(FdtError::Truncated) => refused += 1,
                other => panic!("truncation to {len} gave {other:?}"),
            }
        }
        for byte in 0..n {
            for bit in 0..8 {
                let mut m = b;
                m[byte] ^= 1 << bit;
                match parse_fdt_memory(&m[..n]) {
                    Ok(mem) => {
                        accepted += 1;
                        assert!(!mem.map.is_empty());
                        assert!(mem.map.n_usable <= MAX_REGIONS);
                        assert!(mem.map.regions().iter().all(|r| r.end > r.start));
                    }
                    Err(_) => refused += 1,
                }
            }
        }
        assert_eq!(accepted + refused, n + n * 8);
        assert!(refused > n, "most header / token flips refuse");
    }
}
