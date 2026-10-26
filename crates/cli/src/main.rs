//! The whole command surface (§6.13).
//!
//! The mode flag is on `scan`, not `apply`: mechanism selection happens at
//! scan, which is impossible if scan is never told the mode. `apply` therefore
//! takes no mode flag at all, which is what makes it reproducible from the
//! plan alone.
//!
//! The record of what a run did lives in a directory the tool owns, not
//! beside the plan. `apply` reads the plan once, holds the library's lock
//! across the disclosure and the prompt, and hands `apply` the copy the
//! disclosure was computed from — so what runs is what the user was shown,
//! whatever happens to the file on disk in between.

use clap::{Parser, Subcommand};
use comic_renamer_core::fsops::{find_collisions, is_dataless};
use comic_renamer_core::corpus::{ensure_corpus, parse_manifest, Corpus};
use comic_renamer_core::mapping::Mappings;
use comic_renamer_core::overrides::Overrides;
use comic_renamer_core::preset::{preset_by_id, Preset};
use comic_renamer_core::resolve::{context_for, suggest_with};
use comic_renamer_core::types::{
    human_bytes as size, Aside, Collision, Mechanism, Mode, PlanRow, Progress, ScanOpts,
    Status,
    UndoReport,
};
use comic_renamer_core::{apply, komga_api, plan, record, scan, undo};
mod map;
mod menu;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The mappings that ship in the binary. Compiled in for the same reason as
/// the manifest below: nothing the binary needs is read from the working
/// directory.
use comic_renamer_core::mapping::shipped_mappings;

/// Pinned at build time (§6.5): the repository holds the manifest, the tool
/// fetches and hash-verifies the corpus on first use, then caches it.
const MANIFEST: &str = include_str!("../../../corpus/manifest.json");

/// Compiled in, like the manifest, and for the same reason: a path resolved
/// against the working directory is not something an installed binary can
/// rely on. Layer B is first-party data that CI validates (§6.2.1); a
/// user-supplied overrides file is Phase 3, when §6.11's caps on contributed
/// rule data exist to make it safe.
const OVERRIDES: &str = include_str!("../../../data/overrides.toml");

/// Enough to act on, not a wall of text. The rest are counted.
const COLLISIONS_SHOWN: usize = 5;

#[derive(Parser)]
#[command(name = "comic-renamer", version, about = "Rename comic bundles into a library layout")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Identify every file and write a plan. Touches nothing.
    Scan {
        /// The folder of comics to rename. Not searched recursively.
        source: PathBuf,
        /// Where the renamed library goes. Must already exist.
        #[arg(long)]
        dest: PathBuf,
        /// Where to write the plan [default: comic-renamer-plan.jsonl]
        #[arg(long, short = 'o')]
        out: Option<PathBuf>,
        /// Which library app's layout to produce.
        #[arg(long, default_value = "komga")]
        preset: String,
        /// Which language's editions to match. Never falls back to another.
        #[arg(long, default_value = "en")]
        lang: String,
        /// Which marketplace the files came from. Decides how an unmapped
        /// file is guessed at; only Humble consults the corpus.
        // shortcut: a flag with a Humble default (D12: bundles are the
        // evaluation input); the mapping file will record it per file (§5.3.1).
        #[arg(long, default_value = "Humble")]
        marketplace: String,
        /// Relocate the files; the originals stop existing. The default.
        #[arg(long = "move")]
        move_: bool,
        /// Keep the originals. Clones on APFS, so it usually costs nothing.
        #[arg(long, conflicts_with = "move_")]
        copy: bool,
    },

    /// Identify every file, then walk you through the ones the database does
    /// not know. Writes the same plan `scan` does. Needs a terminal.
    Map {
        /// The folder of comics to rename. Not searched recursively.
        source: PathBuf,
        /// Where the renamed library goes. Must already exist.
        #[arg(long)]
        dest: PathBuf,
        /// Where to write the plan [default: comic-renamer-plan.jsonl]
        #[arg(long, short = 'o')]
        out: Option<PathBuf>,
        /// Which library app's layout to produce.
        #[arg(long, default_value = "komga")]
        preset: String,
        /// Which language's editions to match. Never falls back to another.
        #[arg(long, default_value = "en")]
        lang: String,
        /// Which marketplace the files came from. Decides how an unmapped
        /// file is guessed at; only Humble consults the corpus.
        #[arg(long, default_value = "Humble")]
        marketplace: String,
        /// Relocate the files; the originals stop existing. The default.
        #[arg(long = "move")]
        move_: bool,
        /// Keep the originals. Clones on APFS, so it usually costs nothing.
        #[arg(long, conflicts_with = "move_")]
        copy: bool,
    },

    // The mode flag is on `scan`, not here: §6.10 selects a per-row mechanism
    // during the scan, which is impossible if scan is never told the mode.
    // That is what makes `apply` reproducible from the plan alone (§6.13).
    /// Carry out a plan. States what it will do, then asks you to type the
    /// mode to confirm.
    Apply {
        /// The plan written by `scan`.
        plan: PathBuf,
        // D23: on a cloud-sync source a move is a server-side delete, and
        // cloning a placeholder silently hydrates it.
        /// Permit a move out of a cloud-sync folder, where it deletes the
        /// server's copy too.
        #[arg(long)]
        allow_cloud_move: bool,
        /// Confirm without being asked. What would have been disclosed is
        /// still printed.
        #[arg(long, short = 'y')]
        yes: bool,
        // §6.8.2: hashing switched on after a rename cannot help, because a
        // deleted record can never acquire a hash (D35). The token comes from
        // the environment, never a flag — flags land in shell history.
        /// Refuse to run unless this Komga server is configured to survive a
        /// rename. Reads `COMIC_RENAMER_KOMGA_TOKEN` for the API key.
        #[arg(long)]
        komga_url: Option<String>,
    },

    /// Put a run's files back where they came from.
    Undo {
        /// The run to reverse, as `--list` shows it. Without one, the most
        /// recent run that is not still going.
        run: Option<String>,
        /// Show the runs that can be reversed, and reverse nothing.
        #[arg(long, conflicts_with = "run")]
        list: bool,
    },
}

