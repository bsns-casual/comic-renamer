//! Layer B: the only place a wrong *title* is correctable, and the floor under
//! the unmatched tail (§6.2.1). Rule data never holds anything path-shaped
//! (invariant 1).
//!
//! This is data contributed by strangers, so §6.11's forbid list is the bulk of
//! the module. A rejected rule costs a contributor a round trip; an accepted
//! one that carries `../..` is an arbitrary-write primitive.

use crate::normalize::{nfc, squash};
use crate::types::OverrideError;
use std::collections::BTreeMap;
use std::path::Path;

/// §6.11, per contributed field.
const MAX_FIELD_BYTES: usize = 120;

/// §6.11. Matched case-insensitively, with or without an extension.
const DEVICE_NAMES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverrideEntry {
    pub series: String,
    pub volume: Option<u32>,
    pub subtitle: Option<String>,
    /// Required: a bare filename key recurs across publishers. Matched by
    /// `squash(bundle)` being a substring of `squash(source_folder_name)`.
    ///
    /// `default` so a missing one becomes `MissingBundle`, which names the
    /// entry, rather than a serde message about a missing field.
    #[serde(default)]
    pub bundle: String,
}

#[derive(Clone, Debug, Default)]
pub struct Overrides {
    /// `(squashed filename key, squashed bundle, entry)`.
    entries: Vec<(String, String, OverrideEntry)>,
}

impl Overrides {
    pub fn load(path: &Path) -> Result<Overrides, OverrideError> {
        let text = std::fs::read_to_string(path)?;
        // The toml crate rejects a duplicate key inside one table itself, so
        // DuplicateKey here covers two spellings that squash to one key.
        let raw: BTreeMap<String, OverrideEntry> =
            toml::from_str(&text).map_err(|e| OverrideError::Parse(e.to_string()))?;

        let mut entries: Vec<(String, String, OverrideEntry)> = Vec::with_capacity(raw.len());
        for (key, entry) in raw {
            validate("key", &key)?;
            validate("series", &entry.series)?;
            if let Some(subtitle) = &entry.subtitle {
                validate("subtitle", subtitle)?;
            }
            if entry.bundle.trim().is_empty() {
                return Err(OverrideError::MissingBundle(key));
            }
            validate("bundle", &entry.bundle)?;

            let squashed = squash(&key);
            if entries.iter().any(|(k, _, _)| *k == squashed) {
                return Err(OverrideError::DuplicateKey(key));
            }
            let bundle = squash(&entry.bundle);
            entries.push((squashed, bundle, entry));
        }
        Ok(Overrides { entries })
    }

    pub fn lookup(&self, filename: &str, source_folder: &str) -> Option<&OverrideEntry> {
        // shortcut: linear scan over a hand-curated file of tens of rows.
        // Index by squashed key if Layer B ever reaches the thousands.
        let key = squash(filename);
        let folder = squash(source_folder);
        self.entries
            .iter()
            .find(|(k, bundle, _)| *k == key && folder.contains(bundle.as_str()))
            .map(|(_, _, entry)| entry)
    }
}

/// §6.11's forbid list. Governs contributed rule data only — corpus titles are
/// sanitised by the preset instead, because `Nemesis: Rogues' Gallery` is a
/// correct title and a hostile path.
fn validate(field: &str, value: &str) -> Result<(), OverrideError> {
    let forbidden = |value: &str| OverrideError::Forbidden {
        field: field.to_string(),
        value: value.to_string(),
    };

    if value.len() > MAX_FIELD_BYTES {
        return Err(OverrideError::TooLong {
            field: field.to_string(),
            len: value.len(),
        });
    }
    if value.is_empty() {
        return Err(forbidden(value));
    }
    if value != nfc(value) {
        return Err(forbidden(value));
    }
    if value.contains(['/', '\\', '<', '>', ':', '"', '|', '?', '*']) {
        return Err(forbidden(value));
    }
    if value == "." || value == ".." || value.starts_with('.') {
        return Err(forbidden(value));
    }
    if value.starts_with(['=', '+', '-', '@']) {
        return Err(forbidden(value));
    }
    if value.ends_with('.') || value.ends_with(' ') {
        return Err(forbidden(value));
    }
    let stem = value.split('.').next().unwrap_or(value);
    if DEVICE_NAMES.contains(&stem.to_ascii_lowercase().as_str()) {
        return Err(forbidden(value));
    }
    if value.chars().any(is_invisible_or_control) {
        return Err(forbidden(value));
    }
    if mixes_scripts(value) {
        return Err(forbidden(value));
    }
    Ok(())
}

fn is_invisible_or_control(c: char) -> bool {
    matches!(c,
        '\u{0}'..='\u{1f}'          // C0, which covers tab, CR and LF
        | '\u{7f}'..='\u{9f}'       // DEL and C1
        | '\u{ad}'                  // soft hyphen
        | '\u{200b}'..='\u{200f}'   // zero-width and directional marks
        | '\u{202a}'..='\u{202e}'   // bidi embedding and override
        | '\u{2066}'..='\u{2069}'   // bidi isolates
        | '\u{feff}')               // BOM / zero-width no-break space
}

/// §6.11: a Cyrillic `е` among Latin letters creates a duplicate series that
/// is invisible in a diff. One script per field, or none.
fn mixes_scripts(value: &str) -> bool {
    #[derive(PartialEq, Clone, Copy)]
    enum Script {
        Latin,
        Greek,
        Cyrillic,
    }
    let mut seen: Option<Script> = None;
    for c in value.chars() {
        let script = match c {
            'a'..='z' | 'A'..='Z' | '\u{c0}'..='\u{24f}' => Some(Script::Latin),
            '\u{370}'..='\u{3ff}' | '\u{1f00}'..='\u{1fff}' => Some(Script::Greek),
            '\u{400}'..='\u{52f}' => Some(Script::Cyrillic),
            _ => None,
        };
        match (script, seen) {
            (Some(s), None) => seen = Some(s),
            (Some(s), Some(prev)) if s != prev => return true,
            _ => {}
        }
    }
    false
}
