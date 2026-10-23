//! `map`: open a mapping session for the files the database does not know.

use crate::{guesses_for, parse_overrides, OVERRIDES, SHIPPED_MAPPINGS};
use comic_renamer_core::mapping::Mappings;
use comic_renamer_core::overrides::Overrides;
use comic_renamer_core::types::{Mode, ScanOpts};
use comic_renamer_core::{plan, scan};
mod table;

use std::io::IsTerminal;
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
    let opts = ScanOpts { source_root: source, dest_root: dest, preset, mode: Some(mode), lang: lang.clone() };
    let terminals = (std::io::stdout().is_terminal(), std::io::stdin().is_terminal());
    run_with(&opts, plan_path(out), &config_dir()?, terminals, |first| {
        session(&first.unmapped, &marketplace, &folder, &lang, &overrides)
    })
}

fn run_with(
    opts: &ScanOpts,
    destination: PathBuf,
    config: &Path,
    (stdout_tty, stdin_tty): (bool, bool),
    session: impl FnOnce(&scan::ScanOutcome) -> Result<Outcome, String>,
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
    println!("Nothing has been moved. Review it, then:");
    println!("  comic-renamer apply {}", destination.display());
    Ok(())
}

// shortcut: shows the table and records nothing; the detail editor and the
// write of approved rows to `config_dir()/mappings.tsv` are later tasks, so
// `Done` currently builds the plan from what is already mapped.
fn session(
    unmapped: &[PathBuf],
    marketplace: &str,
    folder: &str,
    lang: &str,
    overrides: &Overrides,
) -> Result<Outcome, String> {
    let entries = guesses_for(unmapped, marketplace, folder, lang, overrides)
        .into_iter()
        .map(|(stem, mut found)| table::Entry {
            stem,
            // Several candidates are an ambiguity, not a guess to approve.
            guess: if found.len() == 1 { found.pop() } else { None },
            final_fields: None,
            approved: false,
        })
        .collect();
    let mut t = table::Table::new(folder.to_string(), entries);
    match table::run(&mut t).map_err(|e| e.to_string())? {
        table::Action::Done => Ok(Outcome::Done),
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
        let r = run_with(&opts, plan.clone(), &config, (true, true), |_| Ok(Outcome::Quit));
        assert!(r.unwrap_err().contains("No plan was written"), "a quit must be an error, so the exit code is non-zero");
        assert!(!plan.exists());
    }

    #[test]
    fn a_finished_session_writes_the_plan() {
        let (_dir, opts, plan, config) = unmapped_bundle();
        run_with(&opts, plan.clone(), &config, (true, true), |_| Ok(Outcome::Done)).unwrap();
        assert!(plan.exists());
    }

    #[test]
    fn no_session_is_opened_without_a_terminal() {
        let (_dir, opts, plan, config) = unmapped_bundle();
        let r = run_with(&opts, plan.clone(), &config, (false, true), |_| panic!("opened a session"));
        assert!(r.is_err());
        assert!(!plan.exists());
    }

    #[test]
    fn the_default_plan_path_is_the_working_directory_never_a_temp_dir() {
        assert_eq!(plan_path(None), PathBuf::from("comic-renamer-plan.jsonl"));
        assert_eq!(plan_path(Some("x/p.jsonl".into())), PathBuf::from("x/p.jsonl"));
    }
}
