//! Integration tests covering both the single-file and directory workflows,
//! including the options for multithreaded zstd diffing.

use hdiffpatch_rs as hdp;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

fn tmp_dir(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("hdp_test_{}_{}", std::process::id(), name));
    if p.exists() {
        fs::remove_dir_all(&p).ok();
    }
    fs::create_dir_all(&p).unwrap();
    p
}

fn write_file(dir: &PathBuf, rel: &str, contents: &[u8]) {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = fs::File::create(&path).unwrap();
    f.write_all(contents).unwrap();
}

fn read_file(path: &PathBuf) -> Vec<u8> {
    fs::read(path).unwrap()
}

fn collect_files(root: &PathBuf) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn walk(root: &PathBuf, cur: &PathBuf, out: &mut Vec<(String, Vec<u8>)>) {
    for ent in fs::read_dir(cur).unwrap() {
        let ent = ent.unwrap();
        let p = ent.path();
        if p.is_dir() {
            walk(root, &p, out);
        } else {
            let rel = p
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .unwrap()
                .replace('\\', "/");
            out.push((rel, fs::read(&p).unwrap()));
        }
    }
}

fn pseudo_random(seed: u32, len: usize, mut mutate: bool) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    let mut s = seed;
    for b in buf.iter_mut() {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        *b = (s & 0xff) as u8;
    }
    if mutate {
        // tweak ~5% of bytes so there is real diff content
        let mut i = (seed as usize) % len.max(1);
        for _ in 0..(len / 20).max(1) {
            buf[i % len] = buf[i % len].wrapping_add(7);
            i = i.saturating_add(11);
        }
        let _ = &mut mutate;
    }
    buf
}

// --- single-file -----------------------------------------------------------

#[test]
fn single_file_roundtrip_various() {
    let cases: &[(&[u8], &[u8])] = &[
        (b"", b""),
        (b"", b"non-empty from empty"),
        (b"old", b""),
        (b"hello", b"hello"),
        (b"the quick brown fox", b"the quick brown fox jumps"),
        (b"aaaa", b"aaabaa"),
    ];
    for (old, new) in cases {
        let diff = hdp::diff(old, new, hdp::DiffOptions::default()).unwrap();
        let got = hdp::patch(&diff, old, Some(new.len())).unwrap();
        assert_eq!(got.as_slice(), *new, "old={:?} new={:?}", old, new);
    }
}

#[test]
fn single_file_multithreaded_high_level() {
    let old = pseudo_random(1, 256 * 1024, false);
    let new = pseudo_random(1, 256 * 1024, true);
    let diff = hdp::diff(&old, &new, hdp::DiffOptions::default().level(19).threads(4)).unwrap();
    let info = hdp::diff_info(&diff).unwrap();
    assert_eq!(info.new_size, new.len() as u64);
    assert_eq!(info.old_size, old.len() as u64);
    let got = hdp::patch(&diff, &old, Some(new.len())).unwrap();
    assert_eq!(got, new);
}

#[test]
fn single_file_into_buffer() {
    let old = b"lorem ipsum dolor sit amet";
    let new = b"lorem ipsum dolor sit amet consectetur adipiscing elit";
    let mut buf = vec![0u8; hdp::ffi::hdp_diff_bound_zstd(new.len(), old.len())];
    let n = hdp::diff_into(old, new, hdp::DiffOptions::default(), &mut buf).unwrap();
    buf.truncate(n);
    let mut out = vec![0u8; new.len()];
    let m = hdp::patch_into(&buf, old, &mut out).unwrap();
    assert_eq!(m, new.len());
    assert_eq!(&out, new);
}

#[test]
fn patch_detects_size_from_diff() {
    let old = b"some baseline content";
    let new = b"some baseline content plus extra";
    let diff = hdp::diff(old, new, hdp::DiffOptions::default()).unwrap();
    // No size hint -> lib reads it from the diff header.
    let got = hdp::patch(&diff, old, None).unwrap();
    assert_eq!(got, new);
}

// --- directory -------------------------------------------------------------

