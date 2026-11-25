//! In-kernel ramfs: named files over borrowed byte slices.
//!
//! Documented subset — **not** POSIX, **not** a user `open`/`read`
//! syscall. The kernel seeds `/init` (and optional `/probe`) from a
//! virtio-blk AETHFS01 image when a drive is present, otherwise from
//! embedded ELF blobs. The loader uses [`RamFs::open`] / [`RamFs::read`]
//! on those names instead of calling `include_bytes!` at the load site.
//! Host tests cover open/read. Numbers 0–10 stay as documented in ABI.md.
//!
//! Files are borrowed slices (embedded `'static` blobs, or the reserved
//! virtio-blk window).

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

impl Default for RamFs<'_> {
    fn default() -> Self {
        Self::new()
    }
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

    #[test]
    fn name_length_boundary_and_non_ascii() {
        let mut fs = RamFs::new();
        // MAX_NAME counts the leading '/': 16 bytes admit, 17 refuse.
        let full = "/0123456789abcde";
        assert_eq!(full.len(), MAX_NAME);
        fs.seed(full, b"full").unwrap();
        assert_eq!(fs.bytes(full).unwrap(), b"full");
        let mut h = fs.open(full).unwrap();
        let mut buf = [0u8; 8];
        assert_eq!(fs.read(&mut h, &mut buf).unwrap(), 4);
        assert_eq!(&buf[..4], b"full");
        let over = "/0123456789abcdef";
        assert_eq!(over.len(), MAX_NAME + 1);
        assert_eq!(fs.seed(over, b"x").unwrap_err(), RamFsError::BadName);
        // Non-ASCII UTF-8, NUL and path separators never become names.
        for bad in ["/\u{e9}", "/a\0b", "/a\\b", "/a\tb", "//"] {
            assert_eq!(
                fs.seed(bad, b"x").unwrap_err(),
                RamFsError::BadName,
                "{bad:?}"
            );
        }
        assert_eq!(fs.len(), 1);
    }

    #[test]
    fn refusal_order_bad_name_then_exists_then_full() {
        let mut fs = RamFs::new();
        for name in ["/a", "/b", "/c", "/d", "/e", "/f", "/g", "/h"] {
            fs.seed(name, b"x").unwrap();
        }
        // At capacity: a bad name is still BadName and a held name is
        // still Exists, so callers can tell the causes apart.
        assert_eq!(fs.seed("bad", b"y").unwrap_err(), RamFsError::BadName);
        assert_eq!(fs.seed("/a", b"y").unwrap_err(), RamFsError::Exists);
        assert_eq!(fs.seed("/z", b"y").unwrap_err(), RamFsError::Full);
        assert_eq!(fs.len(), MAX_FILES);
        assert_eq!(fs.bytes("/a").unwrap(), b"x");
        // Lookups validate the name before searching.
        assert_eq!(fs.open("init").unwrap_err(), RamFsError::BadName);
        assert_eq!(fs.bytes("/a/b").unwrap_err(), RamFsError::BadName);
        assert_eq!(fs.open("/z").unwrap_err(), RamFsError::NotFound);
        assert!(!fs.contains("a"));
    }

    #[test]
    fn refused_seeds_do_not_consume_slots_and_fds_are_stable() {
        let mut fs = RamFs::new();
        let init = fs.seed(INIT_PATH, b"init").unwrap();
        assert_eq!(fs.seed(INIT_PATH, b"dup").unwrap_err(), RamFsError::Exists);
        assert_eq!(fs.seed("/bad name", b"x").unwrap_err(), RamFsError::BadName);
        let probe = fs.seed(PROBE_PATH, b"probe").unwrap();
        assert_eq!(init.slot(), 0);
        assert_eq!(probe.slot(), 1);
        assert_eq!(fs.len(), 2);
        assert_eq!(fs.open(INIT_PATH).unwrap().fd(), init);
        assert_eq!(fs.open(PROBE_PATH).unwrap().fd(), probe);
        assert_eq!(fs.bytes_fd(init).unwrap(), b"init");
        assert_eq!(fs.bytes_fd(probe).unwrap(), b"probe");
        assert_eq!(fs.size(probe).unwrap(), 5);
        // A slot past the published count is BadFd even inside MAX_FILES.
        assert_eq!(fs.size(RamFd(2)).unwrap_err(), RamFsError::BadFd);
        assert_eq!(fs.bytes_fd(RamFd(u8::MAX)).unwrap_err(), RamFsError::BadFd);
    }

    #[test]
    fn handles_have_independent_cursors() {
        let mut fs = RamFs::new();
        fs.seed(INIT_PATH, b"0123456789").unwrap();
        let mut a = fs.open(INIT_PATH).unwrap();
        let mut b = fs.open(INIT_PATH).unwrap();
        let mut buf = [0u8; 4];
        assert_eq!(fs.read(&mut a, &mut buf).unwrap(), 4);
        assert_eq!(&buf, b"0123");
        assert_eq!(b.offset(), 0);
        assert_eq!(fs.read(&mut b, &mut buf[..2]).unwrap(), 2);
        assert_eq!(&buf[..2], b"01");
        // A copied handle snapshots the cursor; advancing one leaves the other.
        let mut c = a;
        assert_eq!(fs.read(&mut c, &mut buf).unwrap(), 4);
        assert_eq!(&buf, b"4567");
        assert_eq!(a.offset(), 4);
        assert_eq!(c.offset(), 8);
        assert_eq!(fs.read(&mut a, &mut buf).unwrap(), 4);
        assert_eq!(&buf, b"4567");
    }

    #[test]
    fn empty_buffer_and_stale_cursor_reads() {
        let mut fs = RamFs::new();
        fs.seed(INIT_PATH, b"abc").unwrap();
        let mut h = fs.open(INIT_PATH).unwrap();
        // A zero-length buffer copies nothing and keeps the cursor.
        assert_eq!(fs.read(&mut h, &mut []).unwrap(), 0);
        assert_eq!(h.offset(), 0);
        // A cursor past EOF is clamped to the file size and reads 0.
        let mut stale = RamHandle {
            fd: h.fd(),
            offset: 100,
        };
        let mut buf = [0xeeu8; 4];
        assert_eq!(fs.read(&mut stale, &mut buf).unwrap(), 0);
        assert_eq!(stale.offset(), 3);
        assert_eq!(buf, [0xee; 4]);
        // A handle whose fd is not published is BadFd and keeps its cursor.
        let mut ghost = RamHandle {
            fd: RamFd(5),
            offset: 1,
        };
        assert_eq!(
            fs.read(&mut ghost, &mut buf).unwrap_err(),
            RamFsError::BadFd
        );
        assert_eq!(ghost.offset(), 1);
    }

    #[test]
    fn files_borrow_seeded_bytes_without_copying() {
        let blob: &'static [u8] = b"embedded-elf";
        let mut fs = RamFs::new();
        let fd = fs.seed(INIT_PATH, blob).unwrap();
        assert_eq!(fs.bytes(INIT_PATH).unwrap().as_ptr(), blob.as_ptr());
        assert_eq!(fs.bytes_fd(fd).unwrap().as_ptr(), blob.as_ptr());
        assert_eq!(fs.bytes(INIT_PATH).unwrap().len(), blob.len());
        // A copied table still borrows the same bytes.
        let copy = fs;
        assert_eq!(copy.bytes(INIT_PATH).unwrap().as_ptr(), blob.as_ptr());
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
