//! `map`: open a mapping session for the files the database does not know.

use crate::{guesses_for, parse_overrides, OVERRIDES};
use comic_renamer_core::mapping::shipped_mappings;
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
pub(crate) fn require_terminal(
    what: &str,
    stdout_is_terminal: bool,
    stdin_is_terminal: bool,
) -> Result<(), String> {
    let hint = "Run it directly in a terminal, or use `scan` to write a plan without a session.";
    if !stdout_is_terminal {
        return Err(format!("{what} needs an interactive terminal, but stdout is not one. {hint}"));
    }
    if !stdin_is_terminal {
        return Err(format!("{what} needs an interactive terminal, but stdin is not one. {hint}"));
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
        let Some(row) = row_for(&e.stem, fields, count, "") else { continue };
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

fn row_for(stem: &str, fields: &ParsedTitle, count: Option<u32>, lang: &str) -> Option<MappingRow> {
    Some(MappingRow {
        key: key(stem)?,
        series: fields.series.clone(),
        volume: fields.volume,
        issue: fields.issue,
        subtitle: fields.subtitle.clone(),
        lang: lang.to_string(),
        volume_count: count,
    })
}

/// What `commit_session` did: rows written, and pairs of files that wanted one
/// key with different titles. Neither of a pair is written.
#[derive(Debug)]
pub struct Committed {
    pub written: usize,
    pub clashes: Vec<(String, String)>,
}

/// The approved entries as validated rows, plus the pairs of files that wanted
/// one key with different titles (neither of a pair is returned). Shared by the
/// local write and the contribution, so what is contributed is what was saved.
fn approved_rows(entries: &[table::Entry], lang: &str) -> Result<(Vec<MappingRow>, Vec<(String, String)>), String> {
    // Grouped by key, in first-seen order.
    let mut groups: Vec<Vec<(&str, MappingRow)>> = Vec::new();
    for e in entries {
        let Some((fields, count)) = e.approved_row() else { continue };
        let row = row_for(&e.stem, fields, count, lang)
            .ok_or_else(|| format!("{:?} has no letters or digits to key a mapping on", e.stem))?;
        row.validate().map_err(|err| format!("{:?}: {err}", e.stem))?;
        // Grouped on the key alone since D64. There is no bundle to agree
        // about any more, so two files whose names share a key clash whatever
        // folder they came from.
        match groups.iter_mut().find(|g| g[0].1.key == row.key) {
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
            // Only pairs that really differ: files given identical titles did not clash.
            for (i, (a, ra)) in g.iter().enumerate() {
                clashes.extend(g[i + 1..].iter().filter(|(_, rb)| rb != ra).map(|(b, _)| (a.to_string(), b.to_string())));
            }
        }
    }
    Ok((fresh, clashes))
}

/// Writes the approved entries into the user's local mapping file. Every new
/// row is validated first and any failure leaves the file as it was; the file
/// then lands via a sibling temp file and a rename, so an interrupted write
/// cannot truncate corrections no other record holds.
/// `lang` is not on an `Entry`, so the caller supplies it.
///
/// An existing file that does not load aborts: overwriting it would drop the
/// user's older corrections. A row with the same key is replaced.
/// Files whose names share a key but whose titles differ are reported and
/// left out, so the rest of the session is kept.
// shortcut: no lock around the read-modify-write, so two sessions inside the
// read-to-rename window lose one set of rows; an advisory lock beside the file
// (as apply's lockfile does) closes it. The *file* is safe either way now —
// each writer has its own temp name, so the loser's rows are missing rather
// than spliced into the winner's.
pub fn commit_session(entries: &[table::Entry], local: &Path, lang: &str) -> Result<Committed, String> {
    let (fresh, clashes) = approved_rows(entries, lang)?;
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
        // Replaced in place, never appended (spec §2.3): the user mapped this
        // filename again and typed something different, and the newer
        // keystroke wins. An append would put a duplicate key in the file and
        // `parse_tsv` would then refuse to load the user's own corrections.
        match rows.iter_mut().find(|r| r.key == row.key) {
            Some(slot) => *slot = row,
            None => rows.push(row),
        }
    }
    let dir = local.parent().ok_or("the mapping file has no parent directory")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    // Unique per writer, through the one name generator `apply`'s temps use. A
    // fixed name plus `File::create`, which truncates, let a second session
    // write over this one's half-finished temp: the rename then published two
    // files spliced together, torn mid-row, and `parse_tsv` refused the result
    // for both `scan` and `map` until a human repaired it.
    let tmp = dir.join(comic_renamer_core::fsops::temp_basename());
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(to_tsv(&rows).as_bytes())?;
        f.sync_all()?;
        // Replacing the old file is the point here; the no-clobber rule is for comics.
        std::fs::rename(&tmp, local)?;
        // `sync_all` makes the contents durable, not the name, as
        // `UndoWriter::create` says in the same situation.
        std::fs::File::open(dir)?.sync_all()
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", local.display())
    })?;
    Ok(Committed { written, clashes })
}

