//! The recorded mapping from a filename to a title. A `MappingRow` is keyed by
//! `key(stem)`; the row converts to `Resolved`, so the presets stay the only
//! code that constructs paths. `series` and `subtitle` are stored verbatim
//! (`Hack/Slash: Back to School` is a real name) and the preset sanitises them
//! into path components; this module never treats a field as a path.

use crate::normalize::is_invisible;
use crate::preset::Preset;
use crate::types::{MappingError, Resolved, Status};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use unicode_normalization::UnicodeNormalization;

const HEADER: &str = "key\tseries\tvolume\tissue\tsubtitle\tlang\tvolume_count";

/// The frozen key rule: NFC, lowercase, keep alphanumerics. Every contributed
/// row was keyed by this, so changing it invalidates all of them. `None` when
/// nothing survives, so an empty key can never become a row that matches every
/// such file.
pub fn key(stem: &str) -> Option<String> {
    let k: String = stem
        .nfc()
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric())
        .collect();
    (!k.is_empty()).then_some(k)
}

#[derive(Clone, Debug, PartialEq)]
pub struct MappingRow {
    pub key: String,
    pub series: String,
    pub volume: Option<u32>,
    /// A single issue (D60). Independent of `volume`: 17 rows in the Humble
    /// table are `Love and Rockets Vol. IV #9`, which is both.
    pub issue: Option<u32>,
    pub subtitle: Option<String>,
    pub lang: String,
    pub volume_count: Option<u32>,
}

impl MappingRow {
    /// `volume_count: None` becomes run 0, which `preset::komga::pad` floors
    /// at two digits. `issue` needs no such width: D60 fixes it at four.
    pub fn resolved(&self) -> Resolved {
        Resolved {
            series: self.series.clone(),
            volume: self.volume,
            issue: self.issue,
            subtitle: self.subtitle.clone(),
            run: self.volume_count.unwrap_or(0),
            status: Status::Mapped,
            record: None,
        }
    }
}

fn num(field: &str, line: usize, name: &str) -> Result<Option<u32>, MappingError> {
    if field.is_empty() {
        return Ok(None);
    }
    field.parse().map(Some).map_err(|_| MappingError::Parse {
        line,
        reason: format!("{name} {field:?} is not a number"),
    })
}

pub fn parse_tsv(text: &str) -> Result<Vec<MappingRow>, MappingError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows = Vec::new();
    let mut seen_header = false;
    for (i, raw) in text.split('\n').enumerate() {
        let line = i + 1;
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if !seen_header {
            if raw != HEADER {
                // The pre-D64 header, named. This is the branch a real stale
                // file actually reaches: its *header* carries the eighth
                // column, so it dies here and never sees the per-row message
                // below. Two adversarial seats found that message unreachable
                // on 2026-10-30 — it was written for the one shape no install
                // has, a new header above old rows.
                let reason = if raw == format!("{HEADER}\tbundle") {
                    "this is the pre-2026-10-30 header, whose 8th column was `bundle`. \
                     Drop that column, and collapse any rows that then share a key: \
                     identity is the key alone (D64)"
                        .to_string()
                } else {
                    format!("expected the header {HEADER:?}")
                };
                return Err(MappingError::Parse { line, reason });
            }
            seen_header = true;
            continue;
        }
        // A trailing newline leaves one empty final piece.
        if raw.is_empty() {
            continue;
        }
        let f: Vec<&str> = raw.split('\t').collect();
        if f.len() != 7 {
            // Named, both counts, because the 8-column shape was this file's
            // format until 2026-10-30 and its 8th field was a `bundle`. Read
            // loosely, that bundle would land in `volume_count` and set a
            // zero-pad width from a folder name. No migration (D63), but no
            // silent misreading either.
            let was_sharded = f.len() == 8;
            return Err(MappingError::Parse {
                line,
                reason: if was_sharded {
                    "expected 7 columns, found 8 — this looks like the pre-2026-10-30 \
                     format, whose 8th column was `bundle`. Drop that column; identity \
                     is the key alone (D64)"
                        .to_string()
                } else {
                    format!("expected 7 columns, found {}", f.len())
                },
            });
        }
        rows.push(MappingRow {
            key: f[0].to_string(),
            series: f[1].to_string(),
            volume: num(f[2], line, "volume")?,
            issue: num(f[3], line, "issue")?,
            subtitle: (!f[4].is_empty()).then(|| f[4].to_string()),
            lang: f[5].to_string(),
            volume_count: num(f[6], line, "volume_count")?,
        });
    }
    if !seen_header {
        return Err(MappingError::Parse { line: 1, reason: "empty file".into() });
    }
    Ok(rows)
}