#[test]
fn dir_roundtrip_basic() {
    let old_dir = tmp_dir("old_basic");
    let new_dir = tmp_dir("new_basic");
    let out_dir = tmp_dir("out_basic");
    fs::remove_dir_all(&out_dir).ok();
    let diff_path = tmp_dir("diffs").join("basic.hdiffz");

    write_file(&old_dir, "a.txt", b"hello a");
    write_file(&old_dir, "sub/b.txt", b"hello b");
    write_file(&new_dir, "a.txt", b"hello a edited");
    write_file(&new_dir, "sub/b.txt", b"hello b");
    write_file(&new_dir, "sub/c.txt", b"brand new file c");

    hdp::diff_dir(&old_dir, &new_dir, &diff_path, hdp::DiffOptions::default()).unwrap();

    hdp::patch_dir(&old_dir, &diff_path, &out_dir, hdp::DiffOptions::default()).unwrap();

    let got = collect_files(&out_dir);
    let want = collect_files(&new_dir);
    assert_eq!(got.len(), want.len(), "file count mismatch");
    for (g, w) in got.iter().zip(want.iter()) {
        assert_eq!(g.0, w.0, "path mismatch");
        assert_eq!(g.1, w.1, "content mismatch for {}", g.0);
    }
}

#[test]
fn dir_roundtrip_multithreaded() {
    let old_dir = tmp_dir("old_mt");
    let new_dir = tmp_dir("new_mt");
    let out_dir = tmp_dir("out_mt");
    fs::remove_dir_all(&out_dir).ok();
    let diff_path = tmp_dir("diffs").join("mt.hdiffz");

    // Several files with varied, partly-shared content.
    write_file(&old_dir, "f1.bin", &pseudo_random(11, 48 * 1024, false));
    write_file(&old_dir, "f2.bin", &pseudo_random(22, 48 * 1024, false));
    write_file(
        &old_dir,
        "nested/deep/f3.bin",
        &pseudo_random(33, 32 * 1024, false),
    );
    write_file(&new_dir, "f1.bin", &pseudo_random(11, 48 * 1024, true));
    write_file(&new_dir, "f2.bin", &pseudo_random(22, 48 * 1024, false)); // unchanged
    write_file(
        &new_dir,
        "nested/deep/f3.bin",
        &pseudo_random(44, 40 * 1024, true),
    );
    write_file(
        &new_dir,
        "new_only.bin",
        &pseudo_random(99, 16 * 1024, false),
    );

    hdp::diff_dir(
        &old_dir,
        &new_dir,
        &diff_path,
        hdp::DiffOptions::default().level(19).threads(4),
    )
    .unwrap();

    hdp::patch_dir(
        &old_dir,
        &diff_path,
        &out_dir,
        hdp::DiffOptions::default().threads(2),
    )
    .unwrap();

    let got = collect_files(&out_dir);
    let want = collect_files(&new_dir);
    assert_eq!(got.len(), want.len());
    for (g, w) in got.iter().zip(want.iter()) {
        assert_eq!(g.0, w.0);
        assert_eq!(g.1, w.1, "content mismatch for {}", g.0);
    }
}

#[test]
fn dir_empty_to_populated() {
    let old_dir = tmp_dir("old_empty");
    let new_dir = tmp_dir("new_pop");
    let out_dir = tmp_dir("out_pop");
    fs::remove_dir_all(&out_dir).ok();
    let diff_path = tmp_dir("diffs").join("empty.hdiffz");

    write_file(&new_dir, "x.txt", b"from nothing");
    write_file(&new_dir, "y/z.txt", b"something");

    hdp::diff_dir(&old_dir, &new_dir, &diff_path, hdp::DiffOptions::default()).unwrap();
    hdp::patch_dir(&old_dir, &diff_path, &out_dir, hdp::DiffOptions::default()).unwrap();

    let got = collect_files(&out_dir);
    let want = collect_files(&new_dir);
    assert_eq!(got, want);
}

#[test]
fn patch_rejects_invalid_diff() {
    let bogus = b"definitely not a diff";
    let err = hdp::patch(bogus, b"old", Some(10)).unwrap_err();
    assert_eq!(err, hdp::Error::Invalid);
}
