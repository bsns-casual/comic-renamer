//! The whole command surface (§6.13).
//!
//! The mode flag is on `scan`, not `apply`: mechanism selection happens at
//! scan, which is impossible if scan is never told the mode. `apply` therefore
//! takes no mode flag at all, which is what makes it reproducible from the
//! plan alone.
//!
//! `undo` takes the **plan**, not the manifest (D38). The manifest is still
//! the record of what happened; the plan is what anchors it.

use clap::{Parser, Subcommand, ValueEnum};
use comic_renamer_core::corpus::{ensure_corpus, parse_manifest, Corpus};
use comic_renamer_core::fsops::is_dataless;
use comic_renamer_core::overrides::Overrides;
use comic_renamer_core::types::{
    Mechanism, Mode, OverrideError, PlanRow, Progress, ScanOpts, Status, UndoReport,
};
use comic_renamer_core::{apply, komga_api, plan, scan};
use std::io::Write;
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

    /// Execute a plan. Discloses the mode and its cost, then asks for the
    /// typed word. Takes no mode flag — the plan carries it (§6.13).
    Apply {
        plan: PathBuf,
        /// A move off a cloud-sync folder is a server-side delete (D23).
        #[arg(long)]
        allow_cloud_move: bool,
        /// Skip the typed confirmation. The disclosure still prints.
        #[arg(long, short = 'y')]
        yes: bool,
        /// Refuse unless the Komga libraries here can survive a rename
        /// (§6.8.2). The token comes from `COMIC_RENAMER_KOMGA_TOKEN`, never
        /// a flag — flags land in shell history.
        #[arg(long)]
        komga_url: Option<String>,
    },

    /// Put a plan's files back. Reads the manifest beside the plan for what
    /// happened, and the plan itself for what to trust (D38).
    Undo { plan: PathBuf },
}

#[derive(Clone, Copy, ValueEnum)]
enum ModeArg {
    Move,
    Copy,
}

