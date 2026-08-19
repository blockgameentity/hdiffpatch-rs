//! `hdiffpatch` CLI — thin command-line wrapper over `hdiffpatch-rs`.
//!
//! Subcommands:
//!   diff       Create a single-file zstd diff (old -> new).
//!   patch      Apply a single-file zstd diff to old, producing new.
//!   info       Print the old/new sizes recorded in a single-file diff.
//!   diff-dir   Create a zstd directory diff.
//!   patch-dir  Apply a zstd directory diff.
//!
//! Use `--help` on any subcommand for details.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use hdiffpatch_rs as hdp;

#[derive(Debug, Parser)]
#[command(
    name = "hdiffpatch",
    version,
    about = "Create and apply HDiffPatch zstd diffs (single-file and directory)",
    long_about = "Wraps the hdiffpatch-rs library, a safe Rust binding to HDiffPatch \
                  with zstd compression and the built-in fadler32 checksum."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// zstd compression level (0 = library default, 1..22 otherwise).
    #[arg(short, long, global = true, default_value_t = 0)]
    level: i32,

    /// Number of worker threads (default 1).
    #[arg(short, long, global = true, default_value_t = 1)]
    threads: i32,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create a single-file zstd diff that turns OLD into NEW.
    Diff {
        /// Path to the old (source) file.
        old: PathBuf,
        /// Path to the new (target) file.
        new: PathBuf,
        /// Where to write the serialized diff.
        out: PathBuf,
    },
    /// Apply a single-file zstd DIFF to OLD, reconstructing NEW at OUT.
    Patch {
        /// Path to the old (source) file.
        old: PathBuf,
        /// Path to the serialized diff.
        diff: PathBuf,
        /// Where to write the reconstructed new data.
        out: PathBuf,
    },
    /// Print the old/new sizes recorded in a single-file diff.
    Info {
        /// Path to the diff to inspect.
        diff: PathBuf,
    },
    /// Create a zstd directory diff between OLD_DIR and NEW_DIR.
    DiffDir {
        /// Old (source) directory.
        old_dir: PathBuf,
        /// New (target) directory.
        new_dir: PathBuf,
        /// Where to write the serialized dir-diff.
        out: PathBuf,
    },
    /// Apply a dir-diff, reconstructing NEW_DIR at OUT_DIR from OLD_DIR + DIFF.
    PatchDir {
        /// Old (source) directory.
        old_dir: PathBuf,
        /// Path to the serialized dir-diff.
        diff: PathBuf,
        /// Where to write the reconstructed new directory.
        out_dir: PathBuf,
    },
}

fn options(cli: &Cli) -> hdp::DiffOptions {
    hdp::DiffOptions::default()
        .level(cli.level)
        .threads(cli.threads)
}

fn run(cli: &Cli) -> Result<(), String> {
    let opts = options(cli);
    match &cli.command {
        Command::Diff { old, new, out } => {
            let old_data =
                std::fs::read(old).map_err(|e| format!("read {}: {}", old.display(), e))?;
            let new_data =
                std::fs::read(new).map_err(|e| format!("read {}: {}", new.display(), e))?;
            let diff =
                hdp::diff(&old_data, &new_data, opts).map_err(|e| format!("diff failed: {e}"))?;
            ensure_parent(out)?;
            std::fs::write(out, &diff).map_err(|e| format!("write {}: {}", out.display(), e))?;
            eprintln!(
                "diff: {} + {} -> {} bytes (level={}, threads={})",
                old.display(),
                new.display(),
                diff.len(),
                opts.level,
                opts.threads,
            );
        }
        Command::Patch { old, diff, out } => {
            let old_data =
                std::fs::read(old).map_err(|e| format!("read {}: {}", old.display(), e))?;
            let diff_data =
                std::fs::read(diff).map_err(|e| format!("read {}: {}", diff.display(), e))?;
            let new_data = hdp::patch(&diff_data, &old_data, None)
                .map_err(|e| format!("patch failed: {e}"))?;
            ensure_parent(out)?;
            std::fs::write(out, &new_data)
                .map_err(|e| format!("write {}: {}", out.display(), e))?;
            eprintln!(
                "patch: {} + {} -> {} ({} bytes)",
                old.display(),
                diff.display(),
                out.display(),
                new_data.len(),
            );
        }
        Command::Info { diff } => {
            let diff_data =
                std::fs::read(diff).map_err(|e| format!("read {}: {}", diff.display(), e))?;
            let info = hdp::diff_info(&diff_data).map_err(|e| format!("info failed: {e}"))?;
            println!("diff:  {}", diff.display());
            println!("old:   {} bytes", info.old_size);
            println!("new:   {} bytes", info.new_size);
        }
        Command::DiffDir {
            old_dir,
            new_dir,
            out,
        } => {
            ensure_parent(out)?;
            hdp::diff_dir(old_dir, new_dir, out, opts)
                .map_err(|e| format!("dir-diff failed: {e}"))?;
            let sz = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
            eprintln!(
                "dir-diff: {} + {} -> {} ({} bytes, level={}, threads={})",
                old_dir.display(),
                new_dir.display(),
                out.display(),
                sz,
                opts.level,
                opts.threads,
            );
        }
        Command::PatchDir {
            old_dir,
            diff,
            out_dir,
        } => {
            if out_dir == old_dir {
                return Err(format!(
                    "out_dir must not equal old_dir ({}); write to a fresh directory",
                    old_dir.display()
                ));
            }
            if out_dir.exists() {
                std::fs::remove_dir_all(out_dir)
                    .map_err(|e| format!("clear {}: {}", out_dir.display(), e))?;
            }
            hdp::patch_dir(old_dir, diff, out_dir, opts)
                .map_err(|e| format!("dir-patch failed: {e}"))?;
            eprintln!(
                "dir-patch: {} + {} -> {}",
                old_dir.display(),
                diff.display(),
                out_dir.display(),
            );
        }
    }
    Ok(())
}

/// Create the parent directory of `path` if it does not already exist.
fn ensure_parent(path: &std::path::Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create {}: {}", parent.display(), e))?;
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
