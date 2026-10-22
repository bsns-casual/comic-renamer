//! Read-only by construction. Pre-flight order is fixed by §6.10.
//!
//! `scan` stats; it never opens. Resolution is filename-only (invariant 5),
//! but the metadata is load-bearing: `st_size` for capacity, `SF_DATALESS`
//! for download cost, and `(size, mtime_ns, inode)` so `apply` can refuse a
//! source that changed since the plan was written.

use crate::corpus::Corpus;
use crate::fsops::{is_dataless, select_mechanism};
use crate::overrides::Overrides;
use crate::plan::checksum;
use crate::preset::preset_by_id;
use crate::resolve::{publisher_hint, resolve};
use crate::types::{Mode, PlanHeader, PlanRow, Progress, ScanError, ScanOpts, SourceStat};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// §6.10. Anything else in the source is skipped silently, not reported as a
/// failure.
const COMIC_EXTENSIONS: [&str; 6] = ["pdf", "cbz", "cbr", "epub", "cb7", "cbt"];

/// Ancillary files distributors ship beside the books.
const ANCILLARY_PREFIXES: [&str; 7] = [
    "id", "readme", "license", "licence", "credits", "cover", "thumbs",
];

/// The plan format this build writes.
const PLAN_VERSION: u32 = 1;

/// `realpath` both roots and refuse if either contains the other; same-volume
/// via `stat().dev`; sum **logical** sizes (`st_size`, never `du`); flag
/// dataless sources and total `download_bytes`; record a per-row mechanism and
/// `SourceStat`.
///
/// Skips silently: extensions outside `pdf|cbz|cbr|epub|cb7|cbt`, dotfiles,
/// `^(id|readme|licen[cs]e|credits|cover|thumbs)[-_.]`, and anything that is
/// not a regular file — exploded bundles ship directories named `Hellboy.pdf`.
///
/// The corpus and overrides are passed in rather than loaded here: loading
/// means a network fetch on first run, and this function is the one the plan
/// calls read-only by construction. The CLI owns the loading.
// shortcut: non-recursive; nested bundles need a decision on multi-format
// duplicates (open question 11.4)
pub fn scan(
    opts: &ScanOpts,
    corpus: &Corpus,
    overrides: &Overrides,
    progress: &mut dyn FnMut(Progress),
) -> Result<(PlanHeader, Vec<PlanRow>), ScanError> {
    let source_root = directory(&opts.source_root)?;
    let dest_root = directory(&opts.dest_root)?;

    // Refuse before anything else: a scan whose destination sits inside its
    // source re-ingests its own output (§6.10).
    if source_root.starts_with(&dest_root) || dest_root.starts_with(&source_root) {
        return Err(ScanError::RootsOverlap);
    }

    let preset =
        preset_by_id(&opts.preset).ok_or_else(|| ScanError::UnknownPreset(opts.preset.clone()))?;
    let mode = opts.mode.unwrap_or(Mode::Move);

    // §6.3: the hint is the source folder's own name, and it is absent for
    // anyone running against ~/Downloads — which is the common case.
    let folder = source_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let hint = publisher_hint(&folder, corpus);

    let mut candidates: Vec<PathBuf> = std::fs::read_dir(&source_root)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| is_comic(path))
        .collect();
    // read_dir order is unspecified; a plan a human reads should be stable.
    candidates.sort();

    let total = candidates.len() as u64;
    let mut rows = Vec::with_capacity(candidates.len());
    let mut required_bytes = 0u64;
    let mut download_bytes = 0u64;

    for (index, path) in candidates.iter().enumerate() {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        progress(Progress {
            done: index as u64,
            total,
            current: name.to_string(),
        });

        let meta = std::fs::symlink_metadata(path)?;
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let extension = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();

        let resolved = resolve(&stem, &folder, &hint, corpus, overrides, &opts.lang);

        // Logical, never `du`: it reports neither cloud-placeholder nor clone
        // reality (§6.10).
        required_bytes += meta.len();
        // D23: a dataless source transfers its bytes whatever mechanism is
        // chosen, so the download cost is reported apart from the disk cost.
        if is_dataless(path)? {
            download_bytes += meta.len();
        }

        rows.push(PlanRow {
            id: format!("r{:05}", index + 1),
            source: path.clone(),
            // A row that resolved to nothing gets no destination. The
            // preset would otherwise emit `_/_.pdf` for every unresolved row
            // in the run, which converges them all onto one path and shows
            // the user a destination for a file that resolved to nothing.
            // An unsplit subtitle still has a series, so it keeps its preview.
            destination_preview: if resolved.series.is_empty() {
                PathBuf::new()
            } else {
                preset.destination(&resolved, &extension)
            },
            extension,
            series: resolved.series.clone(),
            volume: resolved.volume,
            subtitle: resolved.subtitle.clone(),
            run: resolved.run,
            status: resolved.status.clone(),
            mechanism: select_mechanism(path, &dest_root, mode)?,
            source_stat: SourceStat {
                size: meta.len(),
                mtime_ns: meta.mtime() * 1_000_000_000 + meta.mtime_nsec(),
                inode: meta.ino(),
            },
            record: resolved.record.clone(),
        });
    }
    progress(Progress {
        done: total,
        total,
        current: String::new(),
    });

    let mut header = PlanHeader {
        version: PLAN_VERSION,
        preset: opts.preset.clone(),
        mode,
        source_root,
        dest_root,
        lang: opts.lang.clone(),
        required_bytes,
        download_bytes,
        checksum: String::new(),
    };
    header.checksum = checksum(&header, &rows);
    Ok((header, rows))
}

/// `realpath`, and it has to be a directory. Resolving first is what makes the
/// overlap check meaningful — a string comparison misses a symlinked root.
fn directory(path: &Path) -> Result<PathBuf, ScanError> {
    // Named, not bare: `canonicalize` reports which *syscall* failed and not
    // which of the two roots, and a user who has not created the destination
    // yet got "No such file or directory" with nothing to act on.
    let resolved = std::fs::canonicalize(path).map_err(|e| {
        ScanError::Io(std::io::Error::new(e.kind(), format!("{}: {e}", path.display())))
    })?;
    if !resolved.is_dir() {
        return Err(ScanError::NotADirectory(path.to_path_buf()));
    }
    Ok(resolved)
}

fn is_comic(path: &Path) -> bool {
    // Not a regular file: exploded bundles ship directories named
    // `Hellboy.pdf`, and a symlink is not ours to follow.
    if !std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file()) {
        return false;
    }
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_lowercase()) else {
        return false;
    };
    if name.starts_with('.') {
        return false;
    }
    if ANCILLARY_PREFIXES.iter().any(|prefix| {
        name.strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with(['-', '_', '.']))
    }) {
        return false;
    }
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .is_some_and(|e| COMIC_EXTENSIONS.contains(&e.as_str()))
}
