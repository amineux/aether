//! In-kernel ramfs: named files over borrowed byte slices.
//!
//! Documented subset — **not** POSIX, **not** a block device, **not**
//! virtio-blk. The kernel seeds `/init` (and optional `/probe`) from
//! embedded ELF blobs at boot; the loader uses [`RamFs::open`] /
//! [`RamFs::read`] on those names instead of calling `include_bytes!`
//! at the load site. Host tests cover open/read. No user syscall
//! (0–10 stay as documented in ABI.md).
//!
//! Files are borrowed slices. A later virtio-blk cut can copy blocks
//! into a reserved window and `seed` the same names.

/// Flat namespace only. Enough for `/init`, `/probe`, and a few extras.
pub const MAX_FILES: usize = 8;
/// Path length including the leading `/`.
pub const MAX_NAME: usize = 16;

/// Boot `/init` path the ELF loader opens.
pub const INIT_PATH: &str = "/init";
/// Optional second ELF (`/probe`) on x86.
pub const PROBE_PATH: &str = "/probe";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RamFsError {
    BadName,
    Full,
    Exists,
    NotFound,
    BadFd,
}

/// Index into the file table. Stable for the life of the [`RamFs`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RamFd(u8);

impl RamFd {
    pub fn slot(self) -> usize {
        self.0 as usize
    }
}

/// Open handle: a [`RamFd`] plus a read cursor. `read` advances `offset`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RamHandle {
    fd: RamFd,
    offset: usize,
}

impl RamHandle {
    pub fn fd(self) -> RamFd {
        self.fd
    }

    pub fn offset(self) -> usize {
        self.offset
    }
}

#[derive(Clone, Copy, Debug)]
struct RamFile<'a> {
    name: [u8; MAX_NAME],
    name_len: u8,
    data: &'a [u8],
}

impl<'a> RamFile<'a> {
    fn name(&self) -> &str {
        let n = self.name_len as usize;
        // Names are checked ASCII in `check_name`.
        core::str::from_utf8(&self.name[..n]).unwrap_or("")
    }
}

/// Fixed-capacity named file table. Allocation-free; files borrow bytes.
#[derive(Clone, Copy, Debug)]
pub struct RamFs<'a> {
    files: [Option<RamFile<'a>>; MAX_FILES],
    n: usize,
}

impl<'a> RamFs<'a> {
    pub const fn new() -> Self {
        Self {
            files: [None; MAX_FILES],
            n: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    pub fn contains(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    /// Insert a named file. `name` must be `/` + `[A-Za-z0-9._-]+`.
    pub fn seed(&mut self, name: &str, data: &'a [u8]) -> Result<RamFd, RamFsError> {
        check_name(name)?;
        if self.find(name).is_some() {
            return Err(RamFsError::Exists);
        }
        if self.n >= MAX_FILES {
            return Err(RamFsError::Full);
        }
        let mut stored = [0u8; MAX_NAME];
        let nb = name.as_bytes();
        stored[..nb.len()].copy_from_slice(nb);
        let slot = self.n;
        self.files[slot] = Some(RamFile {
            name: stored,
            name_len: nb.len() as u8,
            data,
        });
        self.n += 1;
        Ok(RamFd(slot as u8))
    }

    /// Open a seeded name. Cursor starts at 0.
    pub fn open(&self, name: &str) -> Result<RamHandle, RamFsError> {
        check_name(name)?;
        let slot = self.find(name).ok_or(RamFsError::NotFound)?;
        Ok(RamHandle {
            fd: RamFd(slot as u8),
            offset: 0,
        })
    }

    pub fn size(&self, fd: RamFd) -> Result<usize, RamFsError> {
        Ok(self.file(fd)?.data.len())
    }

    /// Copy from the handle cursor into `buf`. Returns bytes copied
    /// (0 at EOF). Advances the cursor.
    pub fn read(&self, h: &mut RamHandle, buf: &mut [u8]) -> Result<usize, RamFsError> {
        let data = self.file(h.fd)?.data;
        if h.offset > data.len() {
            h.offset = data.len();
        }
        let remain = data.len() - h.offset;
        let n = remain.min(buf.len());
        if n > 0 {
            buf[..n].copy_from_slice(&data[h.offset..h.offset + n]);
            h.offset += n;
        }
        Ok(n)
    }

    /// Whole-file borrow. The ELF loader uses this after `open` proves
    /// the name exists.
    pub fn bytes(&self, name: &str) -> Result<&'a [u8], RamFsError> {
        check_name(name)?;
        let slot = self.find(name).ok_or(RamFsError::NotFound)?;
        Ok(self.file(RamFd(slot as u8))?.data)
    }

    pub fn bytes_fd(&self, fd: RamFd) -> Result<&'a [u8], RamFsError> {
        Ok(self.file(fd)?.data)
    }

    fn file(&self, fd: RamFd) -> Result<&RamFile<'a>, RamFsError> {
        self.files
            .get(fd.slot())
            .and_then(|s| s.as_ref())
            .ok_or(RamFsError::BadFd)
    }