/// Where a contribution goes.
const UPSTREAM: &str = "bsns-casual/comic-renamer";

/// What the user is handed: the rows on disk and the commands that send them.
/// This tool runs none of the commands (§7.1's fallback is the whole path), so
/// the only thing the user agrees to is the file write and the printout.
pub struct Contribution {
    pub path: PathBuf,
    pub commands: Vec<String>,
}

/// The approved rows as a mapping file, and nothing else: no declined row, no
/// field beyond the seven, no path. A `bundle` is a folder's *name*; one that
/// carries a separator is a path fragment and is refused rather than trimmed.
pub fn contribution_tsv(entries: &[table::Entry], bundle: &str, lang: &str) -> Result<String, String> {
    if bundle.contains(['/', '\\']) {
        return Err(format!("{bundle:?} is a path, not a folder name; a contribution never carries one"));
    }
    let (rows, _) = approved_rows(entries, lang)?;
    Ok(to_tsv(&rows))
}

/// Single-quotes `s` for a shell unless it is plainly safe, for the commands
/// printed as manual steps.
pub(crate) fn quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_./:=@%+,-".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Writes `tsv` beside the local mappings and returns the commands that open
/// the pull request. Nothing is executed and no credential is touched: `gh`
/// owns authentication when the user runs the commands.
pub fn contribute(tsv: &str, branch: &str) -> Result<Contribution, String> {
    contribute_in(tsv, branch, &config_dir()?.join("contributions"))
}

fn contribute_in(tsv: &str, branch: &str, dir: &Path) -> Result<Contribution, String> {
    let ok_branch = !branch.is_empty()
        && !branch.starts_with(['-', '.'])
        && branch.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok_branch {
        return Err(format!("{branch:?} is not a usable branch name"));
    }
    let n = tsv.lines().count().saturating_sub(1);
    if n == 0 {
        return Err("there are no approved mappings to contribute".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("{branch}.tsv"));
    std::fs::write(&path, tsv).map_err(|e| format!("{}: {e}", path.display()))?;
    // One target, always (D66). This used to branch on whether the
    // marketplace had a file to append to, and printed nothing when it did
    // not — which was Humble, the default. One table removes the question.
    let target = "data/decisions.tsv";

    let repo = UPSTREAM.rsplit('/').next().unwrap();
    let title = format!("data: {n} mapping{} from {branch}", if n == 1 { "" } else { "s" });
    let work = dir.join(branch);
    let commands = vec![
        format!("mkdir -p {0} && cd {0}", quote(&work.to_string_lossy())),
        format!("gh repo fork {UPSTREAM} --clone"),
        format!("cd {repo}"),
        format!("git checkout -b {branch}"),
        // Appended, header skipped: the table already ships with one, and a
        // second header line is not a row.
        format!("tail -n +2 {} >> {target}", quote(&path.to_string_lossy())),
        format!("git add {target}"),
        format!("git commit -m {}", quote(&title)),
        format!("git push -u origin {branch}"),
        format!("gh pr create --repo {UPSTREAM} --title {} --body 'Mappings contributed from comic-renamer map.'", quote(&title)),
    ];
    Ok(Contribution { path, commands })
}

/// Anything but an explicit yes declines: a contribution is public.
fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Offers the contribution after the local save (§6.5). Reads one line from
/// stdin; the file is written only when the user types yes.
fn offer_contribution(entries: &[table::Entry], bundle: &str, lang: &str) -> Result<(), String> {
    let tsv = contribution_tsv(entries, bundle, lang)?;
    if tsv.lines().count() < 2 {
        return Ok(());
    }
    println!("These mappings can be contributed so the next person who buys this bundle gets them for free.");
    println!("Saying yes writes them to a file and prints the commands for a public pull request. This tool runs none of the commands.");
    print!("Prepare the contribution? [y/N] ");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).map_err(|e| e.to_string())?;
    if !is_yes(&answer) {
        println!("Kept local only.");
        return Ok(());
    }
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let c = contribute(&tsv, &branch_for(bundle, secs))?;
    for line in guidance(&c) {
        println!("{line}");
    }
    for line in &c.commands {
        println!("  {line}");
    }
    Ok(())
}

/// Branch for one contribution. The seconds make a second contribution from
/// one bundle a new branch, directory and file; a bundle with no ASCII letters
/// falls back to `bundle` so it is not `map--`.
/// A name as it appears in a branch: ASCII alphanumerics kept and lowercased,
/// every run of anything else collapsed to one `-`.
///
/// Lived in `core::mapping` until 2026-10-30, where `shipped_mapping_path` was
/// its other caller until D66 deleted that. A branch-name slugifier is not a
/// mapping-format concern.
fn file_slug(name: &str) -> String {
    let rough: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    rough.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}

fn branch_for(bundle: &str, secs: u64) -> String {
    let slug = file_slug(bundle);
    let slug = if slug.is_empty() { "bundle" } else { &slug };
    format!("map-{slug}-{secs}")
}

