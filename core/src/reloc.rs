//! x86_64 kernel PIE reloc table (`R_X86_64_RELATIVE`).
//!
//! The kernel is linked at [`KERNEL_TEXT_VA`](crate::KERNEL_TEXT_VA) as a
//! static-PIE (`relocation-model=pic`). `.rela.dyn` stays in the image.
//! The trampoline writes `addend + slide` through the identity map, then
//! unmaps the unused canonical HH alias when the slide is non-zero.
//!
//! Host twin of `boot/x86_64/trampoline.S` (`apply_pie_relocs`). Not a
//! user-ELF relocator (`core::elf` still rejects `ET_DYN`).

use crate::aspace::{kaslr_slide_valid, KERNEL_LMA, KERNEL_VMA};

/// ELF64 `R_X86_64_RELATIVE` (System V AMD64 ABI).
pub const R_X86_64_RELATIVE: u32 = 8;
/// `sizeof(Elf64_Rela)`.
pub const RELA64_SIZE: usize = 24;
/// Trailer magic at the end of `kernel.bin` (`AETE` / Aether reloc).
pub const PIE_RELOC_MAGIC: u32 = 0xAE7E_4E1C;
/// Bytes appended after the objcopy blob.
pub const PIE_TRAILER_SIZE: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rela64 {
    pub offset: u64,
    pub info: u64,
    pub addend: i64,
}

impl Rela64 {
    pub fn kind(self) -> u32 {
        (self.info & 0xFFFF_FFFF) as u32
    }