    fn find(&self, name: &str) -> Option<usize> {
        self.files[..self.n]
            .iter()
            .position(|f| f.as_ref().map(|file| file.name() == name).unwrap_or(false))
    }
}

fn check_name(name: &str) -> Result<(), RamFsError> {
    let b = name.as_bytes();
    if b.len() < 2 || b.len() > MAX_NAME {
        return Err(RamFsError::BadName);
    }
    if b[0] != b'/' {
        return Err(RamFsError::BadName);
    }
    for &c in &b[1..] {
        let ok = c.is_ascii_alphanumeric() || c == b'.' || c == b'_' || c == b'-';
        if !ok {
            return Err(RamFsError::BadName);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elf::{
        loads_in_window, parse_elf64, ELFCLASS64, ELFDATA2LSB, ELFMAG, ELFOSABI_NONE,
    };

    #[test]
    fn seed_open_read_init_and_probe() {
        let init = b"INIT-ELF-BYTES";
        let probe = b"PROBE-ELF-BYTES";
        let mut fs = RamFs::new();
        assert!(fs.is_empty());
        fs.seed(INIT_PATH, init).unwrap();
        fs.seed(PROBE_PATH, probe).unwrap();
        assert_eq!(fs.len(), 2);
        assert!(fs.contains(INIT_PATH));
        assert!(!fs.contains("/missing"));

        let mut h = fs.open(INIT_PATH).unwrap();
        assert_eq!(fs.size(h.fd()).unwrap(), init.len());
        let mut buf = [0u8; 32];
        let n = fs.read(&mut h, &mut buf).unwrap();
        assert_eq!(&buf[..n], init);
        assert_eq!(h.offset(), init.len());
        assert_eq!(fs.read(&mut h, &mut buf).unwrap(), 0);

        let mut p = fs.open(PROBE_PATH).unwrap();
        let n = fs.read(&mut p, &mut buf).unwrap();
        assert_eq!(&buf[..n], probe);
        assert_eq!(fs.bytes(INIT_PATH).unwrap(), init);
        assert_eq!(fs.bytes(PROBE_PATH).unwrap(), probe);
    }

    #[test]
    fn read_chunks_advances_offset() {
        let mut fs = RamFs::new();
        fs.seed(INIT_PATH, b"abcdefgh").unwrap();
        let mut h = fs.open(INIT_PATH).unwrap();
        let mut a = [0u8; 3];
        let mut b = [0u8; 3];
        let mut c = [0u8; 8];
        assert_eq!(fs.read(&mut h, &mut a).unwrap(), 3);
        assert_eq!(&a, b"abc");
        assert_eq!(fs.read(&mut h, &mut b).unwrap(), 3);
        assert_eq!(&b, b"def");
        assert_eq!(fs.read(&mut h, &mut c).unwrap(), 2);
        assert_eq!(&c[..2], b"gh");
        assert_eq!(h.offset(), 8);
    }

    #[test]
    fn open_missing_is_not_found() {
        let fs = RamFs::new();
        assert_eq!(fs.open(INIT_PATH).unwrap_err(), RamFsError::NotFound);
        assert_eq!(fs.bytes(INIT_PATH).unwrap_err(), RamFsError::NotFound);
    }

    #[test]
    fn seed_duplicate_is_exists() {
        let mut fs = RamFs::new();
        fs.seed(INIT_PATH, b"a").unwrap();
        assert_eq!(fs.seed(INIT_PATH, b"b").unwrap_err(), RamFsError::Exists);
        assert_eq!(fs.bytes(INIT_PATH).unwrap(), b"a");
    }

    #[test]
    fn reject_bad_names() {
        let mut fs = RamFs::new();
        for name in ["", "/", "init", "/foo/bar", "/has space", "///", "/"] {
            assert_eq!(
                fs.seed(name, b"x").unwrap_err(),
                RamFsError::BadName,
                "{name}"
            );
        }
        assert_eq!(
            fs.seed("/this-name-is-way-too-long", b"x").unwrap_err(),
            RamFsError::BadName
        );
        fs.seed("/ok_file.1", b"x").unwrap();
        assert!(fs.contains("/ok_file.1"));
    }

    #[test]
    fn table_full() {
        let mut fs = RamFs::new();
        for name in ["/a", "/b", "/c", "/d", "/e", "/f", "/g", "/h"] {
            fs.seed(name, b"x").unwrap();
        }
        assert_eq!(fs.seed("/z", b"x").unwrap_err(), RamFsError::Full);
    }

    #[test]
    fn empty_file_reads_zero() {
        let mut fs = RamFs::new();
        fs.seed(INIT_PATH, b"").unwrap();
        let mut h = fs.open(INIT_PATH).unwrap();
        assert_eq!(fs.size(h.fd()).unwrap(), 0);
        let mut buf = [0u8; 4];
        assert_eq!(fs.read(&mut h, &mut buf).unwrap(), 0);
    }

    #[test]
    fn bad_fd() {
        let fs = RamFs::new();
        assert_eq!(fs.size(RamFd(0)).unwrap_err(), RamFsError::BadFd);
        assert_eq!(fs.bytes_fd(RamFd(3)).unwrap_err(), RamFsError::BadFd);
    }

    fn write16(buf: &mut [u8], off: usize, v: u16) {
        buf[off..off + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn write32(buf: &mut [u8], off: usize, v: u32) {
        buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn write64(buf: &mut [u8], off: usize, v: u64) {
        buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
    }

    #[test]
    fn loader_opens_init_then_parses_elf() {
        let mut blob = [0u8; 256];
        blob[0..4].copy_from_slice(&ELFMAG);
        blob[4] = ELFCLASS64;
        blob[5] = ELFDATA2LSB;
        blob[6] = 1;
        blob[7] = ELFOSABI_NONE;
        write16(&mut blob, 16, 2); // ET_EXEC
        write16(&mut blob, 18, 62); // EM_X86_64
        write32(&mut blob, 20, 1);
        write64(&mut blob, 24, 0x0200_0100);
        write64(&mut blob, 32, 64);
        write16(&mut blob, 52, 64);
        write16(&mut blob, 54, 56);
        write16(&mut blob, 56, 1);
        write32(&mut blob, 64, 1); // PT_LOAD
        write32(&mut blob, 68, 5);
        write64(&mut blob, 72, 128);
        write64(&mut blob, 80, 0x0200_0000);
        write64(&mut blob, 88, 0x0200_0000);
        write64(&mut blob, 96, 4);
        write64(&mut blob, 104, 12);
        write64(&mut blob, 112, 0x1000);
        blob[128..132].copy_from_slice(b"\x90\x90\x90\x90");

        let mut fs = RamFs::new();
        fs.seed(INIT_PATH, &blob).unwrap();
        let mut h = fs.open(INIT_PATH).unwrap();
        let mut head = [0u8; 4];
        assert_eq!(fs.read(&mut h, &mut head).unwrap(), 4);
        assert_eq!(head, ELFMAG);
        let bytes = fs.bytes(INIT_PATH).unwrap();
        let img = parse_elf64(bytes).unwrap();
        assert_eq!(img.entry, 0x0200_0100);
        assert!(loads_in_window(&img, 0x0200_0000, 0x0220_0000));
    }
}
