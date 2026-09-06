//! AETHFS01 boot disk: named files packed into a raw image.
//!
//! Documented subset — **not** a filesystem, **not** POSIX, **not**
//! GPT/FAT. virtio-blk copies sectors into a reserved window; the
//! kernel [`parse_bootfs`]s this header and [`crate::ramfs::RamFs::seed`]s
//! the same `/init` / `/probe` names the loader already opens.
//!
//! Layout (little-endian):
//!
//! ```text
//! 0x00  magic     8  b"AETHFS01"
//! 0x08  nfiles    4
//! 0x0C  flags     4  must be 0
//! 0x10  entries[n]
//!         name    16  '/' + [A-Za-z0-9._-]+, NUL-padded
//!         offset  4   byte offset from start of image
//!         size    4
//! then file payloads (no overlap; must sit inside the image)
//! ```

use crate::ramfs::{RamFs, RamFsError, INIT_PATH, PROBE_PATH};

/// On-disk magic. Frozen for `scripts/mkbootfs.py` + this parser.
pub const BOOTFS_MAGIC: [u8; 8] = *b"AETHFS01";
pub const BOOTFS_HDR: usize = 16;
pub const BOOTFS_ENT: usize = 24;
pub const BOOTFS_NAME: usize = 16;
pub const BOOTFS_MAX_FILES: usize = 8;
pub const BOOTFS_SECTOR: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootFsError {
    BadMagic,
    Truncated,
    BadFlags,
    BadEntry,
    TooMany,
    Empty,
    NoRoom,
}

/// One named payload borrowed from the image bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootFsFile<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
}

/// Parsed directory. Allocation-free; files borrow `image`.
#[derive(Clone, Copy, Debug)]
pub struct BootFs<'a> {
    files: [Option<BootFsFile<'a>>; BOOTFS_MAX_FILES],
    n: usize,
}

impl<'a> BootFs<'a> {
    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = BootFsFile<'a>> + '_ {
        self.files[..self.n].iter().filter_map(|f| *f)
    }

    pub fn get(&self, name: &str) -> Option<&'a [u8]> {
        self.iter().find(|f| f.name == name).map(|f| f.data)
    }

    /// Seed `/init` (and `/probe` if present). Missing `/init` is an error.
    pub fn seed_ramfs(&self, fs: &mut RamFs<'a>) -> Result<usize, RamFsError> {
        let init = self.get(INIT_PATH).ok_or(RamFsError::NotFound)?;
        fs.seed(INIT_PATH, init)?;
        let mut n = 1;
        if let Some(probe) = self.get(PROBE_PATH) {
            fs.seed(PROBE_PATH, probe)?;
            n += 1;
        }
        Ok(n)
    }
}