    /// Linked VA → offset into the objcopy blob (starts at [`KERNEL_LMA`]).
    pub fn image_offset(self) -> Option<usize> {
        let lma = self.offset.checked_sub(KERNEL_VMA)?;
        let off = lma.checked_sub(KERNEL_LMA)?;
        Some(off as usize)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelocError {
    Truncated,
    BadMagic,
    OutOfImage,
    Unsupported,
    BadSlide,
}

/// Parse one `Elf64_Rela` (little-endian).
pub fn parse_rela64(bytes: &[u8]) -> Result<Rela64, RelocError> {
    if bytes.len() < RELA64_SIZE {
        return Err(RelocError::Truncated);
    }
    Ok(Rela64 {
        offset: u64::from_le_bytes(bytes[0..8].try_into().unwrap()),
        info: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
        addend: i64::from_le_bytes(bytes[16..24].try_into().unwrap()),
    })
}

/// Trailer: `magic, count, rela_off, entsize` (all little-endian u32).
///
/// `rela_off` is the objcopy offset of `.rela.dyn`.
pub fn parse_pie_trailer(image: &[u8]) -> Result<(u32, u32), RelocError> {
    if image.len() < PIE_TRAILER_SIZE {
        return Err(RelocError::Truncated);
    }
    let t = &image[image.len() - PIE_TRAILER_SIZE..];
    let magic = u32::from_le_bytes(t[0..4].try_into().unwrap());
    if magic != PIE_RELOC_MAGIC {
        return Err(RelocError::BadMagic);
    }
    let count = u32::from_le_bytes(t[4..8].try_into().unwrap());
    let rela_off = u32::from_le_bytes(t[8..12].try_into().unwrap());
    let entsize = u32::from_le_bytes(t[12..16].try_into().unwrap());
    if entsize != RELA64_SIZE as u32 {
        return Err(RelocError::Unsupported);
    }
    let need = rela_off as usize + count as usize * RELA64_SIZE;
    if need + PIE_TRAILER_SIZE > image.len() {
        return Err(RelocError::Truncated);
    }
    Ok((count, rela_off))
}

/// `*r_offset = addend + slide` for each `R_X86_64_RELATIVE`.
///
/// `image` is the objcopy blob (LMA `0x400000`). Sites must fall inside it.
pub fn apply_rela_dyn(image: &mut [u8], slide: u64, relas: &[Rela64]) -> Result<u32, RelocError> {
    if !kaslr_slide_valid(slide) {
        return Err(RelocError::BadSlide);
    }
    let mut n = 0u32;
    for r in relas {
        if r.kind() != R_X86_64_RELATIVE {
            return Err(RelocError::Unsupported);
        }
        let Some(off) = r.image_offset() else {
            return Err(RelocError::OutOfImage);
        };
        let end = off.checked_add(8).ok_or(RelocError::OutOfImage)?;
        if end > image.len() {
            return Err(RelocError::OutOfImage);
        }
        let val = (r.addend as u64).wrapping_add(slide);
        image[off..end].copy_from_slice(&val.to_le_bytes());
        n += 1;
    }
    Ok(n)
}

/// Apply a raw `.rela.dyn` blob (array of `Elf64_Rela`).
pub fn apply_rela_bytes(image: &mut [u8], slide: u64, rela: &[u8]) -> Result<u32, RelocError> {
    if rela.len() % RELA64_SIZE != 0 {
        return Err(RelocError::Truncated);
    }
    let mut n = 0u32;
    let mut i = 0;
    while i < rela.len() {
        let r = parse_rela64(&rela[i..])?;
        n += apply_rela_dyn(image, slide, &[r])?;
        i += RELA64_SIZE;
    }
    Ok(n)
}

/// Apply using a packed trailer that names `.rela.dyn` inside `image`.
pub fn apply_pie_image(image: &mut [u8], slide: u64) -> Result<u32, RelocError> {
    let (count, rela_off) = parse_pie_trailer(image)?;
    let mut n = 0u32;
    for i in 0..count {
        let start = rela_off as usize + i as usize * RELA64_SIZE;
        let mut buf = [0u8; RELA64_SIZE];
        buf.copy_from_slice(&image[start..start + RELA64_SIZE]);
        let r = parse_rela64(&buf)?;
        n += apply_rela_dyn(image, slide, &[r])?;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aspace::{KASLR_SLIDE_STRIDE, KERNEL_TEXT_VA};

    fn site(va: u64, addend: u64) -> Rela64 {
        Rela64 {
            offset: va,
            info: R_X86_64_RELATIVE as u64,
            addend: addend as i64,
        }
    }

    #[test]
    fn relative_writes_addend_plus_slide() {
        let mut image = [0u8; 32];
        let r = site(KERNEL_TEXT_VA + 8, KERNEL_TEXT_VA);
        assert_eq!(
            apply_rela_dyn(&mut image, KASLR_SLIDE_STRIDE, &[r]).unwrap(),
            1
        );
        let v = u64::from_le_bytes(image[8..16].try_into().unwrap());
        assert_eq!(v, KERNEL_TEXT_VA + KASLR_SLIDE_STRIDE);
    }

    #[test]
    fn slide_zero_writes_linked_va() {
        let mut image = [0u8; 16];
        let r = site(KERNEL_TEXT_VA, 0xFFFF_FFFF_8040_0100);
        assert_eq!(apply_rela_dyn(&mut image, 0, &[r]).unwrap(), 1);
        let v = u64::from_le_bytes(image[0..8].try_into().unwrap());
        assert_eq!(v, 0xFFFF_FFFF_8040_0100);
    }

    #[test]
    fn trailer_roundtrip_and_apply() {
        let mut image = vec![0u8; 64];
        let rela = {
            let r = site(KERNEL_TEXT_VA + 16, KERNEL_TEXT_VA + 32);
            let mut b = [0u8; RELA64_SIZE];
            b[0..8].copy_from_slice(&r.offset.to_le_bytes());
            b[8..16].copy_from_slice(&r.info.to_le_bytes());
            b[16..24].copy_from_slice(&r.addend.to_le_bytes());
            b
        };
        image[32..56].copy_from_slice(&rela);
        let mut trailer = [0u8; PIE_TRAILER_SIZE];
        trailer[0..4].copy_from_slice(&PIE_RELOC_MAGIC.to_le_bytes());
        trailer[4..8].copy_from_slice(&1u32.to_le_bytes());
        trailer[8..12].copy_from_slice(&32u32.to_le_bytes());
        trailer[12..16].copy_from_slice(&(RELA64_SIZE as u32).to_le_bytes());
        image.extend_from_slice(&trailer);
        assert_eq!(parse_pie_trailer(&image).unwrap(), (1, 32));
        assert_eq!(
            apply_pie_image(&mut image, KASLR_SLIDE_STRIDE).unwrap(),
            1
        );
        let v = u64::from_le_bytes(image[16..24].try_into().unwrap());
        assert_eq!(v, KERNEL_TEXT_VA + 32 + KASLR_SLIDE_STRIDE);
    }

    #[test]
    fn rejects_non_relative_and_oob() {
        let mut image = [0u8; 16];
        let bad = Rela64 {
            offset: KERNEL_TEXT_VA,
            info: 1,
            addend: 0,
        };
        assert_eq!(
            apply_rela_dyn(&mut image, 0, &[bad]),
            Err(RelocError::Unsupported)
        );
        let oob = site(KERNEL_TEXT_VA + 64, KERNEL_TEXT_VA);
        assert_eq!(
            apply_rela_dyn(&mut image, 0, &[oob]),
            Err(RelocError::OutOfImage)
        );
        assert_eq!(
            apply_rela_dyn(&mut image, 0x200000, &[site(KERNEL_TEXT_VA, 0)]),
            Err(RelocError::BadSlide)
        );
    }
}