/// The prose printed above the commands. It names where the clone lands and
/// which commands an owner skips, because the commands are all the user has.
fn guidance(c: &Contribution) -> Vec<String> {
    let work = c.path.with_extension("");

    vec![
        format!("Your rows are in {}.", c.path.display()),
        "To send them you need `gh` signed in (`gh auth login`).".to_string(),
        format!(
            "The commands below create {} and fork {UPSTREAM} into your GitHub account, cloning it to {}.",
            work.display(),
            work.join(UPSTREAM.rsplit('/').next().unwrap()).display()
        ),
        "If you own that repository, skip the first three commands and run the rest in your own clone.".to_string(),
        "Then run:".to_string(),
    ]
}


fn clash_warning(a: &str, b: &str) -> String {
    format!(
        "warning: {a} and {b} give the same mapping key but you titled them differently, so neither was saved. Rename one of the files so the names differ in more than punctuation, case or spacing, then run `map` again."
    )
}

/// At most this many rows are listed before the count takes over. A 300-file
/// bundle must not push the question itself off the screen.
const ROWS_LISTED: usize = 30;

/// What the user is agreeing to (§6.5): every approved row as filename → final
/// name, then the two things the save is about to do that they cannot see from
/// the table — rows that would land on one path, and rows whose filenames share
/// a mapping key. Both used to be printed after the write, which informs
/// without consenting.
fn confirmation(
    entries: &[table::Entry],
    writing: usize,
    clashes: &[(String, String)],
    collisions: &[(String, String)],
    local: &Path,
) -> String {
    let mut out = String::from("\n");
    let listed: Vec<&table::Entry> = entries.iter().filter(|e| e.approved_row().is_some()).collect();
    for e in listed.iter().take(ROWS_LISTED) {
        let (fields, _) = e.approved_row().expect("filtered");
        out.push_str(&format!(
            "  {}  ->  {}\n",
            e.stem,
            table::show(&fields.series, fields.volume, fields.issue, fields.subtitle.as_deref())
        ));
    }
    if listed.len() > ROWS_LISTED {
        out.push_str(&format!("  ...and {} more\n", listed.len() - ROWS_LISTED));
    }
    for (a, b) in collisions {
        out.push_str(&format!("\n{}\n", collision_warning(a, b)));
    }
    for (a, b) in clashes {
        out.push_str(&format!("\n{}\n", clash_warning(a, b)));
    }
    out.push_str(&format!(
        "\nSave {writing} {} to {}? [y/N] ",
        if writing == 1 { "mapping" } else { "mappings" },
        local.display()
    ));
    out
}

/// Shows the confirmation, then writes only on a yes. `ask` takes the text and
/// returns the answer, so "present it, *then* write" is a property a test can
/// hold rather than an ordering in one function nobody can call.
///
/// The contribution offer is the caller's, not this function's: it has a
/// prompt of its own, and a test of the write path must not need a second
/// answer on stdin to reach the end of it.
fn save(
    entries: &[table::Entry],
    local: &Path,
    lang: &str,
    preset: &dyn Preset,
    ask: impl FnOnce(&str) -> Result<bool, String>,
) -> Result<Outcome, String> {
    // The same pure call `commit_session` makes, so the two cannot disagree
    // about which rows are about to be saved.
    let (rows, clashes) = approved_rows(entries, lang)?;
    let collisions = intra_session_collisions(entries, preset);
    if !ask(&confirmation(entries, rows.len(), &clashes, &collisions, local))? {
        println!("Nothing was saved.");
        return Ok(Outcome::Quit);
    }
    let done = commit_session(entries, local, lang)?;
    // `commit_session` recomputes these from the same entries, so a
    // disagreement would mean the confirmation described a different save
    // from the one that happened.
    debug_assert_eq!(done.clashes, clashes, "the confirmation and the write disagree");
    let n = done.written;
    println!("{n} {} saved to {}", if n == 1 { "mapping" } else { "mappings" }, local.display());
    Ok(Outcome::Done)
}

