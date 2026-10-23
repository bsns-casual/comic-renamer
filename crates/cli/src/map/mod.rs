//! `map`: open a mapping session for the files the database does not know.

use crate::{guesses_for, parse_overrides, OVERRIDES, SHIPPED_MAPPINGS};
use comic_renamer_core::mapping::{key, parse_tsv, to_tsv, MappingRow, Mappings};
use comic_renamer_core::normalize::collision_key;
use comic_renamer_core::title::ParsedTitle;
use comic_renamer_core::overrides::Overrides;
use comic_renamer_core::preset::Preset;
use comic_renamer_core::types::{Mode, ScanOpts};
use comic_renamer_core::{plan, scan};
mod detail;
mod table;

use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

/// The user's own corrections live here. macOS may evict `~/Library/Caches`;
/// a corpus can be re-downloaded and a correction cannot. Deliberately not
/// `crate::cache_dir`.
pub fn config_dir() -> Result<PathBuf, String> {
    config_dir_from(std::env::var("XDG_CONFIG_HOME").ok(), std::env::var("HOME").ok())
}

/// Empty and relative values are ignored, as the XDG spec says: either would
/// put the user's corrections in whatever directory they ran from.
fn config_dir_from(xdg: Option<String>, home: Option<String>) -> Result<PathBuf, String> {
    let absolute = |v: String| Some(PathBuf::from(v)).filter(|p| p.is_absolute());
    if let Some(xdg) = xdg.and_then(absolute) {
        return Ok(xdg.join("comic-renamer"));
    }
    let home = home
        .and_then(absolute)
        .ok_or("neither XDG_CONFIG_HOME nor HOME is set to an absolute path")?;
    Ok(home.join("Library/Application Support/comic-renamer"))
}

/// Opens by saying how much of the bundle the database does not know.
pub fn orientation(unmapped: usize, total: usize) -> String {
    let lead = if unmapped * 2 > total {
        format!(
            "It looks like most of this bundle is not in our database yet: {unmapped} of {total} files could not be matched."
        )
    } else {
        format!("{unmapped} of {total} files are not in our database yet.")
    };
    format!(
        "{lead}\n\
         \n\
         Next is every one of them: its filename, our guess, and a spot for you to\n\
         approve or correct the final name. Nothing is moved until you review the\n\
         plan and run `apply`."
    )
}

/// A session reads keys and draws a screen, so it needs both streams. Refused
/// before anything is drawn: a pipe never receives escape codes and a CI job
/// never waits for input that cannot arrive.
fn require_terminal(stdout_is_terminal: bool, stdin_is_terminal: bool) -> Result<(), String> {
    let hint = "Run it directly in a terminal, or use `scan` to write a plan without a session.";
    if !stdout_is_terminal {
        return Err(format!("`map` needs an interactive terminal, but stdout is not one. {hint}"));
    }
    if !stdin_is_terminal {
        return Err(format!("`map` needs an interactive terminal, but stdin is not one. {hint}"));
    }
    Ok(())
}

/// How a session ended. `Quit` writes nothing: a plan the user walked away
/// from is not a plan they asked for.
pub enum Outcome {
    Done,
    Quit,
}

/// Pairs of stems whose approved rows render to one destination, NFC-folded
/// and case-folded as `find_collisions` compares them (invariant 7). Reported
/// at the gate the user confirms; the syscall still refuses either way.
pub fn intra_session_collisions(entries: &[table::Entry], preset: &dyn Preset) -> Vec<(String, String)> {
    let mut first: HashMap<String, &str> = HashMap::new();
    let mut found = Vec::new();
    for e in entries {
        let Some((fields, count)) = e.approved_row() else { continue };
        // An unkeyable stem is `commit_session`'s error to report.
        let Some(row) = row_for(&e.stem, fields, count, "", "") else { continue };
        let dest = collision_key(&preset.destination(&row.resolved(), &e.extension).to_string_lossy());
        match first.get(&dest) {
            Some(earlier) => found.push((earlier.to_string(), e.stem.clone())),
            None => {
                first.insert(dest, &e.stem);
            }
        }
    }
    found
}

