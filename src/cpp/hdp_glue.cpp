// hdp_glue.cpp — implementation of the C++-API shim over HDiffPatch.
// Only zstd (de)compression and the built-in fadler32 checksum are enabled,
// so no external codec dependencies are required.
#include "hdp_glue.hpp"

#include "libHDiffPatch/HDiff/diff.h"
#include "libHDiffPatch/HPatch/patch.h"
#include "compress_plugin_demo.h"     // zstdCompressPlugin
#include "decompress_plugin_demo.h"   // zstdDecompressPlugin
#include "checksum_plugin_demo.h"     // fadler32ChecksumPlugin
#include "dirDiffPatch/dir_diff/dir_diff.h"
#include "dirDiffPatch/dir_patch/dir_patch.h"
#include "file_for_patch.h"
#include "hpatch_dir_listener.h"      // defaultPatchDirlistener

#include <vector>
#include <cstring>
#include <exception>
#include <new>

// ---------------------------------------------------------------------------
// RAII helpers for the file streams (so cleanup is exception-safe).
// ---------------------------------------------------------------------------
namespace {

struct FileStreamInputGuard {
    hpatch_TFileStreamInput s;
    FileStreamInputGuard() { hpatch_TFileStreamInput_init(&s); }
    ~FileStreamInputGuard() { hpatch_TFileStreamInput_close(&s); }
    hpatch_TStreamInput* stream() { return &s.base; }
};

struct FileStreamOutputGuard {
    hpatch_TFileStreamOutput s;
    FileStreamOutputGuard() { hpatch_TFileStreamOutput_init(&s); }
    ~FileStreamOutputGuard() { hpatch_TFileStreamOutput_close(&s); }
    hpatch_TStreamOutput* stream() { return &s.base; }
};

// A directory-diff listener that records execute-file tags via the
// platform helpers in file_for_patch (chmod on POSIX, no-op on Windows).
struct GlueDirDiffListener : IDirDiffListener {
    bool isExecuteFile(const std::string& fileName) override {
        return 0 != hpatch_getIsExecuteFile(fileName.c_str());
    }
};

// A no-op path-ignore filter (keeps every file) for manifest building.
struct GlueNoIgnore : IDirPathIgnore {
    bool isNeedIgnore(const std::string&, size_t) override { return false; }
};

static int clamp_level(int level) {
    if (level <= 0) return 3;   // zstd's own default
    if (level > 22) return 22;
    return level;
}

// Copy a rust::Str (UTF-8, not NUL-terminated) into a NUL-terminated string.
static std::string to_std_string(rust::Str s) {
    return std::string(s.data(), s.size());
}

}  // namespace

// ---------------------------------------------------------------------------
// Single-file diff
// ---------------------------------------------------------------------------
int64_t hdp_diff_zstd(const uint8_t* new_data, size_t new_len,
                      const uint8_t* old_data, size_t old_len,
                      uint8_t* out_diff, size_t out_cap,
                      int level, int threads) {
    if (!new_data || !old_data || !out_diff) return HDP_ERR_NULL;
    if (threads < 1) threads = 1;
    try {
        TCompressPlugin_zstd plugin = zstdCompressPlugin;
        plugin.compress_level = clamp_level(level);
        plugin.thread_num = threads;

        std::vector<uint8_t> diff;
        create_single_compressed_diff(new_data, new_data + new_len,
                                      old_data, old_data + old_len,
                                      diff, &plugin.base,
                                      kDefaultPatchStepMemSize,
                                      kMinSingleMatchScore_default,
                                      /*isUseBigCacheMatch=*/false,
                                      /*threadNum=*/static_cast<size_t>(threads));
        if (diff.size() > out_cap) return HDP_ERR_CAPACITY;
        std::memcpy(out_diff, diff.data(), diff.size());
        return static_cast<int64_t>(diff.size());
    } catch (const std::bad_alloc&) {
        return HDP_ERR_OUT_OF_MEM;
    } catch (const std::exception&) {
        return HDP_ERR_INTERNAL;
    } catch (...) {
        return HDP_ERR_INTERNAL;
    }
}

size_t hdp_diff_bound_zstd(size_t new_len, size_t old_len) {
    // Generous upper bound: the single-compressed format stores covers
    // (position/length records) plus a zstd-compressed stream of the
    // reconstructed instructions. Worst case ~ a few copies of new_len.
    (void)old_len;
    size_t bound = new_len * 4 + (1u << 20) + 4096;
    return bound;
}

