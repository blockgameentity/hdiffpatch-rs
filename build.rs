// build.rs — build the HDiffPatch C/C++ sources, vendored zstd, and the C++ glue
// shim, then wire them into a cxx bridge. Cross-platform (Linux/macOS/Windows).
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let hdp = manifest_dir.join("vendor").join("hdiffpatch");
    let zstd = manifest_dir.join("vendor").join("zstd").join("lib");
    let src_cpp = manifest_dir.join("src").join("cpp");

    // Shared compile definitions (mirror the relevant subset of the upstream
    // Makefile: zstd codec + fadler32 checksum + dir diff + single-stream +
    // multithreading). No zlib/lzma/bzip2/libdeflate dependencies are pulled in.
    let defines: &[(&str, Option<&str>)] = &[
        ("_IS_NEED_ALL_CompressPlugin", Some("0")),
        ("_IS_NEED_DEFAULT_CompressPlugin", Some("0")),
        ("_IS_NEED_ALL_ChecksumPlugin", Some("0")),
        ("_IS_NEED_DEFAULT_ChecksumPlugin", Some("0")),
        ("_IS_NEED_BSDIFF", Some("0")),
        ("_IS_NEED_VCDIFF", Some("0")),
        ("_IS_NEED_DIR_DIFF_PATCH", Some("1")),
        ("_IS_NEED_SINGLE_STREAM_DIFF", Some("1")),
        ("_IS_USED_MULTITHREAD", Some("1")),
        ("_CompressPlugin_zstd", None),
        ("_ChecksumPlugin_fadler32", None),
        // zstd tuning (matches upstream Makefile for ZSTD=1).
        ("ZSTD_MULTITHREAD", Some("1")),
        ("ZSTD_HAVE_WEAK_SYMBOLS", Some("0")),
        ("ZSTD_TRACE", Some("0")),
        ("ZSTD_DISABLE_ASM", Some("1")),
        ("ZSTD_LIB_DEPRECATED", Some("0")),
        ("ZSTD_STRIP_ERROR_STRINGS", Some("1")),
        ("_LARGEFILE_SOURCE", None),
        ("_FILE_OFFSET_BITS", Some("64")),
        ("NDEBUG", None),
    ];

    let includes: &[PathBuf] = &[
        hdp.clone(),
        zstd.clone(),
        zstd.join("common"),
        zstd.join("compress"),
        zstd.join("decompress"),
        manifest_dir.join("src"), // so `include!("cpp/hdp_glue.hpp")` resolves
        src_cpp.clone(),
    ];

    // ---- C++ build: cxx bridge + glue + HDiffPatch C++ sources -------------
    let mut cxx = cxx_build::bridge("src/lib.rs");
    cxx.cpp(true).std("c++14");
    for inc in includes {
        cxx.include(inc);
    }
    for &(k, v) in defines {
        cxx.define(k, v);
    }
    cxx.flag("-O3");
    // Suppress noise from upstream headers when possible.
    if !cfg!(target_os = "windows") {
        cxx.flag("-Wno-unused-function");
        cxx.flag("-Wno-unused-variable");
    }
    cxx.file(src_cpp.join("hdp_glue.cpp"));

    for f in HDIFF_CPP_FILES {
        cxx.file(hdp.join(f));
    }
    cxx.compile("hdiffpatch_rs_cxx");

    // ---- C build: HDiffPatch C sources + vendored zstd sources ------------
    let mut c = cc::Build::new();
    c.cpp(false);
    for inc in includes {
        c.include(inc);
    }
    for &(k, v) in defines {
        c.define(k, v);
    }
    c.flag("-O3");
    if !cfg!(target_os = "windows") {
        c.flag("-Wno-unused-function");
        c.flag("-Wno-unused-variable");
        c.flag("-Wno-unused-but-set-variable");
    }
    for f in HDIFF_C_FILES {
        c.file(hdp.join(f));
    }
    for f in ZSTD_C_FILES {
        c.file(zstd.join(f));
    }
    c.compile("hdiffpatch_rs_c");

    // pthread on POSIX (zstd + hdiffpatch MT).
    if !cfg!(target_os = "windows") {
        println!("cargo:rustc-link-lib=pthread");
    }
    // stdc++ on POSIX (we compile C++ objects).
    if !cfg!(target_os = "windows") {
        println!("cargo:rustc-link-lib=dylib=stdc++");
    }

    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/cpp/hdp_glue.cpp");
    println!("cargo:rerun-if-changed=src/cpp/hdp_glue.hpp");
    for f in HDIFF_CPP_FILES.iter().chain(HDIFF_C_FILES.iter()) {
        println!("cargo:rerun-if-changed={}/{}", hdp.display(), f);
    }
    for f in ZSTD_C_FILES.iter() {
        println!("cargo:rerun-if-changed={}/{}", zstd.display(), f);
    }
}

