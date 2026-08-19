//! Safe, ergonomic Rust API over the HDiffPatch FFI shim.
//!
//! All FFI calls return `i64`/`i32` status codes (see [`ffi::HdpResult`]).
//! This module converts those into typed [`Error`]s and owns the buffer
//! sizing so callers never touch raw pointers.

use crate::ffi;
use std::path::Path;

/// Tuning for diff creation. Sensible defaults: zstd level 3, single thread.
#[derive(Debug, Clone, Copy)]
pub struct DiffOptions {
    /// zstd compression level (1..22). `0` means "use zstd default" (level 3).
    pub level: i32,
    /// Number of worker threads (>=1).
    pub threads: i32,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            level: 0,
            threads: 1,
        }
    }
}

impl DiffOptions {
    /// Set the zstd compression level (clamped to 0..22 on the C++ side).
    #[inline]
    pub const fn level(mut self, level: i32) -> Self {
        self.level = level;
        self
    }

    /// Set the worker-thread count (clamped to >=1 on the C++ side).
    #[inline]
    pub const fn threads(mut self, threads: i32) -> Self {
        self.threads = threads;
        self
    }
}

/// Metadata extracted from a serialized (single-compressed, zstd) diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffInfo {
    /// Size of the new (target) data the diff reconstructs.
    pub new_size: u64,
    /// Size of the old (source) data the diff was built against.
    pub old_size: u64,
}

/// Errors returned by the safe API.
///
/// The `Code` variant carries the raw negative status word from the FFI shim
/// for forward compatibility with codes not yet modeled here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// A null pointer would have been passed across the boundary.
    Null,
    /// A numeric argument was out of its valid range.
    Range,
    /// The caller-supplied output buffer was too small.
    Capacity,
    /// The diff data was malformed / not a valid single-compressed diff.
    Invalid,
    /// A file or stream I/O operation failed.
    Io,
    /// An internal HDiffPatch failure (exception, unexpected `hpatch_FALSE`).
    Internal,
    /// A checksum mismatch was detected while patching.
    Checksum,
    /// An allocation failed inside the C++ layer.
    OutOfMem,
    /// An unrecognized negative status code.
    Code(i64),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            Error::Null => "null argument",
            Error::Range => "argument out of range",
            Error::Capacity => "output buffer too small",
            Error::Invalid => "invalid diff data",
            Error::Io => "I/O failure",
            Error::Internal => "internal HDiffPatch failure",
            Error::Checksum => "checksum mismatch",
            Error::OutOfMem => "out of memory",
            Error::Code(c) => {
                return write!(f, "hdiffpatch error code {}", c);
            }
        };
        f.write_str(msg)
    }
}

impl std::error::Error for Error {}

/// Map a status word (negative code or non-negative byte count) into a result.
#[inline]
fn status(code: i64) -> Result<i64, Error> {
    if code >= 0 {
        Ok(code)
    } else {
        Err(match code as i32 {
            x if x == crate::HdpResult::Null as i32 => Error::Null,
            x if x == crate::HdpResult::Range as i32 => Error::Range,
            x if x == crate::HdpResult::Capacity as i32 => Error::Capacity,
            x if x == crate::HdpResult::Invalid as i32 => Error::Invalid,
            x if x == crate::HdpResult::Io as i32 => Error::Io,
            x if x == crate::HdpResult::Internal as i32 => Error::Internal,
            x if x == crate::HdpResult::Checksum as i32 => Error::Checksum,
            x if x == crate::HdpResult::OutOfMem as i32 => Error::OutOfMem,
            _ => Error::Code(code),
        })
    }
}

/// Map a raw `i32` HdpResult code into `Result<(), Error>`.
#[inline]
fn unit(code: i32) -> Result<(), Error> {
    status(code as i64).map(|_| ())
}

// ---------------------------------------------------------------------------
// Single-file (in-memory) diff/patch
// ---------------------------------------------------------------------------

/// Create a zstd-compressed single-stream diff that turns `old` into `new`.
///
/// The returned `Vec<u8>` is the serialized diff. `opts` selects the zstd
/// level and worker-thread count.
///
/// ```
/// # use hdiffpatch_rs as hdp;
/// let old = b"the quick brown fox";
/// let new = b"the quick brown fox jumps over the lazy dog";
/// let diff = hdp::diff(old, new, hdp::DiffOptions::default()).unwrap();
/// assert!(!diff.is_empty());
/// ```
pub fn diff(old: &[u8], new: &[u8], opts: DiffOptions) -> Result<Vec<u8>, Error> {
    let cap = ffi::hdp_diff_bound_zstd(new.len(), old.len());
    let mut out = vec![0u8; cap];
    let n = status(unsafe {
        ffi::hdp_diff_zstd(
            new.as_ptr(),
            new.len(),
            old.as_ptr(),
            old.len(),
            out.as_mut_ptr(),
            out.len(),
            opts.level,
            opts.threads,
        )
    })?;
    out.truncate(n as usize);
    Ok(out)
}

/// Create a diff into a caller-provided buffer.
///
/// Returns the number of bytes written. If the buffer is too small, returns
/// [`Error::Capacity`] (the FFI will not write partial output).
pub fn diff_into(
    old: &[u8],
    new: &[u8],
    opts: DiffOptions,
    out: &mut [u8],
) -> Result<usize, Error> {
    let n = status(unsafe {
        ffi::hdp_diff_zstd(
            new.as_ptr(),
            new.len(),
            old.as_ptr(),
            old.len(),
            out.as_mut_ptr(),
            out.len(),
            opts.level,
            opts.threads,
        )
    })?;
    Ok(n as usize)
}