fn main() -> ExitCode {
    let Cli { command } = Cli::parse();
    let outcome = match command {
        Command::Scan {
            source,
            dest,
            out,
            preset,
            lang,
            mode,
        } => run_scan(source, dest, out, preset, lang, mode),
        Command::Apply {
            plan,
            allow_cloud_move,
            yes,
            komga_url,
        } => run_apply(&plan, allow_cloud_move, yes, komga_url.as_deref()),
        Command::Undo { plan } => run_undo(&plan),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

// ---------------------------------------------------------------------------
// scan
// ---------------------------------------------------------------------------

fn run_scan(
    source: PathBuf,
    dest: PathBuf,
    out: Option<PathBuf>,
    preset: String,
    lang: String,
    mode: ModeArg,
) -> Result<(), String> {
    let manifest = parse_manifest(MANIFEST).map_err(|e| e.to_string())?;
    let cache = cache_dir()?;
    eprintln!("corpus: {} series, dump {}", manifest.series_count, manifest.dump_date);
    let database = ensure_corpus(&manifest, &cache).map_err(|e| e.to_string())?;
    let corpus = Corpus::open(&database, &manifest).map_err(|e| e.to_string())?;

    // Layer B is a floor, not a requirement, so an absent file is not fatal —
    // but a *malformed* one silently resolving to no overrides at all would
    // hand back worse answers with no word about why. Only NotFound passes.
    let overrides = match Overrides::load(Path::new("data/overrides.toml")) {
        Ok(overrides) => overrides,
        Err(OverrideError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            Overrides::default()
        }
        Err(e) => return Err(format!("data/overrides.toml is unusable: {e}")),
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
    println!("Nothing has been moved. Review it, then:");
    println!("  comic-renamer apply {}", destination.display());
    Ok(())
}

fn summarise(rows: &[PlanRow], required: u64, download: u64) {
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
    println!("\n  {} logical", size(required));
    if download > 0 {
        println!("  {} would be downloaded first (cloud placeholders)", size(download));
    }
}

// ---------------------------------------------------------------------------
// apply
// ---------------------------------------------------------------------------

fn run_apply(
    path: &Path,
    allow_cloud_move: bool,
    yes: bool,
    komga_url: Option<&str>,
) -> Result<(), String> {
    // Read first, for the disclosure's arithmetic. `check` re-reads and
    // revalidates; this copy is never what decides anything.
    let (header, rows, edited) =
        plan::read_allowing_edits(path).map_err(|e| e.to_string())?;

    // Pre-flight before the user is asked anything: a plan that will refuse
    // should refuse now, not after they have typed the word.
    apply::check(path, allow_cloud_move, &mut render_progress).map_err(|e| e.to_string())?;

    // §6.8.2: hashing switched on *after* a rename cannot help, because a
    // deleted record can never acquire a hash. Refuse rather than warn.
    if let Some(url) = komga_url {
        let libraries = komga_api::check_libraries(url).map_err(|e| e.to_string())?;
        komga_api::assert_safe(&libraries).map_err(|e| {
            format!("{e}\nFix it in Komga, then scan again. Renaming now loses read progress.")
        })?;
    }

    let eligible: Vec<&PlanRow> = rows
        .iter()
        .filter(|row| matches!(row.status, Status::Ready(_)))
        .collect();
    if eligible.is_empty() {
        println!("No row is ready. Nothing to do.");
        return Ok(());
    }

    let facts = Facts::gather(header.mode, &eligible, edited).map_err(|e| e.to_string())?;
    print!("{}", disclose(&facts));
    if !yes {
        confirm(header.mode)?;
    }

    let result = apply::apply(path, allow_cloud_move, &mut render_progress)
        .map_err(|e| e.to_string())?;
    println!(
        "\n{} placed · {} skipped · {} failed",
        result.executed, result.skipped, result.failed
    );
    println!("undo manifest: {}", apply::undo_path(path).display());
    println!("  comic-renamer undo {}", path.display());
    Ok(())
}

fn render_progress(p: Progress) {
    // `total: 0` is how core marks a message rather than a count.
    if p.total == 0 {
        eprintln!("\r{}", p.current);
    } else if p.done == p.total {
        eprintln!("\rapplying {}/{}", p.done, p.total);
    } else {
        eprint!("\rapplying {}/{}", p.done, p.total);
    }
}

/// What the disclosure states, measured rather than assumed.
///
/// Totals are over the **eligible** rows, never the plan's header: a scan of
/// 412 files whose review queue holds 150 of them would otherwise disclose a
/// payload `apply` is not going to touch.
struct Facts {
    mode: Mode,
    files: usize,
    bytes: u64,
    /// What `apply` will demand is free. Not the payload: a clone is
    /// copy-on-write and costs nothing, but `select_mechanism` picks Clone
    /// without probing, so the reserve has to cover the streaming fallback.
    reserve: u64,
    cross_volume: bool,
    placeholders: usize,
    placeholder_bytes: u64,
    edited: bool,
}

impl Facts {
    fn gather(mode: Mode, eligible: &[&PlanRow], edited: bool) -> std::io::Result<Facts> {
        let mut placeholders = 0usize;
        let mut placeholder_bytes = 0u64;
        for row in eligible {
            // Re-read rather than trust the scan: a placeholder hydrated since
            // then is no longer a download, and the user is owed the current
            // number. An error here means the tree moved under us, which
            // `apply`'s own re-stat is about to refuse — so it propagates.
            if is_dataless(&row.source)? {
                placeholders += 1;
                placeholder_bytes += row.source_stat.size;
            }
        }
        Ok(Facts {
            mode,
            files: eligible.len(),
            bytes: eligible.iter().map(|row| row.source_stat.size).sum(),
            reserve: apply::required_bytes(eligible),
            cross_volume: eligible
                .iter()
                .any(|row| row.mechanism == Mechanism::Stream),
            placeholders,
            placeholder_bytes,
            edited,
        })
    }
}

/// §6.10: the mode, its consequence, disk arithmetic and **download cost
/// separately** — not a flag in `--help`.
///
/// Departs from §6.10's example block in two places, both recorded in the
/// session notes: `--hardlink` is not advertised, because D27 defers it out
/// of Phase 1 and printing it would offer a flag that does not exist; and the
/// `--copy` line states the reserve rather than "0 GiB extra disk", because
/// `required_bytes` counts Clone rows and would otherwise refuse a run this
/// block had just promised was free.
fn disclose(f: &Facts) -> String {
    let mut out = String::from("\n");
    let payload = format!("{} · {}", plural(f.files, "file"), size(f.bytes));

    match (f.mode, f.cross_volume) {
        (Mode::Move, false) => {
            out.push_str("  MOVE MODE — files will be relocated, not copied.\n\n");
            out.push_str(&format!(
                "  {payload} · same volume → instant, no extra disk used\n"
            ));
            out.push_str("  Originals will no longer exist at their current paths.\n");
        }
        (Mode::Move, true) => {
            out.push_str("  MOVE MODE, ACROSS VOLUMES — copied, verified, then deleted.\n\n");
            out.push_str(&format!(
                "  {payload} · {} must be free at the destination\n",
                size(f.reserve)
            ));
            out.push_str("  Originals will no longer exist at their current paths.\n");
        }
        (Mode::Copy, false) => {
            out.push_str("  COPY MODE — originals are kept where they are.\n\n");
            out.push_str(&format!(
                "  {payload} · same volume → cloned on APFS, costing nothing\n"
            ));
            out.push_str(&format!(
                "  {} must still be free, in case a clone is not available here.\n",
                size(f.reserve)
            ));
        }
        (Mode::Copy, true) => {
            out.push_str("  COPY MODE, ACROSS VOLUMES — streamed.\n\n");
            out.push_str(&format!(
                "  {payload} · {} of extra disk is needed at the destination\n",
                size(f.reserve)
            ));
        }
    }

    // Reported apart from disk, always. A plan whose sources are dataless
    // transfers these bytes whatever mechanism is chosen, and "no extra disk
    // used" beside a silent 6 GiB download is the quiet surprise §4 forbids.
    if f.placeholders > 0 {
        out.push_str(&format!(
            "\n  {} — {} will be DOWNLOADED first.\n",
            if f.placeholders == 1 {
                "1 file is a cloud placeholder".to_string()
            } else {
                format!("{} files are cloud placeholders", f.placeholders)
            },
            size(f.placeholder_bytes)
        ));
    }

    if f.mode == Mode::Move {
        // `bytes`, not `reserve`: this line is about the run the user has
        // *not* chosen. Under --copy every same-volume row becomes a Clone and
        // Clone rows count toward the reserve, so the number that would gate
        // that run is the whole payload — `reserve` is 0 here precisely because
        // a rename needs nothing, and printing it promised a free copy.
        out.push_str(&format!(
            "\n  --copy       keeps originals — clones where it can, {} free needed\n",
            size(f.bytes)
        ));
    }

    if f.edited {
        out.push_str(
            "\n  This plan has been edited since the scan. Every destination is\n  \
             recomputed from its metadata, so an edited title is honoured and an\n  \
             edited path is ignored.\n",
        );
    }
    out.push('\n');
    out
}

/// Typed confirmation (§6.10).
///
/// §6.10 scopes this to "first run and whenever the plan is cross-volume".
/// Nothing persists a notion of first run, and the cheapest home for one — a
/// marker in the cache directory — is state that exists only to ask less. So
/// every run asks, which is stricter than the spec rather than looser, and
/// `--yes` is the explicit way out for a script. Recorded as a departure.
fn confirm(mode: Mode) -> Result<(), String> {
    let word = match mode {
        Mode::Move => "MOVE",
        Mode::Copy => "COPY",
    };
    print!("  Type {word} to continue: ");
    std::io::stdout().flush().map_err(|e| e.to_string())?;

    let mut typed = String::new();
    // EOF — a pipe, or `< /dev/null` — reads as a refusal, which is the safe
    // way for a non-interactive caller to fail. `--yes` is how it consents.
    let read = std::io::stdin().read_line(&mut typed).map_err(|e| e.to_string())?;
    // Piped, there is no echoed newline, so whatever comes next — a refusal,
    // or the first progress line — ran into the prompt it was answering.
    println!();
    if read == 0 || typed.trim() != word {
        return Err(if read == 0 {
            "aborted: nothing was typed (use --yes to confirm non-interactively)".into()
        } else {
            "aborted at the prompt; nothing has been moved".to_string()
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// undo
// ---------------------------------------------------------------------------

fn run_undo(path: &Path) -> Result<(), String> {
    let report = apply::undo(path).map_err(|e| e.to_string())?;
    let UndoReport {
        restored,
        skipped,
        refused,
    } = &report;
    println!("{restored} restored · {skipped} skipped · {} refused", refused.len());
    if refused.is_empty() {
        return Ok(());
    }
    // A refusal means a file is still in the library and undo could not put
    // it back. Exiting zero would tell a script the reverse was clean.
    for why in refused {
        eprintln!("  refused: {why}");
    }
    Err(format!("{} rows could not be reversed", refused.len()))
}

// ---------------------------------------------------------------------------
// shared
// ---------------------------------------------------------------------------

fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// Picks a unit. `{:.1} GiB` alone prints "0.0 GiB" for a real payload, and a
/// safety disclosure that reports zero bytes for a non-empty run is worse than
/// one that reports an awkward unit.
fn size(bytes: u64) -> String {
    const UNITS: [(u64, &str); 3] = [(1 << 30, "GiB"), (1 << 20, "MiB"), (1 << 10, "KiB")];
    for (scale, unit) in UNITS {
        if bytes >= scale {
            return format!("{:.1} {unit}", bytes as f64 / scale as f64);
        }
    }
    format!("{bytes} bytes")
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

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    fn facts(mode: Mode, cross_volume: bool) -> Facts {
        Facts {
            mode,
            files: 412,
            bytes: 31 * GIB,
            reserve: 31 * GIB,
            cross_volume,
            placeholders: 0,
            placeholder_bytes: 0,
            edited: false,
        }
    }

    #[test]
    fn disclosure_separates_disk_from_download() {
        let mut f = facts(Mode::Move, false);
        f.placeholders = 38;
        f.placeholder_bytes = 6 * GIB;
        let out = disclose(&f);
        assert!(out.contains("MOVE MODE"), "{out}");
        assert!(out.contains("no extra disk used"), "{out}");
        // The two numbers are different and both are stated.
        assert!(out.contains("38 files are cloud placeholders"), "{out}");
        assert!(out.contains("6.0 GiB will be DOWNLOADED first"), "{out}");
        assert!(out.contains("31.0 GiB"), "{out}");
    }

    #[test]
    fn a_move_says_the_originals_go_away() {
        for cross in [false, true] {
            let out = disclose(&facts(Mode::Move, cross));
            assert!(
                out.contains("Originals will no longer exist"),
                "cross={cross}: {out}"
            );
        }
    }

    #[test]
    fn a_copy_says_the_originals_are_kept_and_never_advertises_copy() {
        for cross in [false, true] {
            let out = disclose(&facts(Mode::Copy, cross));
            assert!(!out.contains("Originals will no longer exist"), "{out}");
            // Already in copy mode; offering --copy as the alternative is noise.
            assert!(!out.contains("--copy "), "{out}");
        }
    }

    #[test]
    fn the_disclosure_never_advertises_a_flag_that_does_not_exist() {
        // D27 defers --hardlink out of Phase 1. §6.10's example block lists
        // it; printing it would offer a flag clap rejects.
        for mode in [Mode::Move, Mode::Copy] {
            for cross in [false, true] {
                assert!(!disclose(&facts(mode, cross)).contains("hardlink"));
            }
        }
    }

    #[test]
    fn a_cross_volume_move_states_the_space_it_needs() {
        let out = disclose(&facts(Mode::Move, true));
        assert!(out.contains("ACROSS VOLUMES"), "{out}");
        assert!(out.contains("31.0 GiB must be free"), "{out}");
        assert!(!out.contains("no extra disk used"), "{out}");
    }

    #[test]
    fn an_edited_plan_is_disclosed_as_edited() {
        let mut f = facts(Mode::Move, false);
        f.edited = true;
        assert!(disclose(&f).contains("edited since the scan"));
    }

    #[test]
    fn one_file_is_not_1_files() {
        // A disclosure that cannot count undermines the numbers beside it.
        let mut f = facts(Mode::Move, false);
        f.files = 1;
        f.placeholders = 1;
        f.placeholder_bytes = 1 << 20;
        let out = disclose(&f);
        assert!(out.contains("1 file ·"), "{out}");
        assert!(!out.contains("1 files"), "{out}");
        assert!(out.contains("1 file is a cloud placeholder"), "{out}");
    }

    #[test]
    fn the_copy_advertisement_quotes_the_payload_not_this_run_s_reserve() {
        // A same-volume move reserves nothing, so quoting `reserve` here
        // advertised "0 bytes free needed" for a copy that would demand the
        // whole payload — every Rename row becomes a Clone, and Clone rows
        // count toward the gate.
        let mut f = facts(Mode::Move, false);
        f.reserve = 0;
        let out = disclose(&f);
        assert!(out.contains("--copy"), "{out}");
        assert!(!out.contains("0 bytes free needed"), "{out}");
        assert!(out.contains("31.0 GiB free needed"), "{out}");
    }

    #[test]
    fn size_never_reports_zero_for_a_non_empty_payload() {
        // `{:.1} GiB` printed "0.0 GiB" for every fixture-sized run.
        assert_eq!(size(0), "0 bytes");
        assert_eq!(size(1), "1 bytes");
        assert_eq!(size(4096), "4.0 KiB");
        assert_eq!(size(5 * (1 << 20)), "5.0 MiB");
        assert_eq!(size(31 * GIB + GIB / 5), "31.2 GiB");
    }
}