// ---------------------------------------------------------------------------
// Single-file patch
// ---------------------------------------------------------------------------
int64_t hdp_patch_zstd(const uint8_t* diff, size_t diff_len,
                      const uint8_t* old_data, size_t old_len,
                      uint8_t* out_new, size_t out_cap) {
    if (!diff || !old_data || !out_new) return HDP_ERR_NULL;
    try {
        hpatch_singleCompressedDiffInfo info;
        if (!getSingleCompressedDiffInfo_mem(&info, diff, diff + diff_len))
            return HDP_ERR_INVALID;

        const uint64_t new_size = info.newDataSize;
        if (new_size > out_cap) return HDP_ERR_CAPACITY;

        size_t step_mem = static_cast<size_t>(info.stepMemSize);
        if (step_mem < kDefaultPatchStepMemSize) step_mem = kDefaultPatchStepMemSize;
        const size_t io_cache = hpatch_kStreamCacheSize * 3;
        const size_t cache_size = step_mem + io_cache;
        std::vector<uint8_t> cache(cache_size);

        hpatch_TStreamOutput out_stream;
        hpatch_TStreamInput  old_stream;
        hpatch_TStreamInput  diff_stream;
        mem_as_hStreamOutput(&out_stream, out_new, out_new + new_size);
        mem_as_hStreamInput(&old_stream, old_data, old_data + old_len);
        mem_as_hStreamInput(&diff_stream, diff, diff + diff_len);

        hpatch_TDecompress dec = zstdDecompressPlugin;
        dec.decError = hpatch_dec_ok;

        const hpatch_BOOL ok = patch_single_compressed_diff(
            &out_stream, &old_stream, &diff_stream,
            info.diffDataPos, info.uncompressedSize, info.compressedSize,
            &dec, info.coverCount, static_cast<hpatch_size_t>(info.stepMemSize),
            cache.data(), cache.data() + cache.size(),
            /*coversListener=*/NULL, /*threadNum=*/1);
        if (!ok) {
            if (dec.decError != hpatch_dec_ok) return HDP_ERR_INVALID;
            return HDP_ERR_INTERNAL;
        }
        return static_cast<int64_t>(new_size);
    } catch (const std::bad_alloc&) {
        return HDP_ERR_OUT_OF_MEM;
    } catch (const std::exception&) {
        return HDP_ERR_INTERNAL;
    } catch (...) {
        return HDP_ERR_INTERNAL;
    }
}

int32_t hdp_diff_info_zstd(const uint8_t* diff, size_t diff_len,
                           uint64_t* out_new_size, uint64_t* out_old_size) {
    if (!diff || !out_new_size || !out_old_size) return HDP_ERR_NULL;
    try {
        hpatch_singleCompressedDiffInfo info;
        if (!getSingleCompressedDiffInfo_mem(&info, diff, diff + diff_len))
            return HDP_ERR_INVALID;
        *out_new_size = info.newDataSize;
        *out_old_size = info.oldDataSize;
        return HDP_OK;
    } catch (...) {
        return HDP_ERR_INTERNAL;
    }
}

// ---------------------------------------------------------------------------
// Directory diff
// ---------------------------------------------------------------------------
int32_t hdp_dir_diff_zstd(rust::Str old_dir, rust::Str new_dir,
                          rust::Str out_diff_path,
                          int level, int threads) {
    std::string oldPath = to_std_string(old_dir);
    std::string newPath = to_std_string(new_dir);
    std::string outDiffPath = to_std_string(out_diff_path);
    if (oldPath.empty() || newPath.empty() || outDiffPath.empty())
        return HDP_ERR_NULL;
    if (threads < 1) threads = 1;
    try {
        GlueNoIgnore ignore;

        assignDirTag(oldPath);
        assignDirTag(newPath);

        TManifest oldManifest, newManifest;
        get_manifest(&ignore, oldPath, oldManifest);
        get_manifest(&ignore, newPath, newManifest);

        TCompressPlugin_zstd plugin = zstdCompressPlugin;
        plugin.compress_level = clamp_level(level);
        plugin.thread_num = threads;

        FileStreamOutputGuard diff_out;
        if (!hpatch_TFileStreamOutput_open(&diff_out.s, outDiffPath.c_str(),
                                           hpatch_kNullStreamPos))
            return HDP_ERR_IO;
        hpatch_TFileStreamOutput_setRandomOut(&diff_out.s, hpatch_TRUE);

        THDiffSets sets;
        std::memset(&sets, 0, sizeof(sets));
        sets.isDiffInMem = hpatch_TRUE;
        sets.isSingleCompressedDiff = hpatch_TRUE;
        sets.isWindowDiff = hpatch_FALSE;
        sets.isWindowDiffMode = hpatch_FALSE;
        sets.isUseBigCacheMatch = hpatch_FALSE;
        sets.isCheckNotEqual = hpatch_FALSE;
        sets.matchScore = static_cast<size_t>(kMinSingleMatchScore_default);
        sets.patchStepMemSize = kDefaultPatchStepMemSize;
        sets.matchBlockSize = kMatchBlockSize_default;
        sets.threadNum = static_cast<size_t>(threads);
        sets.threadNumSearch_s = 1;
        sets.windowOldSize = 0;
        sets.windowNewSize = 0;
        sets.windowSegSize = 0;
        sets.bigCoverSize = 0;

        GlueDirDiffListener listener;
        dir_diff(&listener, oldManifest, newManifest, diff_out.stream(),
                 &plugin.base, &fadler32ChecksumPlugin, sets,
                 kMaxOpenFileNumber_default_diff);
        if (!hpatch_TFileStreamOutput_close(&diff_out.s)) return HDP_ERR_IO;
        // Guard's dtor calls close again, which is a safe no-op (handle nulled).
        return HDP_OK;
    } catch (const std::bad_alloc&) {
        return HDP_ERR_OUT_OF_MEM;
    } catch (const std::exception&) {
        return HDP_ERR_INTERNAL;
    } catch (...) {
        return HDP_ERR_INTERNAL;
    }
}