fn main() -> ExitCode {
    let Cli { command } = Cli::parse();
    let outcome = match command {
        None => records_dir().and_then(|records| menu::run(&records)),
        Some(command) => run_command(command),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run_command(command: Command) -> Result<(), String> {
    match command {
        Command::Scan {
            source,
            dest,
            out,
            preset,
            lang,
            marketplace,
            // Move is the default, so `--move` states it rather than
            // changing it. clap enforces that the two cannot both be set.
            move_: _,
            copy,
        } => run_scan(
            source,
            dest,
            out,
            preset,
            lang,
            marketplace,
            if copy { Mode::Copy } else { Mode::Move },
        ),
        Command::Map {
            source,
            dest,
            out,
            preset,
            lang,
            marketplace,
            move_: _,
            copy,
        } => map::run(
            source,
            dest,
            out,
            preset,
            lang,
            marketplace,
            if copy { Mode::Copy } else { Mode::Move },
        ),
        Command::Apply {
            plan,
            allow_cloud_move,
            yes,
            komga_url,
        } => run_apply(&plan, allow_cloud_move, yes, komga_url.as_deref()).map(|_| ()),
        Command::Undo { run, list } => run_undo(run.as_deref(), list),
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
    marketplace: String,
    mode: Mode,
) -> Result<(), String> {
    // NFC, as `map`'s `bundle_for` and `scan::scan` both do. Only the
    // publisher hint reads it un-squashed, so this buys a better suggestion
    // rather than correctness -- but one of three call sites left composed
    // differently is the drift the folder-name finding punished.
    let folder = comic_renamer_core::mapping::bundle_name(
        &source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    let guess_lang = lang.clone();
    // shortcut: shipped mappings only; the user's local file arrives with the
    // `map` command, which also owns where it lives.
    let mappings = Mappings::load(&shipped_mappings(), None).map_err(|e| e.to_string())?;
    // Rejected up front: a malformed embedded file is a bad commit, not a
    // condition to degrade around.
    let overrides = parse_overrides(OVERRIDES)?;

    let opts = ScanOpts {
        source_root: source,
        dest_root: dest,
        preset,
        mode: Some(mode),
        lang,
    };

    let mut last = 0u64;
    let scan::ScanOutcome { header, rows, unmapped } = scan::scan(&opts, &mappings, &mut |p| {
        if p.total > 0 && (p.done == p.total || p.done >= last + 100) {
            last = p.done;
            eprint!("\rscanning {}/{}", p.done, p.total);
        }
    })
    .map_err(|e| e.to_string())?;
    eprintln!();

    let destination = out.unwrap_or_else(|| PathBuf::from("comic-renamer-plan.jsonl"));
    plan::write(&header, &rows, &destination).map_err(|e| e.to_string())?;

    summarise(&rows, &unmapped, header.required_bytes, header.download_bytes);
    suggest_for(&unmapped, &marketplace, &folder, &guess_lang, &overrides);
    println!("\nplan written to {}", destination.display());
    println!("Nothing has been moved. Review it, then:");
    println!("  comic-renamer apply {}", destination.display());
    Ok(())
}

/// Guesses for the unmapped files, printed and never saved (D49). The corpus is
/// fetched here and only here: a session with nothing unmapped never needs it,
/// and a fetch that fails costs the guesses, not the scan.
fn guesses_for(
    unmapped: &[PathBuf],
    marketplace: &str,
    folder: &str,
    lang: &str,
    overrides: &Overrides,
) -> Vec<(String, Vec<comic_renamer_core::types::Suggestion>)> {
    if unmapped.is_empty() {
        return Vec::new();
    }
    let corpus = if marketplace.to_ascii_lowercase().starts_with("humble") {
        open_corpus().map_err(|e| eprintln!("no corpus, guessing from filenames only: {e}")).ok()
    } else {
        None
    };
    // §6.3's folder-name wiring lives in core, where a test with a corpus can
    // hold it: both the Layer B key and the publisher hint come from `folder`.
    let ctx = context_for(folder, lang, overrides, corpus.as_ref());
    unmapped
        .iter()
        .map(|path| {
            let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            let found = suggest_with(&stem, marketplace, corpus.as_ref(), &ctx);
            (stem, found)
        })
        .collect()
}

fn suggest_for(unmapped: &[PathBuf], marketplace: &str, folder: &str, lang: &str, overrides: &Overrides) {
    let guesses = guesses_for(unmapped, marketplace, folder, lang, overrides);
    if guesses.is_empty() {
        return;
    }
    println!("
guesses for the unmapped files (nothing is saved):");
    for (stem, found) in guesses.iter().take(COLLISIONS_SHOWN) {
        let shown: Vec<String> = found.iter().map(|s| s.series.clone()).collect();
        let shown = if shown.is_empty() { "no guess".to_string() } else { shown.join(" | ") };
        println!("    {stem}  ->  {shown}");
    }
}

fn parse_overrides(text: &str) -> Result<Overrides, String> {
    Overrides::parse(text).map_err(|e| format!("built-in overrides are malformed: {e}"))
}

fn open_corpus() -> Result<Corpus, String> {
    let manifest = parse_manifest(MANIFEST).map_err(|e| e.to_string())?;
    let db = ensure_corpus(&manifest, &cache_dir()?).map_err(|e| e.to_string())?;
    Corpus::open(&db, &manifest).map_err(|e| e.to_string())
}

fn summarise(rows: &[PlanRow], unmapped: &[PathBuf], required: u64, download: u64) {
    let total = rows.len() + unmapped.len();
    let pct = |n: usize| {
        if total == 0 {
            0.0
        } else {
            100.0 * n as f64 / total as f64
        }
    };
    println!("\n{total} files");
    println!("  mapped    {:>5}  {:>5.1}%", rows.len(), pct(rows.len()));
    println!("  unmapped  {:>5}  {:>5.1}%", unmapped.len(), pct(unmapped.len()));
    // Named, not just counted: an unmapped file is left where it is, and the
    // user needs to know which ones.
    for path in unmapped.iter().take(COLLISIONS_SHOWN) {
        println!("    {}", path.file_name().unwrap_or_default().to_string_lossy());
    }
    if unmapped.len() > COLLISIONS_SHOWN {
        println!("    ...and {} more", unmapped.len() - COLLISIONS_SHOWN);
    }
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
) -> Result<Option<PathBuf>, String> {
    // Read **once**. Everything below — the pre-flight, the disclosure's
    // arithmetic, and the run itself — works from this copy, so a plan
    // rewritten while the user reads the disclosure cannot be what runs.
    let (header, rows, edited) =
        plan::read_allowing_edits(path).map_err(|e| e.to_string())?;

    // Held across the pre-flight, the prompt and the run: another process
    // starting on this library mid-prompt would otherwise be refused only
    // after the user had consented.
    let records = records_dir()?;
    let lock = apply::Lock::library(&records, &header.dest_root).map_err(|e| e.to_string())?;

    // Pre-flight before the user is asked anything: a plan that will refuse
    // should refuse now, not after they have typed the word.
    apply::check(&header, &rows, allow_cloud_move, &mut render_progress)
        .map_err(|e| e.to_string())?;

    // §6.8.2: hashing switched on *after* a rename cannot help, because a
    // deleted record can never acquire a hash. Refuse rather than warn.
    if let Some(url) = komga_url {
        let libraries = komga_api::check_libraries(url).map_err(|e| e.to_string())?;
        komga_api::assert_safe(&libraries).map_err(|e| {
            format!("{e}\nTurn hashing on and trash-emptying off in Komga, let the hash\ntasks drain, then run apply again. The plan is still good. Renaming\nnow would lose read progress, read lists and thumbnails.")
        })?;
    }

    let eligible: Vec<&PlanRow> = rows
        .iter()
        .filter(|row| row.status == Status::Mapped)
        .collect();

    // Canonical, like `apply`'s: an uncanonicalised root walks a different
    // tree through a symlinked library and would miss an existing file.
    let dest_root = std::fs::canonicalize(&header.dest_root)
        .map_err(|e| format!("{}: {e}", header.dest_root.display()))?;
    let preset = preset_by_id(&header.preset)
        .ok_or_else(|| format!("unknown preset {:?}", header.preset))?;
    let facts = Facts::gather(
        header.mode,
        &eligible,
        &rows,
        header.unmapped,
        &dest_root,
        preset.as_ref(),
        edited,
    )
    .map_err(|e| e.to_string())?;
    if facts.files == 0 {
        // Nothing to consent to, so no mode block and no prompt: a MOVE
        // prompt over a run that will move nothing teaches the user that the
        // prompt does not mean anything.
        print!("{}", nothing_to_do(&facts));
        if rows.is_empty() {
            return Ok(None);
        }
        // A caller cannot tell "the library now matches the plan" from "it
        // does not" out of an exit code that is zero either way.
        return Err("nothing was placed".into());
    }

    print!("{}", disclose(&facts));
    if !yes {
        confirm(header.mode)?;
    }

    let result = apply::apply(
        &lock,
        &header,
        &rows,
        &records,
        allow_cloud_move,
        &mut render_progress,
    )
    .map_err(|e| e.to_string())?;
    println!(
        "\n{} placed · {} skipped · {} failed",
        result.executed, result.skipped, result.failed
    );
    // States that the run can be reversed, and names no path. A record is a
    // file the tool owns; printing where it lives invites it being handed
    // back in, and a path a user supplies is exactly the authority the record
    // store exists to take away.
    if result.record.is_some() {
        println!("  comic-renamer undo    puts this run back");
    }
    Ok(result.record)
}

/// The record store: `$XDG_CONFIG_HOME/comic-renamer/runs`, or
/// `~/Library/Application Support/comic-renamer/runs`.
fn records_dir() -> Result<PathBuf, String> {
    Ok(map::config_dir()?.join("runs"))
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
/// Every total is over the rows `apply` will actually run — not the plan's
/// header, and not the ready rows either. Two sets get dropped on the way and
/// both have to be subtracted before a number is honest: rows still in review,
/// and rows `apply` refuses for a collision. `files` counted the ready rows
/// and so promised a file that a destination collision was about to skip.
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
    /// Rows this run will not touch. §6.9 measures one to two rows in three
    /// needing a human, so this is the normal case and not an edge one — a
    /// disclosure that counted only the ready rows would leave a third of the
    /// bundle sitting in the source with no word about why.
    not_ready: usize,
    /// Rows `apply` will refuse, and why — a path already in the library, or
    /// two rows wanting one name. §6.10 calls merging into a library that
    /// already holds `Hellboy/` the common case, not the rare one, so a
    /// disclosure that reported the count without the reason would leave the
    /// user with "0 placed · 1 skipped" and nothing to act on.
    collisions: Vec<String>,
    edited: bool,
}

impl Facts {
    fn gather(
        mode: Mode,
        eligible: &[&PlanRow],
        rows: &[PlanRow],
        unmapped: u64,
        dest_root: &Path,
        preset: &dyn Preset,
        edited: bool,
    ) -> std::io::Result<Facts> {
        // The same call `apply` makes, on the same inputs, so the two cannot
        // disagree about which rows are going to run.
        let found = find_collisions(rows, dest_root, preset);
        let colliding: Vec<&str> = found
            .iter()
            .flat_map(|c| c.row_ids.iter().map(String::as_str))
            .collect();
        let runnable: Vec<&PlanRow> = eligible
            .iter()
            .filter(|row| !colliding.contains(&row.id.as_str()))
            .copied()
            .collect();

        let describe = |c: &Collision| match &c.existing {
            // The path already in the library, in the library's own terms.
            Some(_) => format!(
                "{} is already in the library",
                preview_of(rows, &c.row_ids).unwrap_or_else(|| c.key.clone())
            ),
            None if c.row_ids.len() > 1 => format!(
                "{} rows want one name: {}",
                c.row_ids.len(),
                preview_of(rows, &c.row_ids).unwrap_or_else(|| c.key.clone())
            ),
            None => format!("cannot be placed: {}", c.key),
        };

        let mut placeholders = 0usize;
        let mut placeholder_bytes = 0u64;
        for row in &runnable {
            // Re-read rather than trust the scan: a placeholder hydrated since
            // then is no longer a download, and the user is owed the current
            // number. An error here means the tree moved under us, which
            // `apply`'s own re-stat is about to refuse — so it propagates.
            //
            // shortcut: this accumulation is unprovable locally. `SF_DATALESS`
            // is kernel-managed and `chflags` cannot set it, so no generated
            // fixture is dataless and a mutation that reports zero
            // placeholders survives the suite. `is_dataless` itself is pinned
            // in core against both sparse cases (invariant 8); what is not
            // pinned is that these two lines add up. Verified by hand against
            // a real Dropbox placeholder during D23.
            if is_dataless(&row.source)? {
                placeholders += 1;
                placeholder_bytes += row.source_stat.size;
            }
        }
        Ok(Facts {
            mode,
            files: runnable.len(),
            bytes: runnable.iter().map(|row| row.source_stat.size).sum(),
            reserve: apply::required_bytes(&runnable),
            cross_volume: runnable
                .iter()
                .any(|row| row.mechanism == Mechanism::Stream),
            placeholders,
            placeholder_bytes,
            // Rows `apply` will not run, plus the files `scan` never planned.
            not_ready: (rows.len() - eligible.len()).saturating_add(unmapped as usize),
            // Only the ones that cost this run a row. `find_collisions`
            // scans every row, so two review rows wanting one name would
            // otherwise be reported as a refusal of work nobody asked for.
            collisions: found
                .iter()
                .filter(|c| {
                    c.row_ids
                        .iter()
                        .any(|id| eligible.iter().any(|row| &row.id == id))
                })
                .map(describe)
                .collect(),
            edited,
        })
    }
}

/// The plan's own `destination_preview` for the first of `ids`, which is what
/// that field is for: display data a reviewer reads, never an instruction
/// anything acts on (D14).
fn preview_of(rows: &[PlanRow], ids: &[String]) -> Option<String> {
    let first = ids.first()?;
    let row = rows.iter().find(|row| &row.id == first)?;
    Some(row.destination_preview.display().to_string())
}

/// Why a run that will place nothing is going to place nothing. The mode
/// block is deliberately absent: there is no consequence to disclose and
/// nothing to type a word over.
fn nothing_to_do(f: &Facts) -> String {
    let mut out = String::from("\n  Nothing to do — not one row can be placed.\n");
    if f.not_ready > 0 {
        out.push_str(&format!(
            "\n  Needs a human: {}. Edit the plan's metadata, not its paths.\n",
            plural(f.not_ready, "row")
        ));
    }
    out.push_str(&collision_lines(f));
    out.push('\n');
    out
}

/// Listed, not just counted. §6.10 refuses the row and never overwrites or
/// auto-suffixes, so what the user does next depends entirely on which row it
/// was and why — "0 placed · 1 skipped" on its own is unactionable.
fn collision_lines(f: &Facts) -> String {
    if f.collisions.is_empty() {
        return String::new();
    }
    let mut out = format!(
        "\n  Refused and left alone: {}.\n",
        plural(f.collisions.len(), "row")
    );
    for reason in f.collisions.iter().take(COLLISIONS_SHOWN) {
        out.push_str(&format!("    {reason}\n"));
    }
    if f.collisions.len() > COLLISIONS_SHOWN {
        out.push_str(&format!(
            "    ...and {} more\n",
            f.collisions.len() - COLLISIONS_SHOWN
        ));
    }
    out
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

    // §6.9 measures one to two rows in three needing a human, so this is the
    // normal case. Counting only the ready rows would leave a third of the
    // bundle in the source with no word about why.
    if f.not_ready > 0 {
        out.push_str(&format!(
            // Phrased without a verb that has to agree: "1 of 2 rows are" is
            // the same defect as "1 files", and the count genuinely reaches 1.
            "\n  Not ready, so left where they are: {} of {} rows.\n",
            f.not_ready,
            f.not_ready.saturating_add(f.files)
        ));
    }

    out.push_str(&collision_lines(f));

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
    // The disclosure is printed by the caller into a buffered stdout, so it
    // is pushed out *first* and the typeahead discarded *after* — anything
    // typed from here on was typed by someone looking at the disclosure.
    // Flushing in the other order leaves the window the discard exists to
    // close. See `fsops::discard_typeahead`.
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    comic_renamer_core::fsops::discard_typeahead();
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

fn run_undo(run: Option<&str>, list: bool) -> Result<(), String> {
    let records = records_dir()?;
    if list {
        return list_runs(&records);
    }
    let Some(record) = pick_record(&records, run)? else {
        // "Nothing to undo" and "a run is writing right now" are different
        // answers, and the second arrives at exactly the moment someone
        // changes their mind: 156 of 600 files already moved, and this
        // printed `0 restored · 0 skipped · 0 refused` and exited 0.
        if record::any_live(&records).unwrap_or(false) {
            return Err(
                "a run is in progress; it holds the library, so wait for it to finish \
                 and then undo"
                    .into(),
            );
        }
        // Nothing to reverse: never applied, or already reversed. Idempotent,
        // not an error.
        return print_report(&UndoReport::default(), None);
    };
    // The record names its own library. The same lock `apply` runs under, so
    // a record being written by a live run is not reversed under it.
    reverse_path(&records, &record)
}

/// Reverses one record under its library's lock. Shared by `undo` and the menu.
fn reverse_path(records: &Path, record: &Path) -> Result<(), String> {
    let summary = undo::summarise(record).map_err(|e| e.to_string())?;
    let lock = apply::Lock::library(records, &summary.dest_root).map_err(|e| e.to_string())?;
    let report = apply::reverse_run(&lock, record).map_err(|e| e.to_string())?;
    // Where the files went. A record naming a `source_root` of its own
    // relocates the file it names — D54's accepted residual — but the report
    // printed counts and no path and `--list` showed only `dest_root`, so
    // nothing on the system said where. It is the user's own folder, not a
    // record path, so printing it leaks nothing the record store protects.
    let into = (report.restored > 0).then_some(summary.source_root.as_path());
    print_report(&report, into)
}

fn no_such_run(id: &str) -> String {
    format!("no run named {id}; `comic-renamer undo --list` shows them")
}

/// A run id is a record's file name, matched against what `record::list`
/// enumerates. It is never joined onto the store: that would make any relative
/// path a run id, and a path is what the record store exists to stop accepting.
///
/// With no id, the newest record that no run holds, that can be read, and that
/// has a row to put back — the most recent *reversible* run, as D56 words it,
/// and the same gate the menu's offer uses. One unreadable file must not make
/// every other run unreversible, and nor must a run whose first row failed:
/// `reverse` skips its `failed` row in silence, so picking it deleted the stub
/// and reported success while the run the user meant stayed where it was.
///
/// A named id is not gated. The user chose that run, and reversing an empty
/// record is a no-op that tidies it away.
fn pick_record(records: &Path, run: Option<&str>) -> Result<Option<PathBuf>, String> {
    // A store that was never created holds no runs.
    if !records.exists() {
        return run.map_or(Ok(None), |id| Err(no_such_run(id)));
    }
    let found = record::list(records).map_err(|e| e.to_string())?;
    match run {
        Some(id) => found
            .into_iter()
            .find(|path| path.file_name().is_some_and(|n| n == id))
            .map(Some)
            .ok_or_else(|| no_such_run(id)),
        None => Ok(found.into_iter().find(|path| {
            !record::is_live(path).unwrap_or(true)
                && undo::summarise(path).is_ok_and(|s| s.reversible > 0)
        })),
    }
}

fn list_runs(records: &Path) -> Result<(), String> {
    let found = if records.exists() {
        record::list(records).map_err(|e| e.to_string())?
    } else {
        Vec::new()
    };
    let mut shown = 0;
    for path in found {
        let Ok(summary) = undo::summarise(&path) else {
            continue;
        };
        let Some(id) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let state = if record::is_live(&path).unwrap_or(true) {
            "running"
        } else if summary.interrupted {
            "interrupted"
        } else {
            "finished"
        };
        // Both roots. A run is a direction, and the bundle folder is the
        // half an undo puts files *back* into — the one a forged record can
        // choose, and the one nothing on the system named afterwards.
        println!(
            "{id}  {state}  {} of {} reversible  {} -> {}",
            summary.reversible,
            summary.planned,
            summary.source_root.display(),
            summary.dest_root.display()
        );
        shown += 1;
    }
    if shown == 0 {
        println!("no runs to reverse");
    }
    Ok(())
}

/// `into` is the folder the restored files went back to, when any did. The
/// counts alone could not say where, which is what made a relocation
/// untraceable.
fn print_report(report: &UndoReport, into: Option<&Path>) -> Result<(), String> {
    let UndoReport {
        restored,
        skipped,
        refused,
        aside,
    } = report;
    println!("{restored} restored · {skipped} skipped · {} refused", refused.len());
    if let Some(root) = into {
        println!("  put back into {}", root.display());
    }
    // D58: reversing a copy relocates the library file rather than deleting
    // it, so there is a directory with files in it that the user now owns.
    // Said on every reverse that moved one, because nothing else on the
    // system will: no sweep empties it, and a directory the tool filled and
    // never mentioned is a surprise the next time the library is measured.
    if let Some(Aside { directory, files }) = aside {
        println!("  {} moved aside into {}", plural(*files, "file"), directory.display());
        println!("  nothing deletes them for you");
    }
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
            not_ready: 0,
            collisions: Vec::new(),
            edited: false,
        }
    }

    #[test]
    fn a_malformed_overrides_file_is_an_error_not_a_silent_skip() {
        // CLAUDE.md: this means a bad data/overrides.toml was committed, which
        // is a build to reject rather than a run to degrade.
        let err = parse_overrides("[not valid").unwrap_err();
        assert!(err.contains("overrides"), "{err}");
        assert!(parse_overrides(OVERRIDES).is_ok());
    }

    #[test]
    fn the_overrides_ship_inside_the_binary() {
        // `Overrides::load(Path::new("data/overrides.toml"))` resolved against
        // the working directory. Same binary, same input, from the repo root
        // versus from /tmp: `The Walking Dead/The Walking Dead - Compendium
        // One.pdf` versus `failed`. An installed copy had no Layer B at all.
        let overrides = Overrides::parse(OVERRIDES).expect("built-in overrides must parse");
        assert!(
            overrides
                .lookup("TWD_CompendiumOne", "Image Comics Bundle 2")
                .is_some(),
            "the embedded overrides resolve nothing, so they are not reaching the resolver"
        );
    }

    /// `guesses_for` had no test at all, so the folder name could have stopped
    /// reaching the resolver entirely. Proven through Layer B, which keys on
    /// the folder: no corpus is needed, and none is fetched for a marketplace
    /// that is not Humble.
    #[test]
    fn guesses_for_hands_the_source_folder_name_to_the_resolver() {
        let overrides = Overrides::parse(
            "[\"TWD_CompendiumOne\"]\nseries = \"The Walking Dead\"\nbundle = \"Image Comics Bundle 2\"\n",
        )
        .unwrap();
        let files = [PathBuf::from("/bundle/TWD_CompendiumOne.pdf")];

        let found = guesses_for(&files, "fanatical", "Humble Image Comics Bundle 2", "en", &overrides);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "TWD_CompendiumOne");
        assert_eq!(found[0].1[0].series, "The Walking Dead", "the override fired");

        // The same file from a folder the override does not name: Layer B is
        // keyed on the folder, so a lost folder name loses the answer.
        let other = guesses_for(&files, "fanatical", "Some Other Bundle", "en", &overrides);
        assert_ne!(other[0].1.first().map(|s| s.series.as_str()), Some("The Walking Dead"));

        assert!(guesses_for(&[], "fanatical", "x", "en", &overrides).is_empty());
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
    fn rows_left_behind_are_disclosed_not_silently_dropped() {
        // §6.9 measures one to two rows in three needing a human. A user who
        // applies 260 of 412 rows and is told only about the 260 finds a third
        // of the bundle still in the source with no explanation.
        let mut f = facts(Mode::Move, false);
        f.files = 260;
        f.not_ready = 152;
        let out = disclose(&f);
        assert!(out.contains("left where they are: 152 of 412 rows"), "{out}");

        // And says nothing when there is nothing to say.
        assert!(!disclose(&facts(Mode::Move, false)).contains("not ready"));
    }

    #[test]
    fn a_forged_unmapped_count_cannot_panic_the_disclosure() {
        // The plan file is untrusted (§6.11). u64::MAX must neither panic in
        // debug nor wrap to a small believable number in release.
        let dest = tempfile::tempdir().unwrap();
        let preset = preset_by_id("komga").unwrap();
        // One row that is not runnable, so the subtraction term is non-zero
        // and a plain `+` would overflow.
        let row = PlanRow {
            id: "r1".into(),
            source: PathBuf::from("/src/a.pdf"),
            destination_preview: PathBuf::new(),
            extension: "pdf".into(),
            series: "S".into(),
            volume: None,
            issue: None,
            subtitle: None,
            run: 0,
            status: Status::Unmapped,
            mechanism: Mechanism::Rename,
            source_stat: comic_renamer_core::types::SourceStat { size: 0, mtime_ns: 0, inode: 1 },
            record: None,
        };
        let f = Facts::gather(Mode::Move, &[], &[row], u64::MAX, dest.path(), preset.as_ref(), false)
            .unwrap();
        assert_eq!(f.not_ready, usize::MAX);
        let mut some = facts(Mode::Move, false);
        some.not_ready = usize::MAX;
        let out = disclose(&some);
        assert!(out.contains("left where they are"), "{out}");
        assert!(!nothing_to_do(&f).is_empty());
    }

    /// scan → header was proven, and `Facts::gather` was proven with an
    /// injected count. header → file → disclosure was not: every plan in the
    /// suite set `unmapped: 0`, so replacing `header.unmapped` with a literal
    /// zero at the one place `apply` reads it broke nothing.
    #[test]
    fn a_plans_unmapped_count_reaches_the_disclosure_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("library");
        std::fs::create_dir(&dest).unwrap();
        let path = dir.path().join("plan.jsonl");
        let header = comic_renamer_core::types::PlanHeader {
            version: comic_renamer_core::plan::PLAN_VERSION,
            preset: "komga".into(),
            mode: Mode::Move,
            source_root: dir.path().join("bundle"),
            dest_root: dest.clone(),
            lang: "en".into(),
            required_bytes: 0,
            download_bytes: 0,
            unmapped: 7,
            checksum: String::new(),
        };
        comic_renamer_core::plan::write(&header, &[], &path).unwrap();

        // From the file, as `apply` reads it — a separate invocation from
        // `scan`, possibly against an old or foreign plan.
        let (read_back, rows) = comic_renamer_core::plan::read(&path).unwrap();
        assert_eq!(read_back.unmapped, 7);
        let preset = preset_by_id(&read_back.preset).unwrap();
        let mut f = Facts::gather(read_back.mode, &[], &rows, read_back.unmapped, &dest, preset.as_ref(), false)
            .unwrap();
        assert_eq!(f.not_ready, 7);
        assert!(nothing_to_do(&f).contains("Needs a human: 7 rows"));
        f.files = 2;
        assert!(disclose(&f).contains("left where they are: 7 of 9 rows"), "{}", disclose(&f));
    }

    #[test]
    fn files_scan_left_unmapped_reach_the_apply_disclosure() {
        // Unmapped files never become plan rows, so counting rows cannot see
        // them. The plan header carries the count, and `apply` is a separate
        // invocation from `scan`, possibly against an old or foreign plan.
        let dest = tempfile::tempdir().unwrap();
        let preset = preset_by_id("komga").unwrap();
        let f = Facts::gather(Mode::Move, &[], &[], 3, dest.path(), preset.as_ref(), false)
            .unwrap();
        assert_eq!(f.not_ready, 3);
        assert!(nothing_to_do(&f).contains("Needs a human: 3 rows"));

        let mut some = facts(Mode::Move, false);
        some.files = 9;
        some.not_ready = 3;
        assert!(disclose(&some).contains("left where they are: 3 of 12 rows"));
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

        // And the other side: 412 is not "412 file".
        let mut many = facts(Mode::Move, false);
        many.placeholders = 38;
        many.placeholder_bytes = 1 << 20;
        let out = disclose(&many);
        assert!(out.contains("412 files ·"), "{out}");
        assert!(out.contains("38 files are cloud placeholders"), "{out}");
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

    /// One record in `store`, newer the later the `secs`. `done` is a run that
    /// placed its row; otherwise the row is `failed`, which is what `apply`
    /// writes when the very first row is refused — a record with nothing in
    /// it to put back.
    fn a_run(store: &Path, secs: u64, done: bool) -> PathBuf {
        let source = store.join("bundle");
        std::fs::create_dir_all(&source).unwrap();
        let file = source.join("a.pdf");
        std::fs::write(&file, b"x").unwrap();
        let path = store.join(record::basename(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs),
        ));
        let mut writer =
            undo::UndoWriter::create(&path, &source, &store.join("library"), "komga", 1).unwrap();
        let seq = writer
            .record_pending(
                Path::new("a.pdf"), Path::new("A/A - Vol. 01.pdf"), Mechanism::Rename, Mode::Move,
                &file, "A", Some(1), None, 1, "pdf",
            )
            .unwrap();
        if done {
            writer.record_done(seq).unwrap();
        } else {
            writer.record_failed(seq).unwrap();
        }
        path
    }

    /// D56: `undo` reverses the most recent **reversible** run. The menu gates
    /// its offer on a record with rows to put back and this did not, so a run
    /// whose first row failed was picked ahead of the run the user meant:
    /// `reverse` skips a `failed` row, nothing is refused, so the stub is
    /// deleted and `undo` exits 0 reporting "0 restored" — while the record
    /// that could put their library back sits unreached until they run `undo`
    /// again.
    #[test]
    fn undo_picks_the_newest_reversible_run_not_the_newest_record() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("runs");
        std::fs::create_dir_all(&store).unwrap();
        let reversible = a_run(&store, 1_793_000_000, true);
        let empty = a_run(&store, 1_793_000_100, false);
        assert_eq!(
            record::list(&store).unwrap().first(),
            Some(&empty),
            "the empty record must be the newer one for this to mean anything"
        );

        assert_eq!(pick_record(&store, None).unwrap(), Some(reversible));

        // Named explicitly it is still reachable: the user chose that run, and
        // reversing it is a no-op that tidies the stub away.
        let id = empty.file_name().unwrap().to_str().unwrap().to_owned();
        assert_eq!(pick_record(&store, Some(&id)).unwrap(), Some(empty));
    }
}