/// Parse an AETHFS01 image. Rejects overlap, OOB, empty names, and
/// `flags != 0`.
pub fn parse_bootfs(image: &[u8]) -> Result<BootFs<'_>, BootFsError> {
    if image.len() < BOOTFS_HDR {
        return Err(BootFsError::Truncated);
    }
    if image[..8] != BOOTFS_MAGIC {
        return Err(BootFsError::BadMagic);
    }
    let nfiles = u32::from_le_bytes(image[8..12].try_into().unwrap()) as usize;
    let flags = u32::from_le_bytes(image[12..16].try_into().unwrap());
    if flags != 0 {
        return Err(BootFsError::BadFlags);
    }
    if nfiles == 0 {
        return Err(BootFsError::Empty);
    }
    if nfiles > BOOTFS_MAX_FILES {
        return Err(BootFsError::TooMany);
    }
    let dir_end = BOOTFS_HDR
        .checked_add(nfiles.checked_mul(BOOTFS_ENT).ok_or(BootFsError::Truncated)?)
        .ok_or(BootFsError::Truncated)?;
    if image.len() < dir_end {
        return Err(BootFsError::Truncated);
    }

    let mut files: [Option<BootFsFile<'_>>; BOOTFS_MAX_FILES] = [None; BOOTFS_MAX_FILES];
    let mut spans: [(usize, usize); BOOTFS_MAX_FILES] = [(0, 0); BOOTFS_MAX_FILES];
    for i in 0..nfiles {
        let e = BOOTFS_HDR + i * BOOTFS_ENT;
        let raw_name = &image[e..e + BOOTFS_NAME];
        let name_len = raw_name.iter().position(|&b| b == 0).unwrap_or(BOOTFS_NAME);
        if name_len < 2 {
            return Err(BootFsError::BadEntry);
        }
        let name = core::str::from_utf8(&raw_name[..name_len]).map_err(|_| BootFsError::BadEntry)?;
        let off = u32::from_le_bytes(image[e + 16..e + 20].try_into().unwrap()) as usize;
        let size = u32::from_le_bytes(image[e + 20..e + 24].try_into().unwrap()) as usize;
        let end = off.checked_add(size).ok_or(BootFsError::BadEntry)?;
        if off < dir_end || end > image.len() {
            return Err(BootFsError::BadEntry);
        }
        for j in 0..i {
            let (a0, a1) = spans[j];
            if ranges_overlap(a0, a1, off, end) {
                return Err(BootFsError::BadEntry);
            }
            if files[j].map(|p| p.name) == Some(name) {
                return Err(BootFsError::BadEntry);
            }
        }
        spans[i] = (off, end);
        files[i] = Some(BootFsFile {
            name,
            data: &image[off..end],
        });
    }
    Ok(BootFs { files, n: nfiles })
}

/// Pack named files into `out`. Returns bytes written (padded to a
/// sector so QEMU `-drive format=raw` has a whole number of blocks).
pub fn pack_bootfs(files: &[(&str, &[u8])], out: &mut [u8]) -> Result<usize, BootFsError> {
    if files.is_empty() {
        return Err(BootFsError::Empty);
    }
    if files.len() > BOOTFS_MAX_FILES {
        return Err(BootFsError::TooMany);
    }
    let dir_end = BOOTFS_HDR + files.len() * BOOTFS_ENT;
    let mut cursor = dir_end;
    for (_, data) in files {
        cursor = cursor
            .checked_add(data.len())
            .ok_or(BootFsError::NoRoom)?;
    }
    let padded = align_up(cursor, BOOTFS_SECTOR);
    if out.len() < padded {
        return Err(BootFsError::NoRoom);
    }
    out[..padded].fill(0);
    out[..8].copy_from_slice(&BOOTFS_MAGIC);
    out[8..12].copy_from_slice(&(files.len() as u32).to_le_bytes());
    out[12..16].copy_from_slice(&0u32.to_le_bytes());
    let mut data_off = dir_end;
    for (i, (name, data)) in files.iter().enumerate() {
        if name.len() < 2 || name.len() > BOOTFS_NAME || !name.starts_with('/') {
            return Err(BootFsError::BadEntry);
        }
        let e = BOOTFS_HDR + i * BOOTFS_ENT;
        let nb = name.as_bytes();
        out[e..e + nb.len()].copy_from_slice(nb);
        out[e + 16..e + 20].copy_from_slice(&(data_off as u32).to_le_bytes());
        out[e + 20..e + 24].copy_from_slice(&(data.len() as u32).to_le_bytes());
        out[data_off..data_off + data.len()].copy_from_slice(data);
        data_off += data.len();
    }
    Ok(padded)
}

fn align_up(n: usize, a: usize) -> usize {
    (n + a - 1) & !(a - 1)
}