/// Inspect a serialized diff without patching, returning the recorded old/new sizes.
pub fn diff_info(diff: &[u8]) -> Result<DiffInfo, Error> {
    let mut new_size: u64 = 0;
    let mut old_size: u64 = 0;
    unit(unsafe {
        ffi::hdp_diff_info_zstd(diff.as_ptr(), diff.len(), &mut new_size, &mut old_size)
    })?;
    Ok(DiffInfo { new_size, old_size })
}

/// Apply a single-stream zstd diff to `old`, reconstructing `new`.
///
/// If `new_len_hint` is `Some(n)`, exactly `n` bytes are produced. Otherwise
/// the size is read from the diff via [`diff_info`]. The returned `Vec` holds
/// the reconstructed data.
///
/// ```
/// # use hdiffpatch_rs as hdp;
/// # let old = b"abc"; let new = b"abcdef";
/// # let diff = hdp::diff(old, new, hdp::DiffOptions::default()).unwrap();
/// let got = hdp::patch(&diff, old, new.len()).unwrap();
/// assert_eq!(got, new);
/// ```
pub fn patch(diff: &[u8], old: &[u8], new_len_hint: Option<usize>) -> Result<Vec<u8>, Error> {
    let new_len = match new_len_hint {
        Some(n) => n as u64,
        None => diff_info(diff)?.new_size,
    };
    let mut out = vec![0u8; new_len as usize];
    patch_into(diff, old, &mut out)?;
    Ok(out)
}

/// Apply a single-stream zstd diff into a caller-provided buffer.
///
/// `out` must be at least as large as the new data the diff reconstructs;
/// discover it beforehand with [`diff_info`]. Returns the number of bytes
/// written (which equals `out.len()` when correctly sized).
pub fn patch_into(diff: &[u8], old: &[u8], out: &mut [u8]) -> Result<usize, Error> {
    let n = status(unsafe {
        ffi::hdp_patch_zstd(
            diff.as_ptr(),
            diff.len(),
            old.as_ptr(),
            old.len(),
            out.as_mut_ptr(),
            out.len(),
        )
    })?;
    Ok(n as usize)
}

// ---------------------------------------------------------------------------
// Directory diff/patch
// ---------------------------------------------------------------------------

/// Create a zstd-compressed directory diff between `old_dir` and `new_dir`,
/// writing the serialized dir-diff to `out_diff_path`.
///
/// The diff records the `fadler32` checksum for verification during patch.
pub fn diff_dir<P, Q, R>(
    old_dir: P,
    new_dir: Q,
    out_diff_path: R,
    opts: DiffOptions,
) -> Result<(), Error>
where
    P: AsRef<Path>,
    Q: AsRef<Path>,
    R: AsRef<Path>,
{
    let old_dir = old_dir.as_ref();
    let new_dir = new_dir.as_ref();
    let out = out_diff_path.as_ref();
    let old_s = path_str(old_dir)?;
    let new_s = path_str(new_dir)?;
    let out_s = path_str(out)?;
    unit(ffi::hdp_dir_diff_zstd(
        &old_s,
        &new_s,
        &out_s,
        opts.level,
        opts.threads,
    ))
}

/// Apply a directory diff: reconstruct the new directory at `out_new_dir`
/// by referencing unchanged files from `old_dir` and applying `diff_path`.
///
/// `out_new_dir` must not be the same path as `old_dir` and should be empty
/// or absent (it is created if necessary).
pub fn patch_dir<P, Q, R>(
    old_dir: P,
    diff_path: Q,
    out_new_dir: R,
    opts: DiffOptions,
) -> Result<(), Error>
where
    P: AsRef<Path>,
    Q: AsRef<Path>,
    R: AsRef<Path>,
{
    let old_s = path_str(old_dir.as_ref())?;
    let diff_s = path_str(diff_path.as_ref())?;
    let out_s = path_str(out_new_dir.as_ref())?;
    unit(ffi::hdp_dir_patch_zstd(
        &old_s,
        &diff_s,
        &out_s,
        opts.threads,
    ))
}

/// Convert a `Path` to a UTF-8 string suitable for the FFI.
/// Returns [`Error::Invalid`] if the platform path is not valid UTF-8.
#[inline]
fn path_str(p: &Path) -> Result<String, Error> {
    p.to_str().map(|s| s.to_owned()).ok_or(Error::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(old: &[u8], new: &[u8]) {
        let diff = diff(old, new, DiffOptions::default()).expect("diff");
        let info = diff_info(&diff).expect("info");
        assert_eq!(info.old_size, old.len() as u64);
        assert_eq!(info.new_size, new.len() as u64);
        let got = patch(&diff, old, Some(new.len())).expect("patch");
        assert_eq!(got.as_slice(), new);
    }

    #[test]
    fn identical() {
        roundtrip(b"", b"");
        roundtrip(b"same", b"same");
    }

    #[test]
    fn small_changes() {
        roundtrip(b"hello world", b"hello brave new world");
        roundtrip(b"abcdef", b"abXYef");
    }

    #[test]
    fn larger_random() {
        let mut old = vec![0u8; 64 * 1024];
        let mut new = vec![0u8; 64 * 1024];
        let mut s: u32 = 0x1234_5678;
        for b in old.iter_mut().chain(new.iter_mut()) {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            *b = (s & 0xff) as u8;
        }
        // make ~10% differ
        for i in (0..new.len()).step_by(10) {
            new[i] = new[i].wrapping_add(1);
        }
        roundtrip(&old, &new);
    }
}
