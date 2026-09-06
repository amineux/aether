//! Minimal ELF64 static-executable parser (no relocations).
//!
//! `/init` is a **static non-PIE** `ET_EXEC` for `EM_X86_64`. The kernel
//! copies `PT_LOAD` segments to their `p_vaddr` (identity-mapped user
//! window). PIE / `ET_DYN` is rejected so we do not invent a relocator.

pub const ELFMAG: [u8; 4] = [0x7F, b'E', b'L', b'F'];
pub const ELFCLASS64: u8 = 2;
pub const ELFDATA2LSB: u8 = 1;
pub const ELFOSABI_NONE: u8 = 0;
pub const ET_EXEC: u16 = 2;
pub const EM_X86_64: u16 = 62;
pub const PT_LOAD: u32 = 1;
pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

pub const EI_CLASS: usize = 4;
pub const EI_DATA: usize = 5;
pub const EI_VERSION: usize = 6;
pub const EI_OSABI: usize = 7;

pub const EHDR_SIZE: usize = 64;
pub const PHDR_SIZE: usize = 56;
pub const MAX_LOADS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElfError {
    Truncated,
    BadMagic,
    Not64,
    NotLe,
    NotExec,
    BadMachine,
    BadVersion,
    BadPhdr,
    TooManyLoads,
    BadSegment,
    Empty,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoadSeg {
    pub vaddr: u64,
    pub memsz: u64,
    pub filesz: u64,
    pub offset: u64,
    pub flags: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ElfImage {
    pub entry: u64,
    pub n_loads: usize,
    pub loads: [LoadSeg; MAX_LOADS],
}

impl ElfImage {
    pub fn loads(&self) -> &[LoadSeg] {
        &self.loads[..self.n_loads]
    }

    pub fn load_span(&self) -> Option<(u64, u64)> {
        let mut lo = u64::MAX;
        let mut hi = 0u64;
        if self.n_loads == 0 {
            return None;
        }
        for s in self.loads() {
            lo = lo.min(s.vaddr);
            hi = hi.max(s.vaddr.saturating_add(s.memsz));
        }
        Some((lo, hi))
    }
}

fn r16(b: &[u8], off: usize) -> Result<u16, ElfError> {
    let s = b.get(off..off + 2).ok_or(ElfError::Truncated)?;
    Ok(u16::from_le_bytes([s[0], s[1]]))
}

fn r32(b: &[u8], off: usize) -> Result<u32, ElfError> {
    let s = b.get(off..off + 4).ok_or(ElfError::Truncated)?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn r64(b: &[u8], off: usize) -> Result<u64, ElfError> {
    let s = b.get(off..off + 8).ok_or(ElfError::Truncated)?;
    Ok(u64::from_le_bytes([
        s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
    ]))
}

/// Parse a static ELF64 executable. Does not relocate.
pub fn parse_elf64(bytes: &[u8]) -> Result<ElfImage, ElfError> {
    if bytes.len() < EHDR_SIZE {
        return Err(ElfError::Truncated);
    }
    if bytes[0..4] != ELFMAG {
        return Err(ElfError::BadMagic);
    }
    if bytes[EI_CLASS] != ELFCLASS64 {
        return Err(ElfError::Not64);
    }
    if bytes[EI_DATA] != ELFDATA2LSB {
        return Err(ElfError::NotLe);
    }
    if bytes[EI_VERSION] != 1 {
        return Err(ElfError::BadVersion);
    }
    let e_type = r16(bytes, 16)?;
    if e_type != ET_EXEC {
        return Err(ElfError::NotExec);
    }
    let e_machine = r16(bytes, 18)?;
    if e_machine != EM_X86_64 {
        return Err(ElfError::BadMachine);
    }
    let e_version = r32(bytes, 20)?;
    if e_version != 1 {
        return Err(ElfError::BadVersion);
    }
    let entry = r64(bytes, 24)?;
    let phoff = r64(bytes, 32)?;
    let ehsize = r16(bytes, 52)?;
    let phentsize = r16(bytes, 54)?;
    let phnum = r16(bytes, 56)?;
    if ehsize as usize != EHDR_SIZE || phentsize as usize != PHDR_SIZE {
        return Err(ElfError::BadPhdr);
    }
    if phnum == 0 {
        return Err(ElfError::Empty);
    }
    if phnum as usize > 32 {
        return Err(ElfError::BadPhdr);
    }

    let mut image = ElfImage {
        entry,
        n_loads: 0,
        loads: [LoadSeg {
            vaddr: 0,
            memsz: 0,
            filesz: 0,
            offset: 0,
            flags: 0,
        }; MAX_LOADS],
    };

    for i in 0..phnum as usize {
        let off = phoff
            .checked_add((i * PHDR_SIZE) as u64)
            .ok_or(ElfError::Truncated)? as usize;
        if bytes.len() < off + PHDR_SIZE {
            return Err(ElfError::Truncated);
        }
        let p_type = r32(bytes, off)?;
        if p_type != PT_LOAD {
            continue;
        }
        if image.n_loads >= MAX_LOADS {
            return Err(ElfError::TooManyLoads);
        }
        let flags = r32(bytes, off + 4)?;
        let offset = r64(bytes, off + 8)?;
        let vaddr = r64(bytes, off + 16)?;
        let filesz = r64(bytes, off + 32)?;
        let memsz = r64(bytes, off + 40)?;
        if memsz < filesz {
            return Err(ElfError::BadSegment);
        }
        let file_end = offset.checked_add(filesz).ok_or(ElfError::BadSegment)?;
        if file_end > bytes.len() as u64 {
            return Err(ElfError::BadSegment);
        }
        if vaddr == 0 && memsz > 0 {
            return Err(ElfError::BadSegment);
        }
        image.loads[image.n_loads] = LoadSeg {
            vaddr,
            memsz,
            filesz,
            offset,
            flags,
        };
        image.n_loads += 1;
    }
    if image.n_loads == 0 {
        return Err(ElfError::Empty);
    }
    Ok(image)
}

/// True if every load segment (and the entry) sits in `[base, end)`.
pub fn loads_in_window(image: &ElfImage, base: u64, end: u64) -> bool {
    if image.entry < base || image.entry >= end {
        return false;
    }
    for s in image.loads() {
        if s.vaddr < base {
            return false;
        }
        match s.vaddr.checked_add(s.memsz) {
            Some(seg_end) if seg_end <= end => {}
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write16(buf: &mut [u8], off: usize, v: u16) {
        buf[off..off + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn write32(buf: &mut [u8], off: usize, v: u32) {
        buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn write64(buf: &mut [u8], off: usize, v: u64) {
        buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
    }

    fn minimal_exec(entry: u64, vaddr: u64, payload: &[u8]) -> [u8; 256] {
        let mut b = [0u8; 256];
        b[0..4].copy_from_slice(&ELFMAG);
        b[EI_CLASS] = ELFCLASS64;
        b[EI_DATA] = ELFDATA2LSB;
        b[EI_VERSION] = 1;
        b[EI_OSABI] = ELFOSABI_NONE;
        write16(&mut b, 16, ET_EXEC);
        write16(&mut b, 18, EM_X86_64);
        write32(&mut b, 20, 1);
        write64(&mut b, 24, entry);
        write64(&mut b, 32, EHDR_SIZE as u64); // e_phoff
        write16(&mut b, 52, EHDR_SIZE as u16);
        write16(&mut b, 54, PHDR_SIZE as u16);
        write16(&mut b, 56, 1);
        // phdr at 64
        write32(&mut b, 64, PT_LOAD);
        write32(&mut b, 68, PF_R | PF_X);
        write64(&mut b, 72, 128); // p_offset
        write64(&mut b, 80, vaddr);
        write64(&mut b, 88, vaddr);
        write64(&mut b, 96, payload.len() as u64);
        write64(&mut b, 104, payload.len() as u64 + 8);
        write64(&mut b, 112, 0x1000);
        b[128..128 + payload.len()].copy_from_slice(payload);
        b
    }

    #[test]
    fn parse_static_exec() {
        let bytes = minimal_exec(0x0200_0100, 0x0200_0000, b"\x90\x90\x90\x90");
        let img = parse_elf64(&bytes).unwrap();
        assert_eq!(img.entry, 0x0200_0100);
        assert_eq!(img.n_loads, 1);
        assert_eq!(img.loads[0].vaddr, 0x0200_0000);
        assert_eq!(img.loads[0].filesz, 4);
        assert_eq!(img.loads[0].memsz, 12);
        assert!(loads_in_window(&img, 0x0200_0000, 0x0220_0000));
    }

    #[test]
    fn reject_pie_dyn() {
        let mut bytes = minimal_exec(0x1000, 0x1000, b"abcd");
        write16(&mut bytes, 16, 3); // ET_DYN
        assert_eq!(parse_elf64(&bytes).unwrap_err(), ElfError::NotExec);
    }

    #[test]
    fn reject_bad_magic() {
        let mut bytes = minimal_exec(0x1000, 0x1000, b"abcd");
        bytes[0] = 0;
        assert_eq!(parse_elf64(&bytes).unwrap_err(), ElfError::BadMagic);
    }

    #[test]
    fn reject_window() {
        let bytes = minimal_exec(0x1000, 0x1000, b"abcd");
        let img = parse_elf64(&bytes).unwrap();
        assert!(!loads_in_window(&img, 0x0200_0000, 0x0220_0000));
    }

    #[test]
    fn reject_truncated() {
        assert_eq!(parse_elf64(&[0x7F, b'E', b'L']), Err(ElfError::Truncated));
    }
}
