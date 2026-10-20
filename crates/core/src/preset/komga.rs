//! Flat `<Series>/<Book>.<ext>` — Komga reads only the leaf folder (§6.8.1).

use super::Preset;
use crate::normalize::nfc;
use crate::types::Resolved;
use std::path::PathBuf;

/// A path component's byte budget on every filesystem this targets.
const MAX_COMPONENT_BYTES: usize = 255;

/// §6.11's device names, which are reserved on Windows with or without an
/// extension.
const DEVICE_NAMES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Characters that are illegal in a path, each mapped to a *distinct*
/// look-alike. Mapping `:` onto ` - ` would make `Hellboy: Seed` and
/// `Hellboy - Seed` one folder, which silently merges two series — so every
/// substitute has to be a character nothing else produces.
const SUBSTITUTIONS: [(char, char); 9] = [
    (':', '\u{a789}'),  // ꞉ modifier letter colon
    ('/', '\u{2044}'),  // ⁄ fraction slash
    ('\\', '\u{29f5}'), // ⧵ reverse solidus operator
    ('?', '\u{ff1f}'),  // ？
    ('*', '\u{ff0a}'),  // ＊
    ('"', '\u{ff02}'),  // ＂
    ('<', '\u{ff1c}'),  // ＜
    ('>', '\u{ff1e}'),  // ＞
    ('|', '\u{ff5c}'),  // ｜
];

pub struct Komga;

impl Preset for Komga {
    fn id(&self) -> &'static str {
        "komga"
    }

    fn destination(&self, resolved: &Resolved, ext: &str) -> PathBuf {
        let series = sanitize(&resolved.series);

        // Volume before subtitle: Komga assigns number and numberSort
        // positionally after a natural sort, so filename order is reading
        // order (§6.8.1).
        let mut stem = resolved.series.clone();
        if let Some(volume) = resolved.volume {
            stem.push_str(&format!(" Vol. {}", pad(volume, resolved.run)));
        }
        if let Some(subtitle) = &resolved.subtitle {
            stem.push_str(" - ");
            stem.push_str(subtitle);
        }

        let suffix = if ext.is_empty() {
            String::new()
        } else {
            format!(".{ext}")
        };
        let stem = truncate_bytes(&sanitize(&stem), MAX_COMPONENT_BYTES - suffix.len());

        PathBuf::from(series).join(format!("{stem}{suffix}"))
    }
}

/// Zero-pads to the digit count of `run`, floor 2 (D37).
pub fn pad(volume: u32, run: u32) -> String {
    let width = run.to_string().len().max(2);
    format!("{volume:0width$}")
}

/// Fixes titles legal as titles and hostile as paths. Distinct titles must
/// never converge — that would silently merge two series. Truncation is at 255
/// **bytes** and must not split a UTF-8 sequence.
pub fn sanitize(component: &str) -> String {
    let mapped: String = nfc(component)
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| {
            SUBSTITUTIONS
                .iter()
                .find(|(from, _)| *from == c)
                .map_or(c, |(_, to)| *to)
        })
        .collect();

    let trimmed = trim_hostile_edges(&mapped);

    // Reserved on Windows with any extension, so the check is on the stem.
    let stem = trimmed.split('.').next().unwrap_or(&trimmed);
    let trimmed = if DEVICE_NAMES.contains(&stem.to_ascii_lowercase().as_str()) {
        format!("{trimmed}_")
    } else {
        trimmed
    };

    // Truncation can expose a new trailing dot or space, so trim again.
    let capped = trim_hostile_edges(&truncate_bytes(&trimmed, MAX_COMPONENT_BYTES));
    if capped.is_empty() {
        "_".to_string()
    } else {
        capped
    }
}

/// A trailing dot or space is legal on APFS and silently stripped by Windows,
/// which turns one series into two (§6.11). A leading dot hides the folder.
fn trim_hostile_edges(s: &str) -> String {
    s.trim_end_matches(['.', ' '])
        .trim_start_matches('.')
        .to_string()
}

/// Cuts at the last UTF-8 boundary at or below `max` bytes.
fn truncate_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}
