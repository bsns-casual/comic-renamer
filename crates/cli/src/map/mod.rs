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

/// The approved entries as validated rows, plus the pairs of files that wanted
/// one key with different titles (neither of a pair is returned). Shared by the
/// local write and the contribution, so what is contributed is what was saved.
fn approved_rows(entries: &[table::Entry], bundle: &str, lang: &str) -> Result<(Vec<MappingRow>, Vec<(String, String)>), String> {
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
    let (fresh, clashes) = approved_rows(entries, bundle, lang)?;
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
    let (rows, _) = approved_rows(entries, bundle, lang)?;
    Ok(to_tsv(&rows))
}

/// Single-quotes `s` for a shell unless it is plainly safe, for the commands
/// printed as manual steps.
fn quote(s: &str) -> String {
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
    let repo = UPSTREAM.rsplit('/').next().unwrap();
    let target = format!("data/mappings/{branch}.tsv");
    let title = format!("data: {n} mapping{} from {branch}", if n == 1 { "" } else { "s" });
    let work = dir.join(branch);
    let commands = vec![
        format!("mkdir -p {0} && cd {0}", quote(&work.to_string_lossy())),
        format!("gh repo fork {UPSTREAM} --clone"),
        format!("cd {repo}"),
        format!("git checkout -b {branch}"),
        "mkdir -p data/mappings".to_string(),
        format!("cp {} {target}", quote(&path.to_string_lossy())),
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
fn branch_for(bundle: &str, secs: u64) -> String {
    let slug: String = bundle.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let slug = slug.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
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

/// The source folder's name as the bundle every approved row will carry.
/// Asked *before* the session opens: `approved_rows` validates every row
/// before writing any, so a folder name that cannot be a field used to fail
/// after the work was done and discard every correction the user had typed —
/// to an error that blamed a filename.
///
/// NFC is applied rather than refused, because the bundle is a value derived
/// from the filesystem and HFS+ hands these back decomposed. Anything
/// normalising cannot fix is the user's to fix, now, at no cost.
fn bundle_for(folder: &str) -> Result<String, String> {
    let bundle = comic_renamer_core::mapping::bundle_name(folder);
    comic_renamer_core::mapping::validate_bundle(&bundle).map_err(|e| {
        format!(
            "the source folder's name cannot be recorded with these mappings: it {e}. \
             Rename the folder, or move the files into one whose name is at most 120 bytes \
             and does not start with =, +, - or @."
        )
    })?;
    Ok(bundle)
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
    let mappings = Mappings::load(&shipped_mappings(), local.as_deref()).map_err(|e| e.to_string())?;
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
    let folder = bundle_for(&source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())?;
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
            // The mappings are already saved; a failed offer must not make the run report failure.
            if let Err(e) = offer_contribution(&t.entries, folder, lang) {
                println!("warning: the contribution was not prepared: {e}");
            }
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

    /// `bundle` is `source.file_name()`, not user input, and
    /// `MappingRow::validate` ends with it — so a folder name that cannot be a
    /// field failed *after* the session, throwing away every correction the
    /// user had just typed, with an error that blamed a filename. HFS+ stored
    /// NFD and those names survive copies, so `Café Comics` is an ordinary
    /// folder on macOS.
    #[test]
    fn an_nfd_folder_name_is_normalised_rather_than_refused() {
        let nfd = "Cafe\u{301} Comics";
        let bundle = bundle_for(nfd).unwrap();
        assert_eq!(bundle, "Caf\u{e9} Comics");
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("mappings.tsv");
        let done = commit_session(&[approved("a_vol1", "A", Some(1), None)], &local, &bundle, "en").unwrap();
        assert_eq!(done.written, 1, "the session survives the folder's own name");
        assert_eq!(read_rows(&local)[0].bundle, "Caf\u{e9} Comics");
    }

    /// A folder name normalising cannot fix is reported before the session
    /// opens, where the user has typed nothing to lose.
    #[test]
    fn a_folder_name_that_cannot_be_a_bundle_is_refused_up_front() {
        for bad in ["-Free Comics", "=Humble", "@bundle", "+extra", &"x".repeat(121)] {
            let err = bundle_for(bad).unwrap_err();
            assert!(err.contains("folder"), "{bad:?}: {err}");
            assert!(err.contains("Rename"), "{bad:?} must say what to do: {err}");
        }
        assert!(bundle_for("Humble Comics Bundle: ElfQuest").is_ok());
        assert!(bundle_for("").is_ok(), "no folder name at all is not an error");
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
    fn earlier_corrections_are_kept_and_a_repeat_replaces_its_own_row() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("mappings.tsv");
        commit_session(&[approved("old_vol1", "Old", Some(1), None)], &local, "B", "en").unwrap();
        commit_session(
            &[approved("new_vol1", "New", Some(1), None), approved("old_vol1", "Older", Some(1), None)],
            &local,
            "B",
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
        assert!(commit_session(&[approved("a", "A", Some(1), None)], &local, "B", "en").is_err());
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
        let done = commit_session(&entries, &local, "B", "en").unwrap();
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

    // ---- contribution (Task 11). The tool runs no command, so there is
    // nothing to fake: `contribute_in` returns text and writes one file.

    fn one_row_tsv() -> String {
        contribution_tsv(&[approved("alpha_vol1", "Alpha", Some(1), None)], "Humble Manga", "en").unwrap()
    }

    #[test]
    fn a_contribution_carries_no_filesystem_path() {
        let tsv = one_row_tsv();
        assert!(!tsv.contains('/') && !tsv.contains('\\'), "{tsv}");
        if let Ok(home) = std::env::var("HOME") {
            assert!(!tsv.contains(&home));
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
        assert!(c.commands.iter().any(|l| l.contains("map-b.tsv")), "{:?}", c.commands);
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
        let mk = c.commands.iter().position(|l| l == "mkdir -p data/mappings").expect("shared by clones of main");
        assert!(mk < c.commands.iter().position(|l| l.starts_with("cp ")).unwrap());
    }
}