fn ranges_overlap(a0: usize, a1: usize, b0: usize, b1: usize) -> bool {
    a0 < b1 && b0 < a1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_parse_seed_init_and_probe() {
        let init = b"\x7fELF-init-bytes";
        let probe = b"\x7fELF-probe-bytes";
        let mut img = [0u8; 512];
        let n = pack_bootfs(&[(INIT_PATH, init), (PROBE_PATH, probe)], &mut img).unwrap();
        assert_eq!(n, 512);
        assert_eq!(&img[..8], &BOOTFS_MAGIC);

        let fs_img = parse_bootfs(&img[..n]).unwrap();
        assert_eq!(fs_img.len(), 2);
        assert_eq!(fs_img.get(INIT_PATH), Some(init.as_slice()));
        assert_eq!(fs_img.get(PROBE_PATH), Some(probe.as_slice()));

        let mut ram = RamFs::new();
        assert_eq!(fs_img.seed_ramfs(&mut ram).unwrap(), 2);
        assert_eq!(ram.bytes(INIT_PATH).unwrap(), init);
        assert_eq!(ram.bytes(PROBE_PATH).unwrap(), probe);
    }

    #[test]
    fn pack_parse_init_only() {
        let init = b"just-init";
        let mut img = [0u8; 512];
        let n = pack_bootfs(&[(INIT_PATH, init)], &mut img).unwrap();
        let fs_img = parse_bootfs(&img[..n]).unwrap();
        let mut ram = RamFs::new();
        assert_eq!(fs_img.seed_ramfs(&mut ram).unwrap(), 1);
        assert!(ram.contains(INIT_PATH));
        assert!(!ram.contains(PROBE_PATH));
    }

    #[test]
    fn refuse_bad_magic_and_flags() {
        let mut img = [0u8; 64];
        assert_eq!(parse_bootfs(&img).unwrap_err(), BootFsError::BadMagic);
        img[..8].copy_from_slice(&BOOTFS_MAGIC);
        img[8..12].copy_from_slice(&1u32.to_le_bytes());
        img[12..16].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(parse_bootfs(&img).unwrap_err(), BootFsError::BadFlags);
    }

    #[test]
    fn refuse_truncated_and_oob() {
        assert_eq!(parse_bootfs(b"AETHFS").unwrap_err(), BootFsError::Truncated);
        let mut img = [0u8; 32];
        img[..8].copy_from_slice(&BOOTFS_MAGIC);
        img[8..12].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(parse_bootfs(&img).unwrap_err(), BootFsError::Truncated);

        let mut img = [0u8; 64];
        img[..8].copy_from_slice(&BOOTFS_MAGIC);
        img[8..12].copy_from_slice(&1u32.to_le_bytes());
        img[16] = b'/';
        img[17] = b'a';
        img[32..36].copy_from_slice(&100u32.to_le_bytes());
        img[36..40].copy_from_slice(&8u32.to_le_bytes());
        assert_eq!(parse_bootfs(&img).unwrap_err(), BootFsError::BadEntry);
    }

    #[test]
    fn refuse_empty_and_too_many() {
        let mut img = [0u8; 16];
        img[..8].copy_from_slice(&BOOTFS_MAGIC);
        assert_eq!(parse_bootfs(&img).unwrap_err(), BootFsError::Empty);
        img[8..12].copy_from_slice(&9u32.to_le_bytes());
        assert_eq!(parse_bootfs(&img).unwrap_err(), BootFsError::TooMany);
        assert_eq!(pack_bootfs(&[], &mut img).unwrap_err(), BootFsError::Empty);
    }

    #[test]
    fn refuse_duplicate_names() {
        let mut img = [0u8; 512];
        let n = pack_bootfs(&[(INIT_PATH, b"a"), (INIT_PATH, b"b")], &mut img).unwrap();
        assert_eq!(
            parse_bootfs(&img[..n]).unwrap_err(),
            BootFsError::BadEntry
        );
    }

    #[test]
    fn layout_constants_frozen() {
        assert_eq!(&BOOTFS_MAGIC, b"AETHFS01");
        assert_eq!(BOOTFS_HDR, 16);
        assert_eq!(BOOTFS_ENT, 24);
        assert_eq!(BOOTFS_NAME, 16);
        assert_eq!(BOOTFS_MAX_FILES, 8);
        assert_eq!(BOOTFS_SECTOR, 512);
    }
}
