//! Raw POSIX mmap/munmap/msync bindings (Linux x86_64), hand-declared
//! rather than pulled from a crate — the only unambiguous reading of
//! task.md's "standard library only for the core," since std has no
//! built-in mmap wrapper. All `unsafe` for the store's real file
//! backend is isolated to this file and `page_io/mmap.rs`.

use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::raw::c_int;
use std::os::unix::io::AsRawFd;

const PROT_READ: c_int = 0x1;
const PROT_WRITE: c_int = 0x2;
const MAP_SHARED: c_int = 0x01;
const MS_SYNC: c_int = 0x4;

unsafe extern "C" {
    fn mmap(
        addr: *mut c_void,
        length: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(addr: *mut c_void, length: usize) -> c_int;
    fn msync(addr: *mut c_void, length: usize, flags: c_int) -> c_int;
}

/// A fixed-size, fixed-address `MAP_SHARED` mapping of a file, held for
/// the mapping's entire lifetime. Deliberately never remapped/resized
/// (see `page_io/mmap.rs`) — that sidesteps a remap-during-concurrent-
/// read hazard entirely, at the cost of needing `len` reserved upfront
/// (cheap on 64-bit Linux: unused reserved virtual space costs nothing
/// physically until the backing file is actually grown into it).
///
/// Exposes only `read_at`/`write_at` (raw `memcpy`-style byte copies
/// through the pointer) rather than `&[u8]`/`&mut [u8]` views — this
/// avoids ever materializing a Rust reference over memory the OS
/// considers shared/externally-mutable, which sidesteps Rust's
/// reference-aliasing rules entirely instead of trying to satisfy them.
pub(crate) struct RawMmap {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: `ptr` addresses OS-managed shared memory, not a Rust
// allocation with thread-affine invariants, so moving `RawMmap` across
// threads (Send) is fine. Sharing `&RawMmap` across threads (Sync) is
// fine given the access discipline enforced by `MmapPageIo`/`Store`:
// concurrent `write_at` calls never target overlapping regions (only
// the single serialized writer ever calls it), and a `read_at` never
// targets a page that a concurrent `write_at` could still be touching
// (copy-on-write: a page is written once, then never again in place;
// readers only ever reach pages via an already-published, immutable
// root).
unsafe impl Send for RawMmap {}
unsafe impl Sync for RawMmap {}

impl RawMmap {
    /// Maps `len` bytes of `file` at offset 0, `PROT_READ|PROT_WRITE`,
    /// `MAP_SHARED`. `len` may exceed the file's current size (Linux
    /// permits this); touching bytes beyond the file's real length
    /// before it's grown to cover them raises `SIGBUS`, so callers must
    /// only ever address pages within the already-`set_len`'d region.
    pub fn new(file: &File, len: usize) -> io::Result<Self> {
        let ptr = unsafe {
            mmap(
                std::ptr::null_mut(),
                len,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if ptr == usize::MAX as *mut c_void {
            return Err(io::Error::last_os_error());
        }
        Ok(RawMmap {
            ptr: ptr as *mut u8,
            len,
        })
    }

    pub fn len(&self) -> usize {
        self.len
    }

    /// Copies `len` bytes starting at `offset` out into an owned buffer.
    pub fn read_at(&self, offset: usize, len: usize) -> Vec<u8> {
        assert!(offset + len <= self.len, "read_at out of mapped range");
        let mut buf = vec![0u8; len];
        // SAFETY: bounds-checked above; `offset..offset+len` is within
        // the mapping, and this is a plain byte copy through raw
        // pointers, never forming a `&[u8]`/`&mut [u8]` over the mapping.
        unsafe {
            std::ptr::copy_nonoverlapping(self.ptr.add(offset), buf.as_mut_ptr(), len);
        }
        buf
    }

    /// Copies `bytes` in, starting at `offset`.
    pub fn write_at(&self, offset: usize, bytes: &[u8]) {
        assert!(
            offset + bytes.len() <= self.len,
            "write_at out of mapped range"
        );
        // SAFETY: bounds-checked above; see the `Sync` justification for
        // why concurrent calls here can't race.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.ptr.add(offset), bytes.len());
        }
    }

    /// `msync(MS_SYNC)`: blocks until prior writes are durable on disk.
    pub fn sync(&self) -> io::Result<()> {
        let ret = unsafe { msync(self.ptr as *mut c_void, self.len, MS_SYNC) };
        if ret != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for RawMmap {
    fn drop(&mut self) {
        unsafe {
            munmap(self.ptr as *mut c_void, self.len);
        }
    }
}