// HDiffPatch C++ translation units (single-stream + dir diff + patch + MT).
const HDIFF_CPP_FILES: &[&str] = &[
    "libHDiffPatch/HDiff/diff.cpp",
    "libHDiffPatch/HDiff/private_diff/match_block.cpp",
    "libHDiffPatch/HDiff/private_diff/bytes_rle.cpp",
    "libHDiffPatch/HDiff/private_diff/suffix_string.cpp",
    "libHDiffPatch/HDiff/private_diff/compress_detect.cpp",
    "libHDiffPatch/HDiff/private_diff/libdivsufsort/divsufsort.cpp",
    "libHDiffPatch/HDiff/private_diff/libdivsufsort/divsufsort64.cpp",
    "libHDiffPatch/HDiff/private_diff/window_diff/window_matcher.cpp",
    "libHDiffPatch/HDiff/private_diff/window_diff/covers_range.cpp",
    "libHDiffPatch/HDiff/private_diff/limit_mem_diff/digest_matcher.cpp",
    "libHDiffPatch/HDiff/private_diff/limit_mem_diff/stream_serialize.cpp",
    "dirDiffPatch/dir_diff/dir_diff.cpp",
    "dirDiffPatch/dir_diff/dir_diff_tools.cpp",
    "dirDiffPatch/dir_diff/dir_manifest.cpp",
    "compress_parallel.cpp",
    "libParallel/parallel_channel.cpp",
];

// HDiffPatch C translation units (patch core + MT + dir patch + file I/O).
const HDIFF_C_FILES: &[&str] = &[
    "file_for_patch.c",
    "hdiffz_import_patch.c",
    "libHDiffPatch/HPatch/patch.c",
    "libHDiffPatch/HPatch/hpatch_mt/_hcache_old_mt.c",
    "libHDiffPatch/HPatch/hpatch_mt/_hcache_window_old_mt.c",
    "libHDiffPatch/HPatch/hpatch_mt/_hinput_mt.c",
    "libHDiffPatch/HPatch/hpatch_mt/_houtput_mt.c",
    "libHDiffPatch/HPatch/hpatch_mt/_hpatch_mt.c",
    "libHDiffPatch/HPatch/hpatch_mt/hpatch_mt.c",
    "libParallel/parallel_import_c.c",
    "libHDiffPatch/HDiff/private_diff/limit_mem_diff/adler_roll.c",
    "libHDiffPatch/HPatchLite/hpatch_lite.c",
    "dirDiffPatch/dir_patch/dir_patch.c",
    "dirDiffPatch/dir_patch/res_handle_limit.c",
    "dirDiffPatch/dir_patch/ref_stream.c",
    "dirDiffPatch/dir_patch/new_stream.c",
    "dirDiffPatch/dir_patch/dir_patch_tools.c",
    "dirDiffPatch/dir_patch/new_dir_output.c",
];

// Vendored zstd sources (common + compress + decompress; MT variants).
const ZSTD_C_FILES: &[&str] = &[
    "common/debug.c",
    "common/entropy_common.c",
    "common/error_private.c",
    "common/fse_decompress.c",
    "common/pool.c",
    "common/threading.c",
    "common/xxhash.c",
    "common/zstd_common.c",
    "decompress/huf_decompress.c",
    "decompress/zstd_ddict.c",
    "decompress/zstd_decompress.c",
    "decompress/zstd_decompress_block.c",
    "compress/fse_compress.c",
    "compress/hist.c",
    "compress/huf_compress.c",
    "compress/zstd_compress.c",
    "compress/zstd_compress_literals.c",
    "compress/zstd_compress_sequences.c",
    "compress/zstd_compress_superblock.c",
    "compress/zstd_double_fast.c",
    "compress/zstd_fast.c",
    "compress/zstd_lazy.c",
    "compress/zstd_ldm.c",
    "compress/zstd_opt.c",
    "compress/zstdmt_compress.c",
];