// ---------------------------------------------------------------------------
// Directory patch
// ---------------------------------------------------------------------------
int32_t hdp_dir_patch_zstd(rust::Str old_dir, rust::Str diff_path,
                           rust::Str out_new_dir, int threads) {
    std::string oldPath = to_std_string(old_dir);
    std::string diffPath = to_std_string(diff_path);
    std::string outNewPath = to_std_string(out_new_dir);
    if (oldPath.empty() || diffPath.empty() || outNewPath.empty())
        return HDP_ERR_NULL;
    if (threads < 1) threads = 1;
    try {
        FileStreamInputGuard diff_in;
        if (!hpatch_TFileStreamInput_open(&diff_in.s, diffPath.c_str()))
            return HDP_ERR_IO;

        TDirPatcher dp;
        TDirPatcher_init(&dp);
        int32_t rc = HDP_OK;

        const TDirDiffInfo* info = NULL;
        if (!TDirPatcher_open(&dp, diff_in.stream(), &info) || !info || !info->isDirDiff) {
            rc = HDP_ERR_INVALID;
            TDirPatcher_close(&dp);
            return rc;
        }

        hpatch_TDecompress dec = zstdDecompressPlugin;
        dec.decError = hpatch_dec_ok;
        if (!TDirPatcher_loadDirData(&dp, &dec, oldPath.c_str(), outNewPath.c_str())) {
            rc = HDP_ERR_IO;
            TDirPatcher_close(&dp);
            return rc;
        }

        const hpatch_TStreamInput* old_stream = NULL;
        if (!TDirPatcher_openOldRefAsStream(&dp, kMaxOpenFileNumber_default_patch, &old_stream)) {
            rc = HDP_ERR_IO;
            TDirPatcher_close(&dp);
            return rc;
        }

        IHPatchDirListener* hl = &defaultPatchDirlistener;
        if (!hl->patchBegin(hl, &dp)) {
            rc = HDP_ERR_INTERNAL;
        } else {
            const hpatch_TStreamOutput* new_stream = NULL;
            if (!TDirPatcher_openNewDirAsStream(&dp, &hl->base, &new_stream)) {
                rc = HDP_ERR_IO;
            } else {
                size_t step_mem = kDefaultPatchStepMemSize;
#if (_IS_NEED_SINGLE_STREAM_DIFF)
                if (info->isSingleCompressedDiff) {
                    size_t sm = static_cast<size_t>(info->sdiffInfo.stepMemSize);
                    if (sm > step_mem) step_mem = sm;
                }
#endif
                const size_t io_cache = hpatch_kStreamCacheSize * 3;
                const size_t cache_size =
                    (step_mem + io_cache) * static_cast<size_t>(threads) + (1u << 20);
                std::vector<uint8_t> cache(cache_size);

                const hpatch_BOOL ok = TDirPatcher_patch(
                    &dp, new_stream, old_stream,
                    cache.data(), cache.data() + cache.size(),
                    static_cast<size_t>(threads));
                if (!ok) {
                    if (TDirPatcher_isDiffDataChecksumError(&dp) ||
                        TDirPatcher_isOldRefDataChecksumError(&dp) ||
                        TDirPatcher_isCopyDataChecksumError(&dp) ||
                        TDirPatcher_isNewRefDataChecksumError(&dp)) {
                        rc = HDP_ERR_CHECKSUM;
                    } else {
                        rc = HDP_ERR_INTERNAL;
                    }
                }
                TDirPatcher_closeNewDirStream(&dp);
            }
            hl->patchFinish(hl, rc == HDP_OK);
        }
        TDirPatcher_closeOldRefStream(&dp);
        TDirPatcher_close(&dp);
        return rc;
    } catch (const std::bad_alloc&) {
        return HDP_ERR_OUT_OF_MEM;
    } catch (const std::exception&) {
        return HDP_ERR_INTERNAL;
    } catch (...) {
        return HDP_ERR_INTERNAL;
    }
}
