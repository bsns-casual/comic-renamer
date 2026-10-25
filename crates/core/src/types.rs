//! Every shared type and error. One file, so they cannot drift (see the plan's
//! note on why the skeleton exists).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Move,
    Copy,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mechanism {
    Rename,
    Clone,
    Stream,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Found in the mapping.
    Mapped,
    Unmapped,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeriesRecord {
    pub name: String,
    pub publisher: String,
    pub language: String,
    pub year_began: u16,
    pub run: u32,
}

/// A proposed answer for a file nobody has mapped yet. Never stored without a
/// keystroke (D49): a picker shows these, a human confirms or types over them.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub series: String,
    pub volume: Option<u32>,
    pub subtitle: Option<String>,
    /// The corpus's `issue_count` for this record; the preset's `run`.
    pub volume_count: Option<u32>,
    pub source: SuggestionSource,
}

impl Suggestion {
    /// A corpus hit that carries a subtitle is raw stem text the resolver could
    /// not split (`seedofdestruction`, D15): the series is right and the
    /// subtitle is garbled. The one definition; the picker's guard and the
    /// resolver's pin both call it.
    pub fn is_unsplit(&self) -> bool {
        self.source == SuggestionSource::Corpus && self.subtitle.is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SuggestionSource {
    /// The GCD corpus.
    Corpus,
    /// A Layer B override: the one answer a human already verified.
    Override,
    /// The filename itself, title-cased and parsed.
    Filename,
}

/// Not `Serialize`: a plan row carries these fields flattened, so a plan can
/// never be deserialised straight into a resolver result.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub series: String,
    pub volume: Option<u32>,
    pub subtitle: Option<String>,
    pub run: u32,
    pub status: Status,
    pub record: Option<SeriesRecord>,
}

/// How `apply` detects a source that changed since `scan`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceStat {
    pub size: u64,
    pub mtime_ns: i64,
    pub inode: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Collision {
    pub key: String,
    pub row_ids: Vec<String>,
    pub existing: Option<PathBuf>,
}

/// The publisher hint, derived from the source folder's own name (§6.3).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PublisherHint(pub Option<String>);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanHeader {
    pub version: u32,
    pub preset: String,
    pub mode: Mode,
    pub source_root: PathBuf,
    pub dest_root: PathBuf,
    pub lang: String,
    /// Logical bytes the destination must accommodate (`st_size`, never `du`).
    pub required_bytes: u64,
    /// Bytes a dataless source would pull down, reported apart from disk (D23).
    pub download_bytes: u64,
    /// Files `scan` found but the mapping did not know. They are not rows, so
    /// `apply` could not otherwise tell the user they stay where they are.
    pub unmapped: u64,
    /// Covers the header and every row, so `apply` can tell what a human edited.
    pub checksum: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanRow {
    pub id: String,
    pub source: PathBuf,
    /// Preview only. `apply` recomputes through the preset and never trusts
    /// this string (D14).
    pub destination_preview: PathBuf,
    pub extension: String,
    pub series: String,
    pub volume: Option<u32>,
    pub subtitle: Option<String>,
    pub run: u32,
    pub status: Status,
    pub mechanism: Mechanism,
    pub source_stat: SourceStat,
    pub record: Option<SeriesRecord>,
}

impl PlanRow {
    /// Rebuilds the resolver result so `apply` can recompute the destination.
    /// D14: the recomputation reads these fields, never `destination_preview`.
    pub fn resolved(&self) -> Resolved {
        Resolved {
            series: self.series.clone(),
            volume: self.volume,
            subtitle: self.subtitle.clone(),
            run: self.run,
            status: self.status.clone(),
            record: self.record.clone(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ScanOpts {
    pub source_root: PathBuf,
    pub dest_root: PathBuf,
    pub preset: String,
    pub mode: Option<Mode>,
    pub lang: String,
}

#[derive(Clone, Debug)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
    pub current: String,
}

/// What `reverse` actually did. It used to return `()`, so a run that
/// restored nothing reported success — including when a permission error
/// made `Path::exists()` answer `false`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UndoReport {
    pub restored: usize,
    /// Already reversed, or never executed. Nothing to do.
    pub skipped: usize,
    /// Left alone and why: the destination is not what the manifest recorded,
    /// the original path is occupied, or the row could not be examined.
    pub refused: Vec<String>,
    /// Where this reverse put the library files it was authorised to remove,
    /// and how many are waiting there (D58). `None` when it moved none.
    pub aside: Option<Aside>,
}

/// The directory a reverse moved library files into instead of unlinking
/// them, and how many files it holds (D58).
///
/// One value rather than two fields on the report, so a path cannot be
/// reported over a count of zero. **Nothing ever empties this directory** —
/// removing the files is a keystroke the user makes, because an automatic
/// purge would put back the destructive path D58 removed.
#[derive(Clone, Debug, PartialEq)]
pub struct Aside {
    /// Absolute, under the library root: `<dest_root>/.comic-renamer-aside/<run-id>`.
    pub directory: PathBuf,
    /// What the directory holds, not what this pass put there — a reverse that
    /// refused some rows and was run again adds to a directory the earlier
    /// pass started, and the number the user needs is how many are waiting. A
    /// directory that cannot be read falls back to this pass's own count,
    /// which is a true lower bound.
    pub files: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApplyResult {
    pub executed: usize,
    pub skipped: usize,
    pub failed: usize,
    /// The record this run wrote, or `None` when it placed nothing. The record
    /// is created lazily on the first row that executes, so a run that refused
    /// every row has nothing to reverse and nothing to name.
    pub record: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KomgaLibrary {
    pub name: String,
    pub hash_files: bool,
    pub empty_trash_after_scan: bool,
}

/// Picks a unit. `{:.1} GiB` alone prints "0.0 GiB" for a real payload, and a
/// refusal that reports zero bytes is worse than one that reports an awkward
/// unit. Lives here because the error messages need it as much as the CLI does.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [(u64, &str); 3] = [(1 << 30, "GiB"), (1 << 20, "MiB"), (1 << 10, "KiB")];
    for (scale, unit) in UNITS {
        if bytes >= scale {
            return format!("{:.1} {unit}", bytes as f64 / scale as f64);
        }
    }
    format!("{bytes} bytes")
}

// ---------------------------------------------------------------------------
// Errors. One enum per module, all declared here so later tasks cannot invent
// a variant that another module's test already names.
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum MappingError {
    #[error("mapping file line {line}: {reason}")]
    Parse { line: usize, reason: String },
    #[error("{field} {reason}")]
    Invalid { field: &'static str, reason: String },
    #[error("local mapping file {path} is unreadable: {reason}")]
    Unreadable { path: String, reason: String },
}

#[derive(Debug, thiserror::Error)]
pub enum CorpusError {
    #[error("corpus sha256 is {actual}, manifest pins {expected}")]
    HashMismatch { expected: String, actual: String },
    #[error("corpus is {actual} bytes, manifest pins {expected}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("corpus holds {actual} series, manifest pins {expected}")]
    CountMismatch { expected: u64, actual: u64 },
    #[error("corpus schema version is {actual}, this build needs {expected}")]
    SchemaVersion { expected: u32, actual: u32 },
    #[error("corpus licence {0:?} is not share-alike")]
    Licence(String),
    #[error("malformed manifest: {0}")]
    Manifest(String),
    #[error("corpus archive is unusable: {0}")]
    Archive(String),
    #[error("fetching the corpus failed: {0}")]
    Fetch(String),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum OverrideError {
    #[error("field {field} holds a forbidden value {value:?}")]
    Forbidden { field: String, value: String },
    #[error("entry {0:?} has no bundle discriminator")]
    MissingBundle(String),
    #[error("duplicate override key {0:?}")]
    DuplicateKey(String),
    #[error("field {field} is {len} bytes, the cap is 120")]
    TooLong { field: String, len: usize },
    #[error("malformed overrides: {0}")]
    Parse(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("duplicate row id {0:?}")]
    DuplicateRowId(String),
    #[error("destination {0} escapes the plan's dest_root")]
    OutsideDestRoot(PathBuf),
    #[error("plan checksum is {actual}, header claims {expected}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("line {line}: {message}")]
    Parse { line: usize, message: String },
    #[error("plan is version {found}, this build reads version {expected}; run scan again")]
    UnsupportedVersion { found: u32, expected: u32 },
    #[error("plan has no header")]
    MissingHeader,
    #[error("unknown preset {0:?}")]
    UnknownPreset(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("source and destination roots contain one another")]
    RootsOverlap,
    #[error("{0} is not a directory")]
    NotADirectory(PathBuf),
    #[error("unknown preset {0:?}")]
    UnknownPreset(String),
    #[error(transparent)]
    Corpus(#[from] CorpusError),
    #[error(transparent)]
    Overrides(#[from] OverrideError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("another run holds the lock")]
    Locked,
    /// The record store is unusable, so the run cannot be recorded — which
    /// means it does not happen. Raised before the first row.
    #[error("the record store is unusable: {0}")]
    Records(String),
    /// The lock held is some other library's, so this one is not locked at
    /// all. A caller bug, caught before the first row rather than trusted.
    #[error("the lock held is for {locked}, but this run writes to {planned}")]
    WrongLibrary { locked: PathBuf, planned: PathBuf },
    #[error("source {0} changed since the scan")]
    SourceChanged(PathBuf),
    /// A row's source was there at the pre-flight and is gone by the time the
    /// row runs. Reachable without any race: a hand-edited plan can name one
    /// file twice, and two runs on one library under two `XDG_CONFIG_HOME`s
    /// are not serialised, because the lock lives in the record store. Both
    /// used to arrive as a bare `No such file or directory` naming neither the
    /// row nor the path.
    #[error("row {0}: source {1} is gone; it was there at the pre-flight, so something else took it")]
    SourceGone(String, PathBuf),
    #[error("need {}, only {} available after the reserve", human_bytes(*required), human_bytes(*available))]
    InsufficientSpace { required: u64, available: u64 },
    #[error("row {0}: stored destination does not match the recomputed one")]
    Tampered(String),
    #[error("row {0}: the chosen mechanism is no longer valid")]
    MechanismChanged(String),
    #[error("row {0}: source {1} is not directly inside the plan's source_root")]
    OutsideSourceRoot(String, PathBuf),
    #[error("source and destination roots contain one another")]
    RootsOverlap,
    #[error("{0} is cloud-synced; a move needs an explicit override")]
    CloudSourceMove(PathBuf),
    #[error("aborted at the prompt")]
    Aborted,
    #[error("unknown preset {0:?}")]
    UnknownPreset(String),
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error(transparent)]
    Undo(#[from] UndoError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum UndoError {
    #[error("{0} is occupied; refusing to reverse onto it")]
    Reoccupied(PathBuf),
    #[error("another run holds the undo lock")]
    Locked,
    #[error("manifest records no destination root")]
    NoRoot,
    #[error("the source and destination folders overlap; one sits inside the other")]
    RootsOverlap,
    #[error("{0} is outside the manifest's destination root")]
    OutsideRoot(PathBuf),
    #[error("line {line}: {message}")]
    Parse { line: usize, message: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum KomgaError {
    #[error("library {name:?} has hashFiles={hash_files} and emptyTrashAfterScan={empty_trash}")]
    Unsafe {
        name: String,
        hash_files: bool,
        empty_trash: bool,
    },
    #[error("COMIC_RENAMER_KOMGA_TOKEN is not set")]
    MissingToken,
    #[error("Komga request failed: {0}")]
    Http(String),
}
