//! `cargo xtask measure` -- reports the corpus strategy's located rate and the
//! top suggestion's accuracy against the committed corpora. It reports and
//! exits 0: nothing is stored without approval (D50), so there is no
//! "silent" rate left to gate. Task 13 adds the hard checks.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use comic_renamer_core::corpus::{ensure_corpus, parse_manifest, Corpus};
use comic_renamer_core::overrides::Overrides;
use comic_renamer_core::resolve::{corpus_ladder, publisher_hint, suggest_with, Context};
use comic_renamer_core::types::PublisherHint;
use xtask::grade_ok;

const MANIFEST: &str = include_str!("../../corpus/manifest.json");
const OVERRIDES: &str = include_str!("../../data/overrides.toml");
/// 2,717 filenames held out of the corpus work. Gives the located rate.
const HELDOUT: &str = include_str!("../../fixtures/heldout-filenames.txt");
/// 2,223 verified filename-to-title rows (D26). Gives suggestion accuracy.
const GROUND_TRUTH: &str = include_str!("../../fixtures/humble-ground-truth.tsv");

/// 12 rows the maintainer confirmed title by title (2026-10-23).
const MARKETPLACE: &str = include_str!("../../fixtures/marketplace-ground-truth.tsv");

const LANG: &str = "en";

mod seed;

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("measure") => {
            if let Err(e) = measure() {
                eprintln!("error: {e}");
                std::process::exit(2);
            }
        }
        Some("seed") => {
            if let Err(e) = seed::seed() {
                eprintln!("error: {e}");
                std::process::exit(2);
            }
        }
        _ => {
            eprintln!("usage: cargo xtask <measure|seed>");
            std::process::exit(2);
        }
    }
}

fn measure() -> Result<(), String> {
    let manifest = parse_manifest(MANIFEST).map_err(|e| e.to_string())?;
    let database = ensure_corpus(&manifest, &cache_dir()?).map_err(|e| e.to_string())?;
    let corpus = Corpus::open(&database, &manifest).map_err(|e| e.to_string())?;
    let overrides = Overrides::parse(OVERRIDES).map_err(|e| e.to_string())?;

    // Located: a row counts when the corpus named a series at all, whether or
    // not it then refused or offered several. Only `None` means nothing matched.
    let unhinted = Context { overrides: Some(&overrides), ..Context::default() };
    let mut heldout = 0usize;
    let mut located = 0usize;
    for name in HELDOUT.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let stem = Path::new(name)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        heldout += 1;
        if corpus_ladder(&stem, &corpus, &unhinted).is_some() {
            located += 1;
        }
    }

    // Accuracy: does the top suggestion match the verified title? Rows are
    // bucketed by how many suggestions the picker would show, because one
    // right answer in a list of three is a different product from one right
    // answer offered alone.
    let mut hints: BTreeMap<&str, PublisherHint> = BTreeMap::new();
    let mut rows = 0usize;
    let mut top_right = 0usize;
    let mut series_right = 0usize;
    let mut buckets: BTreeMap<String, (usize, usize)> = BTreeMap::new();
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
            .or_insert_with(|| publisher_hint(bundle, &corpus))
            .clone();
        let ctx = Context {
            source_folder: bundle,
            hint,
            overrides: Some(&overrides),
            lang: LANG,
        };
        let found = suggest_with(stem, "Humble", Some(&corpus), &ctx);
        // Attributed to the strategy that produced the top suggestion. An empty
        // list is split by cause, because the causes point at different fixes.
        let bucket = match found.first() {
            None if corpus_ladder(stem, &corpus, &ctx).is_some() => {
                "none: corpus refused (language or volume)".to_string()
            }
            None => "none: empty filename guess".to_string(),
            Some(top) => format!(
                "{:?}: {}",
                top.source,
                if found.len() == 1 { "one suggestion" } else { "several suggestions" }
            ),
        };
        let entry = buckets.entry(bucket).or_default();
        entry.0 += 1;
        if let Some(top) = found.first() {
            let answer = match &top.subtitle {
                Some(sub) => format!("{}: {sub}", top.series),
                None => top.series.clone(),
            };
            if grade_ok(&top.series, title) {
                series_right += 1;
            }
            if grade_ok(&answer, title) {
                top_right += 1;
                entry.1 += 1;
            } else {
                wrong.push((stem, answer, title));
            }
        }
    }

    println!("graded rows by top suggestion's source, {rows} total");
    for (name, (n, right)) in &buckets {
        println!(
            "  {n:5}  {:5.1}%  {name}  (top is right: {right})",
            percent(*n, rows)
        );
    }

    println!(
        "\nlocated              {:>5.1}%   (held-out, {heldout} names)",
        percent(located, heldout)
    );
    println!(
        "series accuracy      {:>5.1}%   (top suggestion's series alone: a squashed subtitle is not a wrong series)",
        percent(series_right, rows)
    );
    println!(
        "suggestion accuracy  {:>5.1}%   (top suggestion vs verified title, {rows} rows)",
        percent(top_right, rows)
    );

    // Named, not just counted: a bare percentage cannot be audited. Capped so
    // the summary above stays on screen; `--all` prints every one.
    let shown = if std::env::args().any(|a| a == "--all") { wrong.len() } else { 20 };
    println!("\nwrong top suggestions (first {} of {}):", shown.min(wrong.len()), wrong.len());
    for (stem, answer, title) in wrong.iter().take(shown) {
        println!("  {stem}\n      said {answer:?}\n      want {title:?}");
    }
    Ok(())
}

fn percent(hit: usize, total: usize) -> f64 {
    if total == 0 {
        return 0.0;
    }
    hit as f64 * 100.0 / total as f64
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