// shortcut: fields are written verbatim, so a tab or newline inside one would
// corrupt the file; the validation task (§6.11) rejects them before a row exists.
pub fn to_tsv(rows: &[MappingRow]) -> String {
    let n = |v: Option<u32>| v.map(|v| v.to_string()).unwrap_or_default();
    let mut out = format!("{HEADER}\n");
    for r in rows {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            r.key,
            r.series,
            n(r.volume),
            n(r.issue),
            r.subtitle.as_deref().unwrap_or(""),
            r.lang,
            n(r.volume_count),
        ));
    }
    out
}

const MAX_FIELD_BYTES: usize = 120;
const MAX_COMPONENT_BYTES: usize = 255;
const MAX_RENDERED_BYTES: usize = 1024;
/// `volume_count` is the zero-padding width (D44), so an unbounded one renders
/// `Vol. 0000000005`. Four digits covers every real run; the longest manga in
/// the corpus is three.
const MAX_VOLUME_COUNT: u32 = 9999;
/// D60 pads an issue to a fixed four digits, so a larger number would widen
/// the name past what the convention promises. The largest issue number in the
/// corpus is 666.
pub const MAX_ISSUE: u32 = 9999;

fn invalid(field: &'static str, reason: impl Into<String>) -> MappingError {
    MappingError::Invalid { field, reason: reason.into() }
}

// shortcut: only the Latin/Cyrillic/Greek triple is checked, which is the
// look-alike attack §6.11 names; Han/Kana/Hangul/Arabic/Hebrew/Thai mix with
// Latin legitimately. Widen with a script table if another pair is abused.
fn mixes_confusable_scripts(s: &str) -> bool {
    let mut seen = [false; 3]; // Latin, Greek, Cyrillic
    for c in s.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '\u{C0}'..='\u{24F}' | '\u{1E00}'..='\u{1EFF}'
                if c != '\u{D7}' && c != '\u{F7}' =>
            {
                seen[0] = true
            }
            '\u{370}'..='\u{3FF}' | '\u{1F00}'..='\u{1FFF}' => seen[1] = true,
            '\u{400}'..='\u{52F}' | '\u{1C80}'..='\u{1C8F}' | '\u{2DE0}'..='\u{2DFF}'
            | '\u{A640}'..='\u{A69F}' => seen[2] = true,
            _ => {}
        }
    }
    seen.iter().filter(|&&b| b).count() > 1
}

/// Rejects only what the preset's sanitiser cannot repair. `: / \ | " < > ? *`
/// pass: `sanitize` maps them, and 26.8% of real titles carry one (D47).
fn validate_field(field: &'static str, s: &str) -> Result<(), MappingError> {
    if s.len() > MAX_FIELD_BYTES {
        return Err(invalid(field, format!("is {} bytes; the cap is {MAX_FIELD_BYTES}", s.len())));
    }
    if s.chars().any(char::is_control) {
        return Err(invalid(field, "contains a control character"));
    }
    // Invisible to a reviewer, so rejected rather than sanitised. The same
    // predicate the preset strips with and the grader cleans with: three
    // copies at three widths meant the narrowest let through what the widest
    // refused.
    if s.chars().any(is_invisible) {
        return Err(invalid(field, "contains an invisible character"));
    }
    if !s.nfc().eq(s.chars()) {
        return Err(invalid(field, "is not NFC-normalised"));
    }
    if s.starts_with(['=', '+', '-', '@']) {
        return Err(invalid(field, "starts with a spreadsheet formula character"));
    }
    if !s.is_empty() && s.chars().all(|c| c == '.') {
        return Err(invalid(field, "is only dots"));
    }
    if mixes_confusable_scripts(s) {
        return Err(invalid(field, "mixes more than one of Latin, Cyrillic and Greek"));
    }
    Ok(())
}

