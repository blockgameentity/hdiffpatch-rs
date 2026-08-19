//! # hdiffpatch-rs
//!
//! Safe, ergonomic Rust bindings to [HDiffPatch](https://github.com/sisong/HDiffPatch)
//! with zstd compression and directory diff/patch support.
//!
//! The crate links a vendored copy of HDiffPatch v5.0.1 and zstd 1.5.6 via a
//! thin C++-API shim compiled through [`cxx`]. Only zstd (de)compression and
//! the built-in `fadler32` checksum are enabled, so there are no external
//! codec dependencies and the build is fully cross-platform.
//!
//! ## Quick start
//!
//! ```no_run
//! use hdiffpatch_rs as hdp;
//!
//! let old = b"hello world";
//! let new = b"hello brave new world";
//! let diff = hdp::diff(old, new, hdp::DiffOptions::default()).unwrap();
//! let patched = hdp::patch(&diff, old, new.len()).unwrap();
//! assert_eq!(patched, new);
//! ```
//!
//! See [`diff`], [`patch`], [`diff_dir`], and [`patch_dir`] for the ergonomic
//! entry points, and [`DiffOptions`] for tuning.

pub mod api;

// ---------------------------------------------------------------------------
// cxx bridge: declarations live in src/cpp/hdp_glue.hpp and are implemented
// in src/cpp/hdp_glue.cpp. The bridge module is `crate::ffi`.
// ---------------------------------------------------------------------------
#[cxx::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("cpp/hdp_glue.hpp");

        // Single-file (in-memory) diff/patch.
        unsafe fn hdp_diff_zstd(
            new_data: *const u8,
            new_len: usize,
            old_data: *const u8,
            old_len: usize,
            out_diff: *mut u8,
            out_cap: usize,
            level: i32,
            threads: i32,
        ) -> i64;

        fn hdp_diff_bound_zstd(new_len: usize, old_len: usize) -> usize;

        unsafe fn hdp_patch_zstd(
            diff: *const u8,
            diff_len: usize,
            old_data: *const u8,
            old_len: usize,
            out_new: *mut u8,
            out_cap: usize,
        ) -> i64;

        unsafe fn hdp_diff_info_zstd(
            diff: *const u8,
            diff_len: usize,
            out_new_size: *mut u64,
            out_old_size: *mut u64,
        ) -> i32;

        // Directory diff/patch. `&str` maps to `rust::Str` (UTF-8, not
        // NUL-terminated); the shim copies it into a NUL-terminated
        // std::string before calling hdiffpatch's `const char*` file APIs.
        fn hdp_dir_diff_zstd(
            old_dir: &str,
            new_dir: &str,
            out_diff_path: &str,
            level: i32,
            threads: i32,
        ) -> i32;

        fn hdp_dir_patch_zstd(
            old_dir: &str,
            diff_path: &str,
            out_new_dir: &str,
            threads: i32,
        ) -> i32;
    }
}

/// Rust-side mirror of `HdpResult` from hdp_glue.hpp. Only the negative codes
/// are meaningful; non-negative values are byte counts returned directly.
#[allow(dead_code)]
#[repr(i32)]
pub enum HdpResult {
    Ok = 0,
    Null = -1,
    Range = -2,
    Capacity = -3,
    Invalid = -4,
    Io = -5,
    Internal = -6,
    Checksum = -7,
    OutOfMem = -8,
}

pub use api::{
    diff, diff_dir, diff_info, diff_into, patch, patch_dir, patch_into, DiffInfo, DiffOptions,
    Error,
};
