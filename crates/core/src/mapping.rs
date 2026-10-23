//! The recorded mapping from a filename to a title. A `MappingRow` is keyed by
//! `key(stem)`; the row converts to `Resolved`, so the presets stay the only
//! code that constructs paths. `series` and `subtitle` are stored verbatim
//! (`Hack/Slash: Back to School` is a real name) and the preset sanitises them
//! into path components; this module never treats a field as a path.

use crate::types::{MappingError, Resolved, Status};
use unicode_normalization::UnicodeNormalization;

const HEADER: &str = "key\tseries\tvolume\tsubtitle\tlang\tvolume_count\tbundle";

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
    pub subtitle: Option<String>,
    pub lang: String,
    pub volume_count: Option<u32>,
    pub bundle: String,
}

impl MappingRow {
    /// `volume_count: None` becomes run 0, which `preset::komga::pad` floors
    /// at two digits.
    pub fn resolved(&self) -> Resolved {
        Resolved {
            series: self.series.clone(),
            volume: self.volume,
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
                return Err(MappingError::Parse {
                    line,
                    reason: format!("expected the header {HEADER:?}"),
                });
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
            return Err(MappingError::Parse {
                line,
                reason: format!("expected 7 columns, found {}", f.len()),
            });
        }
        rows.push(MappingRow {
            key: f[0].to_string(),
            series: f[1].to_string(),
            volume: num(f[2], line, "volume")?,
            subtitle: (!f[3].is_empty()).then(|| f[3].to_string()),
            lang: f[4].to_string(),
            volume_count: num(f[5], line, "volume_count")?,
            bundle: f[6].to_string(),
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
            r.subtitle.as_deref().unwrap_or(""),
            r.lang,
            n(r.volume_count),
            r.bundle
        ));
    }
    out
}
