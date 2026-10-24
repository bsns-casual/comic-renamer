//! The startup menu: `comic-renamer` with no arguments.
//!
//! Two kinds of record can be waiting, and the screen words them differently.
//! An *interrupted* run died part-way and left a library half-migrated, so it
//! comes first: it is the only item about a library that is inconsistent right
//! now. A *completed* run from an earlier invocation is intact and merely still
//! reversible. Describing the second as a crash would tell the user their last
//! clean `apply` went wrong.
//!
//! `render` is a pure function of the summaries and `dispatch` maps a keystroke
//! to an intent without performing it, so the wording and every key are tested
//! without a terminal. `run` is the thin shell that reads a line and acts.
//!
//! No record path is ever printed. A run id is a file name, not a path, and
//! may be shown; nothing here shows one.

use comic_renamer_core::types::Mode;
use comic_renamer_core::{record, undo::{self, Summary}};
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// What a keystroke asks for. Nothing has happened yet when one is returned.
#[derive(Debug, PartialEq)]
pub enum Key {
    Reverse(PathBuf),
    Decline(PathBuf),
    Map,
    Scan,
    Apply,
    ReverseSession,
    Quit,
}

/// Records worth offering: not held by a live run, at least one row to put
/// back, and readable. Interrupted first; newest first within each group,
/// which is the order `record::list` returns and a stable sort keeps.
pub fn waiting(records: &Path) -> std::io::Result<Vec<(PathBuf, Summary)>> {
    // A store that was never created holds nothing.
    if !records.exists() {
        return Ok(Vec::new());
    }
    let mut found: Vec<(PathBuf, Summary)> = record::list(records)?
        .into_iter()
        // An unanswerable liveness question is treated as live: offering to
        // reverse a record a running `apply` is still writing is the worse error.
        .filter(|path| !record::is_live(path).unwrap_or(true))
        .filter_map(|path| undo::summarise(&path).ok().map(|s| (path, s)))
        .filter(|(_, s)| s.reversible > 0)
        .collect();
    found.sort_by_key(|(_, s)| !s.interrupted);
    Ok(found)
}

/// `YYYY-MM-DD` in UTC. Days-to-civil from Howard Hinnant's date algorithms.
fn date(t: SystemTime) -> String {
    let days = t
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| (d.as_secs() / 86_400) as i64);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn first_interrupted(waiting: &[(PathBuf, Summary)]) -> Option<&(PathBuf, Summary)> {
    waiting.iter().find(|(_, s)| s.interrupted)
}

fn first_completed(waiting: &[(PathBuf, Summary)]) -> Option<&(PathBuf, Summary)> {
    waiting.iter().find(|(_, s)| !s.interrupted)
}

/// The screen. Offers the newest record of each kind; older ones stay
/// reachable through `undo --list`.
pub fn render(waiting: &[(PathBuf, Summary)], session_run: bool) -> String {
    let mut out = String::from("comic-renamer\n\n");
    if let Some((_, s)) = first_interrupted(waiting) {
        out += &format!(
            "A run on {} was interrupted ({} of {} rows placed). \
             Your library at {} is partly migrated.\n  \
             r  put the placed files back\n  \
             l  leave it for now; this stays here until you deal with it\n\n",
            date(s.started),
            s.reversible,
            s.planned,
            s.dest_root.display(),
        );
    }
    if let Some((_, s)) = first_completed(waiting) {
        out += &format!(
            "An earlier run placed {} files on {} in {}.\n  \
             u  undo it\n  \
             k  keep the files, and stop offering to undo this run\n\n",
            s.reversible,
            date(s.started),
            s.dest_root.display(),
        );
    }
    out += "  1  map a bundle\n  2  scan a bundle\n  3  apply a plan\n";
    if session_run {
        out += "  4  undo the run you just made\n";
    }
    out += "  q  quit\n";
    out
}