fn row_for(stem: &str, fields: &ParsedTitle, count: Option<u32>, bundle: &str, lang: &str) -> Option<MappingRow> {
    Some(MappingRow {
        key: key(stem)?,
        series: fields.series.clone(),
        volume: fields.volume,
        subtitle: fields.subtitle.clone(),
        lang: lang.to_string(),
        volume_count: count,
        bundle: bundle.to_string(),
    })
}

/// What `commit_session` did: rows written, and pairs of files that wanted one
/// key with different titles. Neither of a pair is written.
#[derive(Debug)]
pub struct Committed {
    pub written: usize,
    pub clashes: Vec<(String, String)>,
}

/// Writes the approved entries into the user's local mapping file. Every new
/// row is validated first and any failure leaves the file as it was; the file
/// then lands via a sibling temp file and a rename, so an interrupted write
/// cannot truncate corrections no other record holds.
/// `bundle` and `lang` are not on an `Entry`, so the caller supplies them.
///
/// An existing file that does not load aborts: overwriting it would drop the
/// user's older corrections. A row with the same `(key, bundle)` is replaced.
/// Files whose names share a key but whose titles differ are reported and
/// left out, so the rest of the session is kept.
// shortcut: no lock around the read-modify-write, so two sessions inside the
// read-to-rename window lose one set of rows; an advisory lock beside the file
// (as apply's lockfile does) closes it.
pub fn commit_session(entries: &[table::Entry], local: &Path, bundle: &str, lang: &str) -> Result<Committed, String> {
    // Grouped by key, in first-seen order.
    let mut groups: Vec<Vec<(&str, MappingRow)>> = Vec::new();
    for e in entries {
        let Some((fields, count)) = e.approved_row() else { continue };
        let row = row_for(&e.stem, fields, count, bundle, lang)
            .ok_or_else(|| format!("{:?} has no letters or digits to key a mapping on", e.stem))?;
        row.validate().map_err(|err| format!("{:?}: {err}", e.stem))?;
        match groups.iter_mut().find(|g| g[0].1.key == row.key && g[0].1.bundle == row.bundle) {
            Some(g) => g.push((e.stem.as_str(), row)),
            None => groups.push(vec![(e.stem.as_str(), row)]),
        }
    }
    // `hellboy-1.pdf` and `hellboy1.pdf` share a key. Identical rows lose
    // nothing; differing ones would have one silently win.
    let mut fresh = Vec::new();
    let mut clashes = Vec::new();
    for g in groups {
        if g.iter().all(|(_, r)| *r == g[0].1) {
            fresh.push(g.into_iter().next().unwrap().1);
        } else {
            clashes.extend(g[1..].iter().map(|(s, _)| (g[0].0.to_string(), s.to_string())));
        }
    }
    if fresh.is_empty() {
        return Ok(Committed { written: 0, clashes });
    }
    let mut rows = match std::fs::read_to_string(local) {
        Ok(text) => parse_tsv(&text).map_err(|e| format!("{}: {e}", local.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(format!("{}: {e}", local.display())),
    };
    let written = fresh.len();
    for row in fresh {
        match rows.iter_mut().find(|r| r.key == row.key && r.bundle == row.bundle) {
            Some(slot) => *slot = row,
            None => rows.push(row),
        }
    }
    let dir = local.parent().ok_or("the mapping file has no parent directory")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = local.with_extension("tsv.tmp");
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(to_tsv(&rows).as_bytes())?;
        f.sync_all()?;
        // Replacing the old file is the point here; the no-clobber rule is for comics.
        std::fs::rename(&tmp, local)
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", local.display())
    })?;
    Ok(Committed { written, clashes })
}

fn clash_warning(a: &str, b: &str) -> String {
    format!(
        "warning: {a} and {b} give the same mapping key but you titled them differently, so neither was saved. Rename one of the files so the names differ in more than punctuation, case or spacing, then run `map` again."
    )
}

/// `apply` removes every row of a collision from the run, so the user must hear
/// that neither file is filed, not that one of them is.
fn collision_warning(a: &str, b: &str) -> String {
    format!("warning: {a} and {b} would land on the same path, so neither is placed; both files stay where they are.")
}

/// `undo` derives the manifest from the plan's path (D38), so the default is
/// the working directory, as `scan` has it, and never a temp directory.
fn plan_path(out: Option<PathBuf>) -> PathBuf {
    out.unwrap_or_else(|| PathBuf::from("comic-renamer-plan.jsonl"))
}