impl MappingRow {
    pub fn validate(&self) -> Result<(), MappingError> {
        // A key `key()` would not produce never matches a stem: a dead row.
        if key(&self.key).as_deref() != Some(self.key.as_str()) {
            return Err(invalid("key", "is not what key() produces from itself"));
        }
        // `sanitize("")` is `_`, which would file the comic under a folder of that name.
        if self.series.trim().is_empty() {
            return Err(invalid("series", "is empty or only whitespace"));
        }
        validate_field("series", &self.series)?;
        if let Some(sub) = &self.subtitle {
            validate_field("subtitle", sub)?;
        }
        validate_field("lang", &self.lang)?;
        // No cross-field rule against `volume`: a row may be both (D60).
        if let Some(issue) = self.issue {
            if issue > MAX_ISSUE {
                return Err(invalid(
                    "issue",
                    format!("is {issue}; the ceiling is {MAX_ISSUE}"),
                ));
            }
        }
        if let Some(count) = self.volume_count {
            if count > MAX_VOLUME_COUNT {
                return Err(invalid(
                    "volume_count",
                    format!("is {count}; the ceiling is {MAX_VOLUME_COUNT}"),
                ));
            }
            // Two rows of one series with different counts pad differently
            // inside one folder, and a volume past the end of the run means
            // `Vol. 100` beside `Vol. 05`, which sorts wrongly everywhere but
            // Komga. The resolver already treats feasibility as a hard
            // constraint, so no corpus candidate disagrees with itself here.
            if self.volume.is_some_and(|v| v > count) {
                return Err(invalid(
                    "volume",
                    format!("is {}, past the end of a run of {count}", self.volume.unwrap_or(0)),
                ));
            }
        }
        Ok(())
    }
}

/// A source folder's name, NFC-normalised. HFS+ stored NFD and those names
/// survive copies, so `Café Comics` arrives decomposed.
///
/// No longer a stored field (D64) — the shipped table has no `bundle` column.
/// It remains the key Layer B's overrides are looked up by, and the stem
/// `branch_for` slugifies, both of which are genuinely about *this folder on
/// this disk* rather than about a row's identity.
pub fn bundle_name(folder: &str) -> String {
    folder.nfc().collect()
}

/// Whether the destination the preset renders from this row fits 255 bytes
/// per component and 1024 total. Measures the row-controlled part only: CI
/// cannot know `dest_root`.
///
/// Currently unreachable for Komga: `sanitize` truncates every component at
/// 255 bytes, so a validated row cannot exceed either cap. Keep it anyway
/// (§7.4): Phase 2 presets have other path shapes (Kavita repeats the series
/// inside the filename), and the tests exercise it with presets that don't
/// truncate. Not dead code.
pub fn rendered_budget_ok(row: &MappingRow, preset: &dyn Preset) -> bool {
    let dest = preset.destination(&row.resolved(), "cbz");
    let parts: Vec<usize> = dest.components().map(|c| c.as_os_str().len()).collect();
    let total = parts.iter().sum::<usize>() + parts.len().saturating_sub(1);
    parts.iter().all(|&n| n <= MAX_COMPONENT_BYTES) && total <= MAX_RENDERED_BYTES
}

