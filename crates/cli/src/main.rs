//! The mode flag is on `scan`, not `apply`: mechanism selection happens at
//! scan, which is impossible if scan is never told the mode (§6.13).
//!
//! Task 10's slice only. `apply` lands at Task 11/12 and is deliberately
//! absent — there is nothing here that can move a file.

use clap::{Parser, Subcommand, ValueEnum};
use comic_renamer_core::corpus::{ensure_corpus, parse_manifest, Corpus};
use comic_renamer_core::overrides::Overrides;
use comic_renamer_core::types::{Mode, ScanOpts, Status};
use comic_renamer_core::{plan, scan};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Pinned at build time (§6.5): the repository holds the manifest, the tool
/// fetches and hash-verifies the corpus on first use, then caches it.
const MANIFEST: &str = include_str!("../../../corpus/manifest.json");

#[derive(Parser)]
#[command(name = "comic-renamer", version, about = "Rename comic bundles into a library layout")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read the source, resolve every filename, and write a reviewable plan.
    /// Touches nothing.
    Scan {
        source: PathBuf,
        #[arg(long)]
        dest: PathBuf,
        /// Where to write the plan. Defaults to `comic-renamer-plan.jsonl`.
        #[arg(long, short = 'o')]
        out: Option<PathBuf>,
        #[arg(long, default_value = "komga")]
        preset: String,
        #[arg(long, default_value = "en")]
        lang: String,
        #[arg(long, value_enum, default_value_t = ModeArg::Move)]
        mode: ModeArg,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum ModeArg {
    Move,
    Copy,
}

fn main() -> ExitCode {
    let Cli { command } = Cli::parse();
    match run(command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: Command) -> Result<(), String> {
    let Command::Scan {
        source,
        dest,
        out,
        preset,
        lang,
        mode,
    } = command;

    let manifest = parse_manifest(MANIFEST).map_err(|e| e.to_string())?;
    let cache = cache_dir()?;
    eprintln!("corpus: {} series, dump {}", manifest.series_count, manifest.dump_date);
    let database = ensure_corpus(&manifest, &cache).map_err(|e| e.to_string())?;
    let corpus = Corpus::open(&database, &manifest).map_err(|e| e.to_string())?;

    let overrides = match Overrides::load(Path::new("data/overrides.toml")) {
        Ok(overrides) => overrides,
        // Layer B is a floor, not a requirement: a missing file is not fatal.
        Err(_) => Overrides::default(),
    };

    let opts = ScanOpts {
        source_root: source,
        dest_root: dest,
        preset,
        mode: Some(match mode {
            ModeArg::Move => Mode::Move,
            ModeArg::Copy => Mode::Copy,
        }),
        lang,
    };

    let mut last = 0u64;
    let (header, rows) = scan::scan(&opts, &corpus, &overrides, &mut |p| {
        if p.total > 0 && (p.done == p.total || p.done >= last + 100) {
            last = p.done;
            eprint!("\rscanning {}/{}", p.done, p.total);
        }
    })
    .map_err(|e| e.to_string())?;
    eprintln!();

    let destination = out.unwrap_or_else(|| PathBuf::from("comic-renamer-plan.jsonl"));
    plan::write(&header, &rows, &destination).map_err(|e| e.to_string())?;

    summarise(&rows, header.required_bytes, header.download_bytes);
    println!("\nplan written to {}", destination.display());
    println!("Nothing has been moved. `apply` does not exist yet.");
    Ok(())
}

fn summarise(rows: &[comic_renamer_core::types::PlanRow], required: u64, download: u64) {
    let mut ready = 0usize;
    let mut review = 0usize;
    let mut failed = 0usize;
    for row in rows {
        match row.status {
            Status::Ready(_) => ready += 1,
            Status::Review(_) => review += 1,
            Status::Failed => failed += 1,
        }
    }
    let pct = |n: usize| {
        if rows.is_empty() {
            0.0
        } else {
            100.0 * n as f64 / rows.len() as f64
        }
    };
    println!("\n{} files", rows.len());
    println!("  ready   {ready:>5}  {:>5.1}%", pct(ready));
    println!("  review  {review:>5}  {:>5.1}%", pct(review));
    println!("  failed  {failed:>5}  {:>5.1}%", pct(failed));
    println!("\n  {} logical", gib(required));
    if download > 0 {
        println!("  {} would be downloaded first (cloud placeholders)", gib(download));
    }
}

fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / (1u64 << 30) as f64)
}

/// `$XDG_CACHE_HOME/comic-renamer`, or `~/Library/Caches/comic-renamer`.
fn cache_dir() -> Result<PathBuf, String> {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(xdg).join("comic-renamer"));
    }
    let home = std::env::var("HOME").map_err(|_| "neither XDG_CACHE_HOME nor HOME is set")?;
    Ok(PathBuf::from(home)
        .join("Library/Caches")
        .join("comic-renamer"))
}