fn scan_once(opts: &ScanOpts, config: &Path) -> Result<scan::ScanOutcome, String> {
    let local = config.join("mappings.tsv");
    let local = local.exists().then_some(local);
    let mappings = Mappings::load(SHIPPED_MAPPINGS, local.as_deref()).map_err(|e| e.to_string())?;
    scan::scan(opts, &mappings, &mut |_| {}).map_err(|e| e.to_string())
}

pub fn run(
    source: PathBuf,
    dest: PathBuf,
    out: Option<PathBuf>,
    preset: String,
    lang: String,
    marketplace: String,
    mode: Mode,
) -> Result<(), String> {
    let folder = source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let overrides = parse_overrides(OVERRIDES)?;
    let preset_impl = comic_renamer_core::preset::preset_by_id(&preset).ok_or_else(|| format!("unknown preset {preset:?}"))?;
    let opts = ScanOpts { source_root: source, dest_root: dest, preset, mode: Some(mode), lang: lang.clone() };
    let terminals = (std::io::stdout().is_terminal(), std::io::stdin().is_terminal());
    let config = config_dir()?;
    let local = config.join("mappings.tsv");
    run_with(
        &opts,
        plan_path(out),
        &config,
        terminals,
        |first| session(&first.unmapped, &marketplace, &folder, &lang, &overrides, preset_impl.as_ref(), &local),
        |plan| crate::run_apply(plan, false, false, None),
    )
}

fn run_with(
    opts: &ScanOpts,
    destination: PathBuf,
    config: &Path,
    (stdout_tty, stdin_tty): (bool, bool),
    session: impl FnOnce(&scan::ScanOutcome) -> Result<Outcome, String>,
    apply: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let first = scan_once(opts, config)?;
    let finished = if first.unmapped.is_empty() {
        println!("Every file is already mapped; nothing needed mapping.");
        first
    } else {
        require_terminal(stdout_tty, stdin_tty)?;
        let total = first.rows.len() + first.unmapped.len();
        println!("{}\n", orientation(first.unmapped.len(), total));
        if let Outcome::Quit = session(&first)? {
            // A quit exits non-zero: an exit code that is zero either way cannot
            // tell a caller "a plan exists" from "none does".
            return Err("Stopped. No plan was written.".into());
        }
        // The session records what the user approved in the local mapping
        // file; the plan is built from what that leaves.
        scan_once(opts, config)?
    };
    plan::write(&finished.header, &finished.rows, &destination).map_err(|e| e.to_string())?;
    println!("plan written to {}", destination.display());
    // `apply` shows the disclosure and asks for its own confirmation; declining
    // there leaves this plan in place for a later `apply`. Without a terminal
    // nobody can type the word, so the plan is left for `apply --yes`.
    if stdout_tty && stdin_tty {
        return apply(&destination);
    }
    println!("Nothing has been moved. Review it, then:");
    println!("  comic-renamer apply {}", destination.display());
    Ok(())
}