/// One line from the terminal. EOF reads as a refusal, as `apply`'s prompt
/// has it: nobody typed a yes.
fn ask_on_stdin(text: &str) -> Result<bool, String> {
    print!("{text}");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).map_err(|e| e.to_string())?;
    Ok(is_yes(&answer))
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
    let mappings = Mappings::load(shipped_mappings(), local.as_deref()).map_err(|e| e.to_string())?;
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
    // NFC through the one function `map` records with. No longer validated:
    // since D64 nothing stores a folder name, so there is nothing to refuse.
    let folder = comic_renamer_core::mapping::bundle_name(
        &source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
    );
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
        |plan| crate::run_apply(plan, false, false, None).map(|_| ()),
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
        require_terminal("`map`", stdout_tty, stdin_tty)?;
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
            let outcome = save(&t.entries, local, lang, preset, ask_on_stdin)?;
            if let Outcome::Done = outcome {
                // The mappings are already saved; a failed offer must not make
                // the run report failure.
                if let Err(e) = offer_contribution(&t.entries, folder, lang) {
                    println!("warning: the contribution was not prepared: {e}");
                }
            }
            Ok(outcome)
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

    /// `bundle` is `source.file_name()`, not user input, and
    /// `MappingRow::validate` ends with it — so a folder name that cannot be a
    /// field failed *after* the session, throwing away every correction the
    /// user had just typed, with an error that blamed a filename. HFS+ stored
    /// NFD and those names survive copies, so `Café Comics` is an ordinary
    /// folder on macOS.
    /// The same folder, typed in a different case. On APFS that is one
    /// directory, so the second session must correct the row the first wrote
    /// rather than add a second one -- a pair the loader now refuses, which
    /// would stop the user's own file from loading at all.
    #[test]
    fn a_second_session_corrects_the_row_instead_of_adding_one() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("mappings.tsv");
        commit_session(&[approved("a_vol1", "A", Some(1), None)], &local, "en").unwrap();
        // A different folder entirely, which is the stronger version of the
        // case-only test this replaces: identity is the key (D64), so the
        // second session corrects the first row wherever the files came from.
        let done =
            commit_session(&[approved("a_vol1", "Corrected", Some(1), None)], &local, "en")
                .unwrap();
        assert_eq!(done.written, 1);

        let rows = read_rows(&local);
        assert_eq!(rows.len(), 1, "one key is one row, not two: {rows:?}");
        assert_eq!(rows[0].series, "Corrected");
        assert!(
            comic_renamer_core::mapping::Mappings::load(
                comic_renamer_core::mapping::shipped_mappings(),
                Some(&local)
            )
            .is_ok(),
            "and the file the session wrote still loads"
        );
    }

    /// `mapping::bundle_name` still normalises, because Layer B's override keys are
    /// composed and HFS+ hands folder names back decomposed. It no longer
    /// validates: `a_folder_name_that_cannot_be_a_bundle_is_refused_up_front`
    /// retired with the stored field (D64), since nothing can now be hurt by a
    /// long or oddly-punctuated folder name — it is a map key and a branch slug.
    #[test]
    fn a_folder_name_is_normalised_and_never_refused() {
        use comic_renamer_core::mapping::bundle_name;
        assert_eq!(bundle_name("Cafe\u{301} Comics"), "Caf\u{e9} Comics");
        for once_refused in ["-Free Comics", "=Humble", "@bundle", "+extra", &"x".repeat(121)] {
            assert_eq!(bundle_name(once_refused), once_refused, "passed through now");
        }
        assert_eq!(bundle_name(""), "");

        // And a session under such a folder completes, where it used to be
        // refused before it opened.
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("mappings.tsv");
        let done = commit_session(&[approved("a_vol1", "A", Some(1), None)], &local, "en").unwrap();
        assert_eq!(done.written, 1);
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
        assert!(require_terminal("`map`", true, true).is_ok());
        let out = require_terminal("`map`", false, true).unwrap_err();
        assert!(out.contains("stdout") && out.contains("terminal"), "{out}");
        let inp = require_terminal("`map`", true, false).unwrap_err();
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

    /// `run_with` scans twice precisely so the plan carries the rows the user
    /// just approved. Nothing inspected those rows: the stub session above
    /// writes no mapping file, so `first` and `finished` were identical and
    /// `plan::write(&first.…)` passed the suite — a plan holding none of the
    /// user's work, with the whole chain green.
    ///
    /// This is also the only test of the round trip itself: approve → commit →
    /// re-scan → the plan carries it. Every link was tested alone.
    #[test]
    fn the_plan_carries_the_rows_the_session_just_approved() {
        let (_dir, opts, plan, config) = unmapped_bundle();
        let local = config.join("mappings.tsv");
        run_with(
            &opts,
            plan.clone(),
            &config,
            (true, true),
            |first| {
                assert_eq!(first.rows.len(), 0, "the file starts unmapped");
                assert_eq!(first.unmapped.len(), 1);
                let stem = first.unmapped[0].file_stem().unwrap().to_string_lossy().into_owned();
                let e = approved(&stem, "Nothing At All", Some(1), None);
                assert_eq!(commit_session(&[e], &local, "en").unwrap().written, 1);
                Ok(Outcome::Done)
            },
            |_| Ok(()),
        )
        .unwrap();
        let (header, rows) = comic_renamer_core::plan::read(&plan).unwrap();
        assert_eq!(rows.len(), 1, "the approved row is in the plan");
        assert_eq!(rows[0].series, "Nothing At All");
        assert_eq!(header.unmapped, 0, "and is no longer counted as unmapped");
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
            issue: None,
            subtitle: subtitle.map(String::from),
            volume_count: count,
            source: SuggestionSource::Corpus,
        }
    }

    fn approved(stem: &str, series: &str, volume: Option<u32>, count: Option<u32>) -> table::Entry {
        let mut e = table::Entry::new(stem.into(), vec![sug(series, volume, None, count)]).with_extension("cbz");
        assert!(e.accept_edit(ParsedTitle { series: series.into(), volume, issue: None, subtitle: None }));
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
        pending.final_fields = Some(ParsedTitle { series: "Usagi Yojimbo".into(), volume: Some(1), issue: None, subtitle: None });
        let entries = vec![approved("a", "Usagi Yojimbo", Some(1), None), approved("b", "Usagi Yojimbo", Some(2), None), pending];
        assert!(intra_session_collisions(&entries, &Komga).is_empty(), "an unapproved row is never written, so never collides");
    }

    #[test]
    fn only_approved_rows_reach_the_local_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("config/mappings.tsv");
        let entries = vec![approved("alpha_vol1", "Alpha", Some(1), None), unapproved("beta_vol1", "Beta")];
        assert_eq!(commit_session(&entries, &local, "en").unwrap().written, 1);
        let text = std::fs::read_to_string(&local).unwrap();
        assert!(text.contains("alphavol1\tAlpha"), "{text}");
        assert!(!text.contains("Beta") && !text.contains("betavol1"), "{text}");
    }

    #[test]
    fn nothing_approved_writes_no_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("mappings.tsv");
        assert_eq!(commit_session(&[unapproved("x", "X")], &local, "en").unwrap().written, 0);
        assert!(!local.exists());
    }

    #[test]
    fn volume_count_survives_into_the_written_row() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("mappings.tsv");
        commit_session(&[approved("lw_vol1", "Lone Wolf", Some(1), Some(128))], &local, "en").unwrap();
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
        assert!(e.accept_edit(ParsedTitle { series: "Something Else".into(), volume: Some(1), issue: None, subtitle: None }));
        commit_session(&[e], &local, "en").unwrap();
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
            "en",
        )
        .unwrap();
        let before = std::fs::read(&local).unwrap();
        assert!(String::from_utf8_lossy(&before).contains("Older"));
        // "=SUM" starts with a spreadsheet formula character, which validate() rejects.
        let entries = vec![approved("good_vol1", "Good", Some(1), None), approved("bad_vol1", "=SUM(A1)", Some(1), None)];
        let err = commit_session(&entries, &local, "en").unwrap_err();
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
        let done = commit_session(&entries, &local, "en").unwrap();
        assert_eq!(done.written, 1);
        assert_eq!(done.clashes, vec![("hellboy-1".to_string(), "hellboy1".to_string())]);
        let text = std::fs::read_to_string(&local).unwrap();
        assert!(text.contains("Good") && !text.contains("Hellboy"), "neither clashing row is guessed at: {text}");
        let msg = clash_warning("hellboy-1", "hellboy1");
        assert!(msg.contains("hellboy-1") && msg.contains("hellboy1") && msg.contains("Rename one"), "{msg}");
    }

    /// The temp name was fixed — `local.with_extension("tsv.tmp")` — and
    /// `File::create` truncates, so interleaving two sessions (A create, B
    /// create, A write 10k, B write 8k, A rename) left `mappings.tsv` holding
    /// B's bytes spliced onto A's tail, torn mid-row. `parse_tsv` then fails
    /// on column count and **both `scan` and `map` refuse** until a human
    /// repairs the file. The old `shortcut:` comment claimed the exposure was
    /// one set of rows.
    #[test]
    fn concurrent_sessions_cannot_splice_the_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("mappings.tsv");
        std::thread::scope(|s| {
            for i in 0..8 {
                let local = local.clone();
                s.spawn(move || {
                    let e = approved(&format!("file{i}_vol1"), &format!("Series {i}"), Some(1), None);
                    commit_session(&[e], &local, "en").unwrap();
                });
            }
        });
        // Whichever interleaving won, the file is a mapping file. Losing a
        // set of rows is the documented shortcut; a torn one is not.
        assert!(!read_rows(&local).is_empty());
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n != "mappings.tsv")
            .collect();
        assert!(left.is_empty(), "temp files left behind: {left:?}");
    }

    /// The confirmation is the last thing the user sees before the rows land,
    /// so both kinds of warning have to be in it. Printed afterwards they
    /// inform without consenting.
    #[test]
    fn the_confirmation_names_the_rows_and_both_warnings() {
        let entries = vec![
            approved("one", "Usagi Yojimbo", None, None),
            approved("two", "usagi yojimbo", None, None),
            approved("hellboy-1", "Hellboy", Some(1), None),
            approved("hellboy1", "Hellboy Omnibus", Some(1), None),
        ];
        let (rows, clashes) = approved_rows(&entries, "en").unwrap();
        let collisions = intra_session_collisions(&entries, &Komga);
        let text = confirmation(&entries, rows.len(), &clashes, &collisions, Path::new("/cfg/mappings.tsv"));
        assert!(text.contains("one") && text.contains("Usagi Yojimbo"), "{text}");
        assert!(text.contains("same path") && text.contains("neither is placed"), "the collision: {text}");
        assert!(text.contains("neither was saved"), "the clash: {text}");
        assert!(text.contains("/cfg/mappings.tsv"), "{text}");
        assert!(text.trim_end().ends_with("[y/N]"), "it ends by asking: {text}");
    }

    #[test]
    fn declining_the_confirmation_writes_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("mappings.tsv");
        let mut shown = None;
        let out = save(&[approved("a_vol1", "A", Some(1), None)], &local, "en", &Komga, |text| {
            shown = Some(text.to_string());
            Ok(false)
        })
        .unwrap();
        assert!(matches!(out, Outcome::Quit));
        assert!(!local.exists(), "a declined save writes nothing at all");
        assert!(shown.unwrap().contains("a_vol1"));
    }

    #[test]
    fn the_warnings_are_shown_while_the_file_is_still_untouched() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("mappings.tsv");
        let entries = vec![
            approved("one", "Usagi Yojimbo", None, None),
            approved("two", "usagi yojimbo", None, None),
        ];
        let out = save(&entries, &local, "en", &Komga, |text| {
            assert!(text.contains("same path"), "{text}");
            assert!(!local.exists(), "the rows were written before the user was asked");
            Ok(true)
        })
        .unwrap();
        assert!(matches!(out, Outcome::Done));
        assert_eq!(read_rows(&local).len(), 2, "a yes saves them");
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
        assert!(e.accept_edit(ParsedTitle { series: "Black Is the Color".into(), volume: Some(1), issue: None, subtitle: None }));
        commit_session(&[e], &local, "en").unwrap();
        assert_eq!(read_rows(&local)[0].volume_count, Some(128));
    }

    #[test]
    fn earlier_corrections_are_kept_and_a_repeat_replaces_its_own_row() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("mappings.tsv");
        commit_session(&[approved("old_vol1", "Old", Some(1), None)], &local, "en").unwrap();
        commit_session(
            &[approved("new_vol1", "New", Some(1), None), approved("old_vol1", "Older", Some(1), None)],
            &local,
            "en",
        )
        .unwrap();
        let rows = read_rows(&local);
        let series: Vec<_> = rows.iter().map(|r| r.series.as_str()).collect();
        assert_eq!(series, ["Older", "New"]);
    }

    #[test]
    fn an_unreadable_existing_file_aborts_rather_than_being_overwritten() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("mappings.tsv");
        std::fs::write(&local, "not a mapping file").unwrap();
        assert!(commit_session(&[approved("a", "A", Some(1), None)], &local, "en").is_err());
        assert_eq!(std::fs::read_to_string(&local).unwrap(), "not a mapping file");
        assert!(!dir.path().join("mappings.tsv.tmp").exists());
    }

    #[test]
    fn only_pairs_that_differ_are_reported_as_clashes() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("mappings.tsv");
        let entries = vec![
            approved("hellboy-1", "Hellboy", Some(1), None),
            approved("hellboy_1", "Hellboy", Some(1), None),
            approved("hellboy1", "Other", Some(1), None),
        ];
        let done = commit_session(&entries, &local, "en").unwrap();
        assert_eq!(done.written, 0);
        assert_eq!(
            done.clashes,
            vec![("hellboy-1".into(), "hellboy1".into()), ("hellboy_1".into(), "hellboy1".into())]
        );
    }

    #[test]
    fn two_files_with_one_key_and_identical_rows_lose_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("mappings.tsv");
        let entries = vec![approved("hellboy-1", "Hellboy", Some(1), None), approved("hellboy1", "Hellboy", Some(1), None)];
        assert_eq!(commit_session(&entries, &local, "en").unwrap().written, 1);
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

    // ---- contribution (Task 11). The tool runs no command, so there is
    // nothing to fake: `contribute_in` returns text and writes one file.

    fn one_row_tsv() -> String {
        contribution_tsv(&[approved("alpha_vol1", "Alpha", Some(1), None)], "Humble Manga", "en").unwrap()
    }

    /// Re-scoped. It used to feed a clean stem and a clean series and assert
    /// `!tsv.contains('/')`, which proved nothing and was wrong in general:
    /// D47 stores `Hack/Slash` verbatim, so that assertion would fail on a
    /// legitimate row. What it can prove is the opposite pairing — a series
    /// whose own name holds a slash survives intact, while the row still
    /// carries nothing from the filesystem. The path-shaped *stem* case is
    /// `a_path_like_stem_cannot_put_a_path_in_the_contribution` below.
    #[test]
    fn a_slash_in_a_series_is_stored_verbatim_and_is_not_a_path() {
        let tsv = contribution_tsv(
            &[approved("hackslash_vol1", "Hack/Slash", Some(1), None)],
            "Humble Manga",
            "en",
        )
        .unwrap();
        assert!(tsv.contains("Hack/Slash"), "D47: the real title is stored as published: {tsv}");
        assert_eq!(tsv.matches('/').count(), 1, "and nothing else carries one: {tsv}");
        if let Ok(home) = std::env::var("HOME") {
            assert!(!tsv.contains(&home), "{tsv}");
        }
    }

    #[test]
    fn a_bundle_name_that_is_a_path_fragment_is_refused() {
        for bad in ["a/b", "/Users/mike/Comics", "..\\x"] {
            let r = contribution_tsv(&[approved("alpha_vol1", "Alpha", Some(1), None)], bad, "en");
            assert!(r.is_err(), "{bad:?}");
        }
    }

    #[test]
    fn rows_the_user_declined_to_contribute_stay_local() {
        let entries = vec![approved("alpha_vol1", "Alpha", Some(1), None), unapproved("beta_vol1", "Beta")];
        let tsv = contribution_tsv(&entries, "B", "en").unwrap();
        assert!(tsv.contains("Alpha") && !tsv.contains("Beta"), "{tsv}");
    }

    #[test]
    fn a_contribution_holds_exactly_the_row_fields() {
        let tsv = one_row_tsv();
        assert_eq!(comic_renamer_core::mapping::parse_tsv(&tsv).unwrap().len(), 1);
        assert_eq!(tsv.lines().nth(1).unwrap().split('\t').count(), 7, "{tsv}");
    }

    #[test]
    fn the_contribution_is_a_file_and_commands_for_the_user_to_run() {
        let dir = tempfile::tempdir().unwrap();
        let c = contribute_in(&one_row_tsv(), "map-b", dir.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&c.path).unwrap(), one_row_tsv());
        assert!(c.commands.iter().any(|l| l.starts_with("gh pr create")), "{:?}", c.commands);
        // Appended to the one shipped table, never a new file named after the
        // branch. An unembedded TSV under `data/` fails
        // `the_decisions_file_is_the_one_that_ships` on arrival -- true only
        // since that test was made recursive on 2026-10-30. Before then a
        // `data/mappings/<branch>.tsv`, which is exactly the shape of the
        // 2026-10-29 bug this test is about, passed every gate.
        assert!(
            c.commands.iter().any(|l| l.contains("data/decisions.tsv") && l.starts_with("tail -n +2")),
            "{:?}",
            c.commands
        );
        assert!(!c.commands.iter().any(|l| l.contains("data/mappings/map-b.tsv")), "{:?}", c.commands);
    }

    /// Observed 2026-10-29 on a real `map` run: the printed commands copied the
    /// rows to `data/mappings/map-<bundle>-<secs>.tsv`, and following them
    /// literally failed the embedding guard — the contribution channel produced
    /// a pull request that was red before anyone read it.
    ///
    /// `a_marketplace_with_no_shipped_file_prints_no_commands` lived here and
    /// retires with the sharding (D66). It existed because Humble, the
    /// *default* marketplace, had no file to append to and the honest thing was
    /// to print nothing. One table means there is always a target, so the
    /// branch, the "not open yet" prose and the `marketplace` field on
    /// `Contribution` all go. This is what replaces it: the one target is
    /// always named, whatever the marketplace was.
    #[test]
    fn a_contribution_always_has_one_target() {
        let dir = tempfile::tempdir().unwrap();
        let c = contribute_in(&one_row_tsv(), "map-b", dir.path()).unwrap();

        assert_eq!(std::fs::read_to_string(&c.path).unwrap(), one_row_tsv());
        assert!(
            c.commands.iter().any(|l| l.starts_with("tail -n +2") && l.ends_with("data/decisions.tsv")),
            "{:?}",
            c.commands
        );
        assert_eq!(
            c.commands.iter().filter(|l| l.contains("data/")).count(),
            2,
            "the append and the add, and no other path: {:?}",
            c.commands
        );
        // Never a *target* named after the branch: that was the 2026-10-29
        // bug. Checked on the right-hand side of the redirect, because the
        // left-hand side is the contribution file and is legitimately called
        // `map-b.tsv` — an earlier version of this assertion looked for the
        // two strings anywhere in the line and so could never pass.
        let appended = c.commands.iter().find(|l| l.starts_with("tail -n +2")).expect("an append");
        let target = appended.rsplit(">>").next().unwrap().trim();
        assert_eq!(target, "data/decisions.tsv", "{appended}");
    }

    #[test]
    fn every_action_that_reaches_a_remote_or_the_filesystem_is_a_printed_command() {
        // The offer promises the user will see these before anything happens.
        let dir = tempfile::tempdir().unwrap();
        let c = contribute_in(&one_row_tsv(), "map-b", dir.path()).unwrap();
        for needle in ["gh repo fork", "mkdir -p", "git push", "gh pr create"] {
            assert!(c.commands.iter().any(|l| l.contains(needle)), "{needle}: {:?}", c.commands);
        }
    }

    #[test]
    fn no_command_carries_a_token_or_credential() {
        let dir = tempfile::tempdir().unwrap();
        let c = contribute_in(&one_row_tsv(), "map-b", dir.path()).unwrap();
        for l in &c.commands {
            let l = l.to_lowercase();
            assert!(!l.contains("token") && !l.contains("password") && !l.contains("ghp_"), "{l}");
        }
    }

    #[test]
    fn a_branch_that_could_be_a_flag_or_a_path_is_refused_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["", "-x", "../x", "a/b", "a b", ".hidden"] {
            assert!(contribute_in(&one_row_tsv(), bad, dir.path()).is_err(), "{bad:?}");
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn an_empty_contribution_is_refused_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let header_only = contribution_tsv(&[unapproved("x", "X")], "B", "en").unwrap();
        assert!(contribute_in(&header_only, "map-b", dir.path()).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn only_an_explicit_yes_prepares_a_contribution() {
        for yes in ["y", "Y\n", " yes "] {
            assert!(is_yes(yes), "{yes:?}");
        }
        for no in ["", "\n", "n", "ok", "yep", "sure"] {
            assert!(!is_yes(no), "{no:?}");
        }
    }

    #[test]
    fn a_path_like_stem_cannot_put_a_path_in_the_contribution() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/mike".into());
        let stem = format!("{home}/Comics/Humble/alpha_vol1");
        let tsv = contribution_tsv(&[approved(&stem, "Alpha", Some(1), None)], "B", "en").unwrap();
        assert!(!tsv.contains('/') && !tsv.contains("Comics") && !tsv.contains("Users"), "{tsv}");
    }

    /// Every `.rs` file under `dir`, recursively.
    fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                rust_sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    #[test]
    fn the_cli_crate_has_no_way_to_start_a_process() {
        // "This tool spawns nothing" is structural, so pin it over the whole
        // crate, test code included, with no cut-off a `cfg(test)` could move.
        // Needles are assembled so this test does not trip on itself. The
        // clap subcommand enum is named `Command`, so the bare token is not
        // usable; the constructor, the spawn APIs and any item of the process
        // module other than `ExitCode` are.
        let join = |p: &[&str]| p.concat();
        let banned = [
            join(&["Command", "::new"]),
            join(&["Std", "io"]),
            join(&["process", "::{"]),
            join(&["posix", "_spawn"]),
            join(&["libc", "::system"]),
            join(&["libc", "::fork"]),
            join(&["libc", "::exec"]),
        ];
        let mut files = Vec::new();
        rust_sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        assert!(files.len() >= 4, "the walk found {files:?}");
        let proc_path = join(&["process", "::"]);
        for f in files {
            let src = std::fs::read_to_string(&f).unwrap();
            for b in &banned {
                assert!(!src.contains(b.as_str()), "{} contains {b}", f.display());
            }
            for (i, _) in src.match_indices(proc_path.as_str()) {
                assert!(src[i + proc_path.len()..].starts_with("ExitCode"), "{} uses a process item other than ExitCode", f.display());
            }
        }
    }

    #[test]
    fn branches_differ_per_run_and_per_bundle_even_without_ascii_letters() {
        assert_eq!(branch_for("Humble  Manga!", 7), "map-humble-manga-7");
        assert_ne!(branch_for("Humble Manga", 7), branch_for("Humble Manga", 8));
        assert_eq!(branch_for("\u{6F2B}\u{753B}", 7), "map-bundle-7");
        assert!(contribute_in(&one_row_tsv(), &branch_for("\u{6F2B}\u{753B}", 7), tempfile::tempdir().unwrap().path()).is_ok());
    }

    #[test]
    fn the_prose_matches_the_commands_it_introduces() {
        let dir = tempfile::tempdir().unwrap();
        let c = contribute_in(&one_row_tsv(), "map-b", dir.path()).unwrap();
        // Owners skip exactly the commands that make the fresh clone.
        assert!(c.commands[0].starts_with("mkdir -p") && c.commands[1].starts_with("gh repo fork") && c.commands[2].starts_with("cd "));
        let prose = guidance(&c).join("\n");
        assert!(prose.contains("skip the first three commands"), "{prose}");
        assert!(prose.contains(&dir.path().join("map-b").join("comic-renamer").to_string_lossy().into_owned()), "{prose}");
        assert!(prose.contains("gh auth login"));
        assert!(!prose.contains("first command forks"));
        // Shared by clones of main, so ordered independently of the fork
        // steps above: the branch exists, then the rows are appended, then the
        // file that was appended to is the file staged. `mkdir -p
        // data/mappings` used to sit here to precede a `cp`; a clone that
        // ships three TSVs always has the directory, and there is no `cp`.
        let at = |needle: &str| {
            c.commands.iter().position(|l| l.starts_with(needle)).unwrap_or_else(|| panic!("{needle}: {:?}", c.commands))
        };
        assert!(at("git checkout -b") < at("tail -n +2"));
        assert!(at("tail -n +2") < at("git add"));
        let target = "data/decisions.tsv";
        assert!(c.commands[at("tail -n +2")].ends_with(target), "{:?}", c.commands);
        assert_eq!(c.commands[at("git add")], format!("git add {target}"));
    }
}
