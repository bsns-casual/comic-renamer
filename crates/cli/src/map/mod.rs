//! `map`: open a mapping session for the files the database does not know.

use crate::{parse_overrides, suggest_for, OVERRIDES, SHIPPED_MAPPINGS};
use comic_renamer_core::mapping::Mappings;
use comic_renamer_core::overrides::Overrides;
use comic_renamer_core::types::{Mode, ScanOpts};
use comic_renamer_core::{plan, scan};
use std::io::IsTerminal;
use std::path::PathBuf;

/// The user's own corrections live here. macOS may evict `~/Library/Caches`;
/// a corpus can be re-downloaded and a correction cannot. Deliberately not
/// `crate::cache_dir`.
pub fn config_dir() -> Result<PathBuf, String> {
    config_dir_from(std::env::var("XDG_CONFIG_HOME").ok(), std::env::var("HOME").ok())
}

fn config_dir_from(xdg: Option<String>, home: Option<String>) -> Result<PathBuf, String> {
    if let Some(xdg) = xdg {
        return Ok(PathBuf::from(xdg).join("comic-renamer"));
    }
    let home = home.ok_or("neither XDG_CONFIG_HOME nor HOME is set")?;
    Ok(PathBuf::from(home).join("Library/Application Support/comic-renamer"))
}

/// Opens with the fact that matters: the database does not know this bundle.
pub fn orientation(unmapped: usize, total: usize) -> String {
    format!(
        "This bundle is not in our database: {unmapped} of {total} files could not be matched.\n\
         \n\
         Next is every one of them: its filename, our guess, and a spot for you to\n\
         approve or correct the final name. Nothing is moved until you review the\n\
         plan and run `apply`."
    )
}

/// A session needs a keyboard and a screen. Refused before anything is drawn,
/// so a pipe never receives escape codes and a CI job never waits for input.
fn require_terminal(stdout_is_terminal: bool) -> Result<(), String> {
    if stdout_is_terminal {
        Ok(())
    } else {
        Err("`map` needs an interactive terminal, but stdout is not one. \
             Run it directly in a terminal, or use `scan` to write a plan without a session."
            .into())
    }
}

/// `undo` derives the manifest from the plan's path (D38), so the default is
/// the working directory, as `scan` has it, and never a temp directory.
fn plan_path(out: Option<PathBuf>) -> PathBuf {
    out.unwrap_or_else(|| PathBuf::from("comic-renamer-plan.jsonl"))
}

fn scan_once(opts: &ScanOpts) -> Result<scan::ScanOutcome, String> {
    let local = config_dir()?.join("mappings.tsv");
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
    let destination = plan_path(out);

    let first = scan_once(&opts)?;
    let finished = if first.unmapped.is_empty() {
        println!("Every file is already mapped; nothing needed mapping.");
        first
    } else {
        require_terminal(std::io::stdout().is_terminal())?;
        let total = first.rows.len() + first.unmapped.len();
        println!("{}\n", orientation(first.unmapped.len(), total));
        session(&first.unmapped, &marketplace, &folder, &lang, &overrides);
        // The session records what the user approved in the local mapping
        // file; the plan is built from what that leaves.
        scan_once(&opts)?
    };
    plan::write(&finished.header, &finished.rows, &destination).map_err(|e| e.to_string())?;
    println!("plan written to {}", destination.display());
    println!("Nothing has been moved. Review it, then:");
    println!("  comic-renamer apply {}", destination.display());
    Ok(())
}

// shortcut: prints guesses and records nothing; the next task replaces this
// body with the interactive table and detail editor, which write approved rows
// to `config_dir()/mappings.tsv`.
fn session(unmapped: &[PathBuf], marketplace: &str, folder: &str, lang: &str, overrides: &Overrides) {
    suggest_for(unmapped, marketplace, folder, lang, overrides);
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
    fn orientation_leads_with_the_bundle_being_unknown_and_names_the_count() {
        let s = orientation(38, 40);
        assert!(s.contains("38 of 40"), "{s}");
        assert!(s.lines().next().unwrap().contains("not in our database"), "{s}");
        assert!(s.contains("our guess"), "{s}");
    }

    #[test]
    fn orientation_is_plain_text() {
        assert!(!orientation(1, 1).contains('\x1b'));
    }

    #[test]
    fn a_non_tty_stdout_is_refused_with_a_message() {
        let e = require_terminal(false).unwrap_err();
        assert!(e.contains("terminal"), "{e}");
        assert!(require_terminal(true).is_ok());
    }

    #[test]
    fn the_default_plan_path_is_the_working_directory_never_a_temp_dir() {
        assert_eq!(plan_path(None), PathBuf::from("comic-renamer-plan.jsonl"));
        assert_eq!(plan_path(Some("x/p.jsonl".into())), PathBuf::from("x/p.jsonl"));
    }
}