/// A keystroke to an intent. `l` on an interrupted offer returns `None` on
/// purpose: leaving a half-migrated library declines nothing, so its record
/// stays. Declining a *completed* run (`k`) is a decision to forget it.
pub fn dispatch(key: char, waiting: &[(PathBuf, Summary)]) -> Option<Key> {
    match key.to_ascii_lowercase() {
        'r' => first_interrupted(waiting).map(|(p, _)| Key::Reverse(p.clone())),
        'u' => first_completed(waiting).map(|(p, _)| Key::Reverse(p.clone())),
        'k' => first_completed(waiting).map(|(p, _)| Key::Decline(p.clone())),
        '1' => Some(Key::Map),
        '2' => Some(Key::Scan),
        '3' => Some(Key::Apply),
        '4' => Some(Key::ReverseSession),
        'q' => Some(Key::Quit),
        _ => None,
    }
}

fn ask(prompt: &str, default: Option<&str>) -> Result<String, String> {
    let mut stdout = std::io::stdout();
    match default {
        Some(d) => write!(stdout, "{prompt} [{d}]: "),
        None => write!(stdout, "{prompt}: "),
    }
    .and_then(|()| stdout.flush())
    .map_err(|e| e.to_string())?;
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line).map_err(|e| e.to_string())?;
    let line = line.trim();
    if read == 0 {
        return Err("no input; stopping".into());
    }
    match (line.is_empty(), default) {
        (true, Some(d)) => Ok(d.to_string()),
        (true, None) => Err(format!("{prompt} is required")),
        _ => Ok(line.to_string()),
    }
}

/// Paths typed at a prompt, with `~/` and surrounding quotes dropped into the
/// shape a shell would have given `scan`.
fn ask_path(prompt: &str) -> Result<PathBuf, String> {
    let typed = ask(prompt, None)?;
    Ok(PathBuf::from(typed.trim_matches(|c| c == '\'' || c == '"')))
}

fn ask_mode() -> Result<Mode, String> {
    match ask("move or copy the files", Some("move"))?.to_ascii_lowercase().as_str() {
        "move" => Ok(Mode::Move),
        "copy" => Ok(Mode::Copy),
        other => Err(format!("expected move or copy, got {other:?}")),
    }
}

pub fn run(records: &Path) -> Result<(), String> {
    crate::map::require_terminal(
        "the menu",
        std::io::stdout().is_terminal(),
        std::io::stdin().is_terminal(),
    )?;
    // The record `apply` made from this menu, if it did. Item 4 exists for it.
    let mut session: Option<String> = None;
    loop {
        let waiting = waiting(records).map_err(|e| e.to_string())?;
        print!("\n{}", render(&waiting, session.is_some()));
        let choice = ask("choose", None)?;
        let Some(typed) = choice.chars().next() else { continue };
        // Leaving declines nothing: the interrupted record stays.
        if typed.eq_ignore_ascii_case(&'l') {
            return Ok(());
        }
        let Some(key) = dispatch(typed, &waiting) else {
            println!("not an option here");
            continue;
        };
        let done = match key {
            Key::Quit => return Ok(()),
            // shortcut: deleting a declined record lands with the decline task;
            // until then `k` leaves it, which is the safe side.
            Key::Decline(_) => return Ok(()),
            Key::Reverse(record) => crate::reverse_path(records, &record),
            Key::ReverseSession => match session.take() {
                Some(id) => crate::reverse_path(records, &records.join(id)),
                None => {
                    println!("not an option here");
                    continue;
                }
            },
            Key::Map => ask_bundle().and_then(|(source, dest, mode)| {
                crate::map::run(source, dest, None, "komga".into(), "en".into(), "Humble".into(), mode)
            }),
            Key::Scan => ask_bundle().and_then(|(source, dest, mode)| {
                crate::run_scan(source, dest, None, "komga".into(), "en".into(), "Humble".into(), mode)
            }),
            Key::Apply => {
                let before = ids(records);
                let result = ask_path("plan file")
                    .and_then(|plan| crate::run_apply(&plan, false, false, None));
                // The record this apply made is the one that was not there before.
                if let Some(id) = ids(records).into_iter().find(|id| !before.contains(id)) {
                    session = Some(id);
                }
                result
            }
        };
        // A failed action is reported and the menu comes back: the user chose
        // from a list, and an exit here would throw the list away.
        if let Err(message) = done {
            eprintln!("error: {message}");
        }
    }
}

