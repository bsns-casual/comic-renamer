//! `cargo xtask measure` — the §7 gate. Reports the located rate, the ready
//! rate and the silent false positive rate against the committed corpora, and
//! exits non-zero when any threshold is breached.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use comic_renamer_core::corpus::{ensure_corpus, parse_manifest, Corpus};
use comic_renamer_core::overrides::Overrides;
use comic_renamer_core::resolve::{publisher_hint, resolve};
use comic_renamer_core::types::{PublisherHint, ReviewReason, Status};
use xtask::{gates_pass, grade_ok, Rates};

const MANIFEST: &str = include_str!("../../corpus/manifest.json");
const OVERRIDES: &str = include_str!("../../data/overrides.toml");
/// 2,717 filenames held out of the corpus work. Gives the located rate.
const HELDOUT: &str = include_str!("../../fixtures/heldout-filenames.txt");
/// 2,223 verified filename-to-title rows. Gives ready and silent FP (D26).
const GROUND_TRUTH: &str = include_str!("../../fixtures/humble-ground-truth.tsv");

const LANG: &str = "en";

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("measure") => match measure() {
            Ok(rates) => {
                report(rates);
                if !gates_pass(rates) {
                    std::process::exit(1);
                }
            }
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(2);
            }
        },
        _ => {
            eprintln!("usage: cargo xtask measure");
            std::process::exit(2);
        }
    }
}

fn measure() -> Result<Rates, String> {
    let manifest = parse_manifest(MANIFEST).map_err(|e| e.to_string())?;
    let database = ensure_corpus(&manifest, &cache_dir()?).map_err(|e| e.to_string())?;
    let corpus = Corpus::open(&database, &manifest).map_err(|e| e.to_string())?;
    let overrides = Overrides::parse(OVERRIDES).map_err(|e| e.to_string())?;

    // Located: a row counts when the ladder named a series at all, ready or
    // review. Only `Failed` means nothing was found.
    let unhinted = PublisherHint(None);
    let mut heldout = 0usize;
    let mut located = 0usize;
    for name in HELDOUT.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let stem = Path::new(name)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        heldout += 1;
        if resolve(&stem, "", &unhinted, &corpus, &overrides, LANG).status != Status::Failed {
            located += 1;
        }
    }

    // Ready and silent FP, both over every graded row. A silent false positive
    // is the one that matters: `ready` means the tool would move the file
    // without asking, and the answer is wrong.
    let mut hints: HashMap<&str, PublisherHint> = HashMap::new();
    let mut rows = 0usize;
    let mut ready = 0usize;
    // A bare ready rate cannot be acted on: it does not say whether the
    // missing rows failed to match or matched ambiguously, and those point at
    // different fixes.
    let mut why: HashMap<&'static str, usize> = HashMap::new();
    let mut wrong: Vec<(&str, String, &str)> = Vec::new();
    for line in GROUND_TRUTH.lines().skip(1) {
        let mut fields = line.split('\t');
        let (Some(bundle), Some(title), Some(stem)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        rows += 1;
        let hint = hints
            .entry(bundle)
            .or_insert_with(|| publisher_hint(bundle, &corpus));
        let resolved = resolve(stem, bundle, hint, &corpus, &overrides, LANG);
        *why.entry(label(&resolved.status)).or_default() += 1;
        if matches!(resolved.status, Status::Ready(_)) {
            ready += 1;
            let answer = match &resolved.subtitle {
                Some(sub) => format!("{}: {sub}", resolved.series),
                None => resolved.series.clone(),
            };
            if !grade_ok(&answer, title) {
                wrong.push((stem, answer, title));
            }
        }
    }

    // Every silent false positive, named. A bare percentage cannot be audited,
    // and the published 0.48% was reached by hand-sampling failures.
    for (stem, answer, title) in &wrong {
        println!("silent FP  {stem}\n    said {answer:?}\n    want {title:?}");
    }

    let mut breakdown: Vec<(&str, usize)> = why.into_iter().collect();
    breakdown.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    println!("\ngraded rows by status, {rows} total");
    for (name, n) in breakdown {
        println!("  {n:5}  {:5.1}%  {name}", percent(n, rows));
    }

    Ok(Rates {
        located: percent(located, heldout),
        ready: percent(ready, rows),
        silent_false_positive: percent(wrong.len(), rows),
    })
}

fn label(status: &Status) -> &'static str {
    match status {
        Status::Ready(_) | Status::Mapped => "ready",
        Status::Unmapped => "unmapped",
        Status::Failed => "failed - no series matched",
        Status::Review(ReviewReason::Ambiguous(_)) => "review - ambiguous",
        Status::Review(ReviewReason::UnsplitSubtitle(_)) => "review - unsplit subtitle",
        Status::Review(ReviewReason::NoLanguageMatch) => "review - no language match",
        Status::Review(ReviewReason::VolumeInfeasible) => "review - volume infeasible",
    }
}

fn percent(hit: usize, total: usize) -> f64 {
    if total == 0 {
        return 0.0;
    }
    hit as f64 * 100.0 / total as f64
}

fn report(rates: Rates) {
    let gate = |ok: bool| if ok { "pass" } else { "FAIL" };
    println!(
        "\nlocated  {:>5.1}%   gate >= 92.3%   {}",
        rates.located,
        gate(rates.located >= 92.3)
    );
    println!(
        "ready    {:>5.1}%   gate >= 66.0%   {}",
        rates.ready,
        gate(rates.ready >= 66.0)
    );
    println!(
        "silent FP {:>5.2}%  gate <= 0.48%   {}",
        rates.silent_false_positive,
        gate(rates.silent_false_positive <= 0.48)
    );
}

/// Mirrors the CLI's own cache location so the two share one download.
// shortcut: duplicated from crates/cli/src/main.rs — a bin crate cannot be
// imported. Belongs in `corpus` if a third caller ever appears.
fn cache_dir() -> Result<PathBuf, String> {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(xdg).join("comic-renamer"));
    }
    let home = std::env::var("HOME").map_err(|_| "neither XDG_CACHE_HOME nor HOME is set")?;
    Ok(PathBuf::from(home)
        .join("Library/Caches")
        .join("comic-renamer"))
}