fn session(
    unmapped: &[PathBuf],
    marketplace: &str,
    folder: &str,
    lang: &str,
    overrides: &Overrides,
    preset: &dyn Preset,
    local: &Path,
) -> Result<Outcome, String> {
    let entries = guesses_for(unmapped, marketplace, folder, lang, overrides)
        .into_iter()
        .zip(unmapped)
        .map(|((stem, found), path)| {
            // Lowercased as `scan` does, so the editor shows the extension `apply` will use.
            let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            table::Entry::new(stem, found).with_extension(&ext)
        })
        .collect();
    let mut t = table::Table::new(folder.to_string(), entries);
    match table::run(&mut t, preset).map_err(|e| e.to_string())? {
        table::Action::Done => {
            for (a, b) in intra_session_collisions(&t.entries, preset) {
                println!("{}", collision_warning(&a, &b));
            }
            let done = commit_session(&t.entries, local, folder, lang)?;
            for (a, b) in &done.clashes {
                println!("{}", clash_warning(a, b));
            }
            let n = done.written;
            println!("{n} {} saved to {}", if n == 1 { "mapping" } else { "mappings" }, local.display());
            Ok(Outcome::Done)
        }
        _ => Ok(Outcome::Quit),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_is_application_support_never_caches() {
        let dir = config_dir_from(None, Some("/Users/x".into())).unwrap();
        assert_eq!(dir, PathBuf::from("/Users/x/Library/Application Support/comic-renamer"));
        assert!(!dir.to_string_lossy().contains("Caches"));
    }

    #[test]
    fn config_dir_honours_xdg_config_home() {
        let dir = config_dir_from(Some("/cfg".into()), Some("/Users/x".into())).unwrap();
        assert_eq!(dir, PathBuf::from("/cfg/comic-renamer"));
    }

    #[test]
    fn config_dir_without_home_or_xdg_is_an_error() {
        assert!(config_dir_from(None, None).is_err());
    }

    #[test]
    fn config_dir_ignores_an_empty_or_relative_xdg_config_home() {
        for bad in ["", "cfg", "./cfg"] {
            let dir = config_dir_from(Some(bad.into()), Some("/Users/x".into())).unwrap();
            assert_eq!(
                dir,
                PathBuf::from("/Users/x/Library/Application Support/comic-renamer"),
                "XDG_CONFIG_HOME={bad:?}"
            );
        }
    }

    #[test]
    fn config_dir_never_resolves_against_the_working_directory() {
        for home in ["", "relative"] {
            assert!(config_dir_from(None, Some(home.into())).is_err(), "HOME={home:?}");
        }
        assert!(config_dir_from(Some("".into()), Some("".into())).is_err());
    }

    #[test]
    fn orientation_names_the_count() {
        let s = orientation(38, 40);
        assert!(s.contains("38 of 40"), "{s}");
        assert!(s.contains("our guess"), "{s}");
    }

    #[test]
    fn orientation_says_most_of_the_bundle_is_unknown_only_when_it_is() {
        let most = orientation(38, 40);
        assert!(most.lines().next().unwrap().contains("most of this bundle"), "{most}");
        let few = orientation(2, 40);
        assert!(!few.contains("most of this bundle"), "{few}");
        assert!(few.lines().next().unwrap().contains("2 of 40"), "{few}");
    }

    #[test]
    fn orientation_is_plain_text() {
        assert!(!orientation(1, 1).contains('\x1b'));
    }

    #[test]
    fn a_session_needs_both_streams_to_be_terminals() {
        assert!(require_terminal(true, true).is_ok());
        let out = require_terminal(false, true).unwrap_err();
        assert!(out.contains("stdout") && out.contains("terminal"), "{out}");
        let inp = require_terminal(true, false).unwrap_err();
        assert!(inp.contains("stdin") && inp.contains("terminal"), "{inp}");
    }

    /// A bundle of one file the shipped mappings do not know.
    fn unmapped_bundle() -> (tempfile::TempDir, ScanOpts, PathBuf, PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (source, dest) = (root.join("bundle"), root.join("library"));
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::File::create(source.join("zzqx_nothing_vol1.pdf")).unwrap();
        let opts = ScanOpts {
            source_root: source,
            dest_root: dest,
            preset: "komga".into(),
            mode: Some(Mode::Move),
            lang: "en".into(),
        };
        let plan = root.join("plan.jsonl");
        let config = root.join("config");
        (dir, opts, plan, config)
    }

    #[test]
    fn a_session_the_user_quit_writes_no_plan() {
        let (_dir, opts, plan, config) = unmapped_bundle();
        let r = run_with(&opts, plan.clone(), &config, (true, true), |_| Ok(Outcome::Quit), |_| Ok(()));
        assert!(r.unwrap_err().contains("No plan was written"), "a quit must be an error, so the exit code is non-zero");
        assert!(!plan.exists());
    }

    #[test]
    fn a_finished_session_writes_the_plan() {
        let (_dir, opts, plan, config) = unmapped_bundle();
        run_with(&opts, plan.clone(), &config, (true, true), |_| Ok(Outcome::Done), |_| Ok(())).unwrap();
        assert!(plan.exists());
    }

    #[test]
    fn no_session_is_opened_without_a_terminal() {
        let (_dir, opts, plan, config) = unmapped_bundle();
        let r = run_with(&opts, plan.clone(), &config, (false, true), |_| panic!("opened a session"), |_| Ok(()));
        assert!(r.is_err());
        assert!(!plan.exists());
    }

    #[test]
    fn the_default_plan_path_is_the_working_directory_never_a_temp_dir() {
        assert_eq!(plan_path(None), PathBuf::from("comic-renamer-plan.jsonl"));
        assert_eq!(plan_path(Some("x/p.jsonl".into())), PathBuf::from("x/p.jsonl"));
    }

    // --- Task 10: commit_session, collisions, delegation ---

    use comic_renamer_core::preset::komga::Komga;
    use comic_renamer_core::title::ParsedTitle;
    use comic_renamer_core::types::{Suggestion, SuggestionSource};

    fn sug(series: &str, volume: Option<u32>, subtitle: Option<&str>, count: Option<u32>) -> Suggestion {
        Suggestion {
            series: series.into(),
            volume,
            subtitle: subtitle.map(String::from),
            volume_count: count,
            source: SuggestionSource::Corpus,
        }
    }

    fn approved(stem: &str, series: &str, volume: Option<u32>, count: Option<u32>) -> table::Entry {
        let mut e = table::Entry::new(stem.into(), vec![sug(series, volume, None, count)]).with_extension("cbz");
        assert!(e.accept_edit(ParsedTitle { series: series.into(), volume, subtitle: None }));
        e
    }

    fn unapproved(stem: &str, series: &str) -> table::Entry {
        table::Entry::new(stem.into(), vec![sug(series, Some(1), None, None)]).with_extension("cbz")
    }

    fn read_rows(path: &Path) -> Vec<comic_renamer_core::mapping::MappingRow> {
        comic_renamer_core::mapping::parse_tsv(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn two_approved_rows_rendering_to_one_destination_are_reported_before_apply() {
        // The brief's example (two subtitles) renders to two paths under Komga,
        // which puts the subtitle in the filename. Two rows with the same series
        // and no volume or subtitle are how two rows really share one path.
        let entries = vec![approved("one", "Usagi Yojimbo", None, None), approved("two", "usagi yojimbo", None, None)];
        let found = intra_session_collisions(&entries, &Komga);
        assert_eq!(found, vec![("one".to_string(), "two".to_string())]);
    }

    #[test]
    fn distinct_destinations_and_unapproved_rows_are_not_collisions() {
        let mut pending = unapproved("c", "Usagi Yojimbo");
        pending.final_fields = Some(ParsedTitle { series: "Usagi Yojimbo".into(), volume: Some(1), subtitle: None });
        let entries = vec![approved("a", "Usagi Yojimbo", Some(1), None), approved("b", "Usagi Yojimbo", Some(2), None), pending];
        assert!(intra_session_collisions(&entries, &Komga).is_empty(), "an unapproved row is never written, so never collides");
    }

    #[test]
    fn only_approved_rows_reach_the_local_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("config/mappings.tsv");
        let entries = vec![approved("alpha_vol1", "Alpha", Some(1), None), unapproved("beta_vol1", "Beta")];
        assert_eq!(commit_session(&entries, &local, "Bundle", "en").unwrap().written, 1);
        let text = std::fs::read_to_string(&local).unwrap();
        assert!(text.contains("alphavol1\tAlpha"), "{text}");
        assert!(!text.contains("Beta") && !text.contains("betavol1"), "{text}");
    }

    #[test]
    fn nothing_approved_writes_no_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("mappings.tsv");
        assert_eq!(commit_session(&[unapproved("x", "X")], &local, "B", "en").unwrap().written, 0);
        assert!(!local.exists());
    }

    #[test]
    fn volume_count_survives_into_the_written_row() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("mappings.tsv");
        commit_session(&[approved("lw_vol1", "Lone Wolf", Some(1), Some(128))], &local, "B", "en").unwrap();
        let rows = read_rows(&local);
        assert_eq!(rows[0].volume_count, Some(128));
        // ...and so pads to three digits, not the two-digit floor.
        let dest = Komga.destination(&rows[0].resolved(), "cbz");
        assert!(dest.to_string_lossy().contains("Vol. 001"), "{dest:?}");
    }

    #[test]
    fn an_edited_series_does_not_inherit_a_count_it_has_no_reason_to_have() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("mappings.tsv");
        let mut e = table::Entry::new("x".into(), vec![sug("Lone Wolf", Some(1), None, Some(128))]).with_extension("cbz");
        assert!(e.accept_edit(ParsedTitle { series: "Something Else".into(), volume: Some(1), subtitle: None }));
        commit_session(&[e], &local, "B", "en").unwrap();
        assert_eq!(read_rows(&local)[0].volume_count, None);
    }

    #[test]
    fn the_local_write_is_validated_before_it_lands() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("mappings.tsv");
        // A file holding real accumulated corrections, not just a header.
        commit_session(
            &[approved("old_vol1", "Old", Some(1), Some(30)), approved("older_vol2", "Older", Some(2), None)],
            &local,
            "B",
            "en",
        )
        .unwrap();
        let before = std::fs::read(&local).unwrap();
        assert!(String::from_utf8_lossy(&before).contains("Older"));
        // "=SUM" starts with a spreadsheet formula character, which validate() rejects.
        let entries = vec![approved("good_vol1", "Good", Some(1), None), approved("bad_vol1", "=SUM(A1)", Some(1), None)];
        let err = commit_session(&entries, &local, "B", "en").unwrap_err();
        assert!(err.contains("formula"), "{err}");
        assert_eq!(std::fs::read(&local).unwrap(), before, "the file must be byte-identical");
        assert!(!dir.path().join("mappings.tsv.tmp").exists());
    }

    #[test]
    fn two_files_that_want_one_key_are_reported_and_the_rest_still_saved() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("mappings.tsv");
        // The first two normalise to the key `hellboy1` but were typed differently.
        let entries = vec![
            approved("hellboy-1", "Hellboy", Some(1), None),
            approved("good_vol1", "Good", Some(1), None),
            approved("hellboy1", "Hellboy Omnibus", Some(1), None),
        ];
        let done = commit_session(&entries, &local, "B", "en").unwrap();
        assert_eq!(done.written, 1);
        assert_eq!(done.clashes, vec![("hellboy-1".to_string(), "hellboy1".to_string())]);
        let text = std::fs::read_to_string(&local).unwrap();
        assert!(text.contains("Good") && !text.contains("Hellboy"), "neither clashing row is guessed at: {text}");
        let msg = clash_warning("hellboy-1", "hellboy1");
        assert!(msg.contains("hellboy-1") && msg.contains("hellboy1") && msg.contains("Rename one"), "{msg}");
    }

    #[test]
    fn a_collision_warning_says_neither_row_is_placed() {
        let msg = collision_warning("one", "two");
        assert!(msg.contains("neither") && msg.contains("stay"), "{msg}");
        assert!(!msg.contains("skipped"), "{msg}");
    }

    #[test]
    fn a_case_only_edit_keeps_the_count() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("mappings.tsv");
        let mut e = table::Entry::new("x".into(), vec![sug("Black is the Color", Some(1), None, Some(128))]).with_extension("cbz");
        assert!(e.accept_edit(ParsedTitle { series: "Black Is the Color".into(), volume: Some(1), subtitle: None }));
        commit_session(&[e], &local, "B", "en").unwrap();
        assert_eq!(read_rows(&local)[0].volume_count, Some(128));
    }

    #[test]
    fn two_files_with_one_key_and_identical_rows_lose_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("mappings.tsv");
        let entries = vec![approved("hellboy-1", "Hellboy", Some(1), None), approved("hellboy1", "Hellboy", Some(1), None)];
        assert_eq!(commit_session(&entries, &local, "B", "en").unwrap().written, 1);
    }

    #[test]
    fn the_plan_is_written_where_undo_can_find_it_and_apply_is_handed_it() {
        let (_dir, opts, plan, config) = unmapped_bundle();
        let mut applied = None;
        run_with(&opts, plan.clone(), &config, (true, true), |_| Ok(Outcome::Done), |p| {
            applied = Some(p.to_path_buf());
            Ok(())
        })
        .unwrap();
        assert!(plan.exists());
        assert_eq!(applied.as_deref(), Some(plan.as_path()));
        // The default is the working directory (see the plan_path test), never a temp dir.
        assert_eq!(plan_path(None), PathBuf::from("comic-renamer-plan.jsonl"));
    }
}