fn ask_bundle() -> Result<(PathBuf, PathBuf, Mode), String> {
    Ok((
        ask_path("folder of comics")?,
        ask_path("library folder (must exist)")?,
        ask_mode()?,
    ))
}

/// File names in the store, never paths.
fn ids(records: &Path) -> Vec<String> {
    if !records.exists() {
        return Vec::new();
    }
    record::list(records)
        .unwrap_or_default()
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use comic_renamer_core::types::Mechanism;
    use comic_renamer_core::undo::UndoWriter;
    use std::time::Duration;

    fn p(name: &str) -> PathBuf {
        PathBuf::from(name)
    }

    fn summary(planned: u32, reversible: u32, interrupted: bool, secs: u64) -> Summary {
        Summary {
            source_root: p("/src"),
            dest_root: p("/lib"),
            preset: "komga".into(),
            planned,
            reversible,
            interrupted,
            started: UNIX_EPOCH + Duration::from_secs(secs),
        }
    }

    // 2026-10-24T00:00:00Z
    const DAY: u64 = 1_792_800_000;

    fn interrupted(placed: u32, planned: u32) -> Summary {
        summary(planned, placed, true, DAY)
    }

    fn completed(placed: u32) -> Summary {
        // One row more than was placed: a clean run can still have skipped a file.
        summary(placed + 1, placed, false, DAY - 86_400)
    }

    #[test]
    fn dates_are_utc_calendar_dates() {
        assert_eq!(date(UNIX_EPOCH), "1970-01-01");
        assert_eq!(date(UNIX_EPOCH + Duration::from_secs(DAY)), "2026-10-24");
        // A leap day and the day after it.
        assert_eq!(date(UNIX_EPOCH + Duration::from_secs(1_709_164_800)), "2024-02-29");
        assert_eq!(date(UNIX_EPOCH + Duration::from_secs(1_709_251_200)), "2024-03-01");
        // The last day of a 400-year cycle, where the century terms matter.
        assert_eq!(date(UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29");
    }

    #[test]
    fn the_two_offers_are_worded_differently_and_interrupted_comes_first() {
        let w = vec![(p("a"), interrupted(312, 400)), (p("b"), completed(400))];
        let screen = render(&w, false);
        assert!(screen.contains("interrupted (312 of 400 rows placed)"), "{screen}");
        assert!(screen.contains("placed 400 files on 2026-10-23"), "{screen}");
        assert!(screen.find("interrupted").unwrap() < screen.find("placed 400").unwrap());
        // The completed offer must never be called a crash, nor the crash a
        // finished run: that is the defect the split exists to prevent.
        let completed_only = render(&[(p("b"), completed(400))], false);
        assert!(!completed_only.contains("interrupted"), "{completed_only}");
        assert!(!completed_only.contains("partly migrated"), "{completed_only}");
        let interrupted_only = render(&[(p("a"), interrupted(3, 4))], false);
        assert!(!interrupted_only.contains("placed 3 files"), "{interrupted_only}");
        assert!(interrupted_only.contains("r  "));
        assert!(!interrupted_only.contains("u  undo"));
    }

    #[test]
    fn item_four_appears_only_with_a_run_from_this_session() {
        assert!(!render(&[], false).contains("  4  "));
        assert!(render(&[], true).contains("  4  "));
    }

    #[test]
    fn the_menu_never_prints_a_record_path() {
        let store = "/home/x/Library/Application Support/comic-renamer/runs";
        let w = vec![
            (p(&format!("{store}/.r1.record")), interrupted(1, 2)),
            (p(&format!("{store}/.r2.record")), completed(2)),
        ];
        for session in [false, true] {
            let screen = render(&w, session);
            for leak in ["Application Support", ".record", "/runs"] {
                assert!(!screen.contains(leak), "{leak}: {screen}");
            }
        }
    }

    #[test]
    fn r_reverses_the_interrupted_record_and_u_the_completed_one() {
        let w = vec![(p("a"), interrupted(312, 400)), (p("b"), completed(400))];
        assert!(matches!(dispatch('r', &w), Some(Key::Reverse(x)) if x == p("a")));
        assert!(matches!(dispatch('u', &w), Some(Key::Reverse(x)) if x == p("b")));
        assert!(matches!(dispatch('k', &w), Some(Key::Decline(x)) if x == p("b")));
        assert!(dispatch('l', &w).is_none(),"leaving an interrupted run declines nothing");
    }

    #[test]
    fn a_key_with_no_offer_behind_it_does_nothing() {
        let only_completed = vec![(p("b"), completed(4))];
        assert_eq!(dispatch('r', &only_completed), None);
        let only_interrupted = vec![(p("a"), interrupted(1, 4))];
        assert_eq!(dispatch('u', &only_interrupted), None);
        assert_eq!(dispatch('k', &only_interrupted), None);
        assert_eq!(dispatch('r', &[]), None);
    }

    #[test]
    fn the_numbered_keys_and_quit_map_to_their_actions() {
        assert_eq!(dispatch('1', &[]), Some(Key::Map));
        assert_eq!(dispatch('2', &[]), Some(Key::Scan));
        assert_eq!(dispatch('3', &[]), Some(Key::Apply));
        assert_eq!(dispatch('4', &[]), Some(Key::ReverseSession));
        assert_eq!(dispatch('Q', &[]), Some(Key::Quit));
        assert_eq!(dispatch('x', &[]), None);
    }

    /// A real record with `placed` rows done and, when `pending`, one more
    /// that never finished.
    fn record_with(dir: &Path, planned: u32, placed: u32, pending: bool) -> PathBuf {
        let path = dir.join(record::basename(SystemTime::now()));
        let src = dir.join("a.cbz");
        std::fs::write(&src, b"").unwrap();
        let mut w = UndoWriter::create(&path, Path::new("/src"), dir, "komga", planned).unwrap();
        let row = |w: &mut UndoWriter| {
            w.record_pending(
                Path::new("a.cbz"), Path::new("S/a.cbz"), Mechanism::Rename, Mode::Move,
                &src, "S", Some(1), None, 1, "cbz",
            )
            .unwrap()
        };
        for _ in 0..placed {
            let seq = row(&mut w);
            w.record_done(seq).unwrap();
        }
        if pending {
            row(&mut w);
        }
        path
    }

    #[test]
    fn a_live_or_empty_record_is_not_offered() {
        let dir = tempfile::tempdir().unwrap();
        let live = record_with(dir.path(), 2, 1, false);
        let empty = record_with(dir.path(), 2, 0, true);
        let good = record_with(dir.path(), 2, 2, false);
        std::fs::write(dir.path().join(record::basename(SystemTime::now())), "not a record").unwrap();
        let _held = record::hold(&live).unwrap();

        let offered: Vec<PathBuf> = waiting(dir.path()).unwrap().into_iter().map(|(p, _)| p).collect();
        assert_eq!(offered, vec![good.clone()], "live={live:?} empty={empty:?}");
    }

    #[test]
    fn waiting_puts_the_interrupted_record_first_even_when_it_is_older() {
        let dir = tempfile::tempdir().unwrap();
        let crashed = record_with(dir.path(), 3, 1, true);
        let newer = record_with(dir.path(), 2, 2, false);
        let order: Vec<PathBuf> = waiting(dir.path()).unwrap().into_iter().map(|(p, _)| p).collect();
        assert_eq!(order, vec![crashed, newer]);
    }

    #[test]
    fn a_store_that_was_never_created_holds_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(waiting(&dir.path().join("runs")).unwrap().is_empty());
    }
}
