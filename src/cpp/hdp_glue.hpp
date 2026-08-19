// hdp_glue.hpp — thin C++ API shim over HDiffPatch (zstd + fadler32).
// Exposed to Rust via a cxx bridge (`extern "C++"`), so functions use C++
// linkage and may take `rust::Str` (cxx's borrowed-string type).
#ifndef HDP_GLUE_HPP
#define HDP_GLUE_HPP

#include <stddef.h>
#include <stdint.h>

#include "rust/cxx.h"  // rust::Str

// Error codes returned by the shim (negative); non-negative values are byte
// counts. Kept in sync with `HdpResult` in src/ffi.rs.
enum HdpResult : int64_t {
    HDP_OK              = 0,
    HDP_ERR_NULL        = -1,
    HDP_ERR_RANGE       = -2,
    HDP_ERR_CAPACITY    = -3,
    HDP_ERR_INVALID     = -4,
    HDP_ERR_IO          = -5,
    HDP_ERR_INTERNAL    = -6,
    HDP_ERR_CHECKSUM    = -7,
    HDP_ERR_OUT_OF_MEM  = -8,
};

// ---- Single-file (in-memory) diff/patch ----------------------------------

// Create a zstd-compressed single-stream diff from old->new.
// Returns the bytes written, or a negative HdpResult.
int64_t hdp_diff_zstd(const uint8_t* new_data, size_t new_len,
                      const uint8_t* old_data, size_t old_len,
                      uint8_t* out_diff, size_t out_cap,
                      int level, int threads);

// Generous upper bound for a diff's serialized size.
size_t hdp_diff_bound_zstd(size_t new_len, size_t old_len);

// Apply a zstd-compressed single-stream diff. Returns bytes written, or a
// negative HdpResult.
int64_t hdp_patch_zstd(const uint8_t* diff, size_t diff_len,
                      const uint8_t* old_data, size_t old_len,
                      uint8_t* out_new, size_t out_cap);

// Read old/new sizes from a diff. Returns HDP_OK on success.
int32_t hdp_diff_info_zstd(const uint8_t* diff, size_t diff_len,
                           uint64_t* out_new_size, uint64_t* out_old_size);

// ---- Directory diff/patch ------------------------------------------------

// Create a directory diff between old_dir and new_dir -> out_diff_path.
int32_t hdp_dir_diff_zstd(rust::Str old_dir, rust::Str new_dir,
                          rust::Str out_diff_path,
                          int level, int threads);

// Apply a directory diff: reconstruct out_new_dir from old_dir + diff_path.
int32_t hdp_dir_patch_zstd(rust::Str old_dir, rust::Str diff_path,
                           rust::Str out_new_dir, int threads);

#endif  // HDP_GLUE_CPP