/// Parse and validate one layer, naming the first bad line. `origin` says which
/// layer, because "line 7" alone is ambiguous across two files.
fn load_layer(text: &str, origin: &str) -> Result<Vec<MappingRow>, MappingError> {
    let at = |line: usize, reason: String| MappingError::Parse {
        line,
        reason: format!("{origin}: {reason}"),
    };
    let rows = parse_tsv(text).map_err(|e| match e {
        MappingError::Parse { line, reason } => at(line, reason),
        other => other,
    })?;
    // parse_tsv keeps no line numbers; data rows are the non-empty lines after the header.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<usize> = text
        .split('\n')
        .enumerate()
        .skip(1)
        .filter(|(_, l)| !l.strip_suffix('\r').unwrap_or(l).is_empty())
        .map(|(i, _)| i + 1)
        .collect();
    for (row, line) in rows.iter().zip(&lines) {
        row.validate().map_err(|e| at(*line, e.to_string()))?;
    }
    // CI runs this over shipped data; running it at load is what makes it true
    // of a file nobody checked. `lookup` takes the first row for a key, so a
    // second one is ignored in silence — and in the user's own file that second
    // row is their later correction.
    if let Some(key) = find_key_collisions(&rows).into_iter().next() {
        // The row that would be ignored, so the user is sent to the one they
        // most likely meant to keep.
        let ignored = rows
            .iter()
            .zip(&lines)
            .filter(|(r, _)| r.key == key)
            .nth(1)
            .map_or(0, |(_, line)| *line);
        return Err(at(
            ignored,
            format!("a second row for key {key:?}; identity is the key alone, so correct the first row rather than adding another"),
        ));
    }
    Ok(rows)
}

fn group(rows: Vec<MappingRow>) -> HashMap<String, Vec<MappingRow>> {
    let mut by_key: HashMap<String, Vec<MappingRow>> = HashMap::new();
    for row in rows {
        by_key.entry(row.key.clone()).or_default().push(row);
    }
    by_key
}

/// Shipped rows plus the user's own, keyed by `key` and nothing else (D64).
pub struct Mappings {
    by_key: HashMap<String, Vec<MappingRow>>,
}

impl Mappings {
    /// `local: None` means there is no local file. A `Some` path that cannot be
    /// read is an error: it holds the user's own corrections, and dropping them
    /// quietly would look like the corrections never worked.
    pub fn load(shipped: &str, local: Option<&Path>) -> Result<Mappings, MappingError> {
        let mut by_key = group(load_layer(shipped, "shipped mappings")?);
        if let Some(path) = local {
            let text = std::fs::read_to_string(path).map_err(|e| MappingError::Unreadable {
                path: path.display().to_string(),
                reason: e.to_string(),
            })?;
            // A local key replaces every shipped row for it. Merging instead would
            // leave two rows, and a bundle matching neither would drop the correction.
            by_key.extend(group(load_layer(&text, &format!("local mappings {}", path.display()))?));
        }
        Ok(Mappings { by_key })
    }

    /// The decision for a key, if the table holds one.
    ///
    /// One argument since D64. There used to be a second, a bundle, because a
    /// key could carry several rows and the caller's folder name chose between
    /// them — which meant 158 of 2,043 keys resolved only for whoever's folder
    /// matched, and for nobody else. `load_layer` refuses a duplicate key, so
    /// the slice is always one row.
    pub fn lookup(&self, key: &str) -> Option<&MappingRow> {
        self.by_key.get(key)?.first()
    }

    pub fn len(&self) -> usize {
        self.by_key.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }
}


/// Keys held by more than one row. Under one row per key (D64) a repeat is a
/// mistake whatever its fields say, so this no longer compares them: an exact
/// duplicate is as wrong as a disagreement, because a correction is an edit to
/// the existing line and never a second line.
pub fn find_key_collisions(rows: &[MappingRow]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut bad = BTreeSet::new();
    for r in rows {
        if !seen.insert(&r.key) {
            bad.insert(r.key.clone());
        }
    }
    bad.into_iter().collect()
}


/// The one shipped table (D63), `include_str!`-embedded so an installed binary
/// needs nothing from the working directory.
///
/// One file rather than one per marketplace, because the sharding was a fiction
/// the code never believed: `find_key_collisions` always ran over the joined
/// table and `lookup` keyed on a row's bundle rather than its file. A
/// contribution appends here and the build picks the rows up with no code
/// change in the pull request (D66).
pub fn shipped_mappings() -> &'static str {
    const SHIPPED: &str = include_str!("../../../data/decisions.tsv");
    SHIPPED.strip_prefix('\u{feff}').unwrap_or(SHIPPED)
}
