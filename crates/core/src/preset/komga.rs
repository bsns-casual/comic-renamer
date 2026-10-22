//! Flat `<Series>/<Book>.<ext>` — Komga reads only the leaf folder (§6.8.1).

use super::Preset;
use crate::normalize::nfc;
use crate::overrides::DEVICE_NAMES;
use crate::types::Resolved;
use std::path::PathBuf;

/// A path component's byte budget on every filesystem this targets.
const MAX_COMPONENT_BYTES: usize = 255;

/// Characters that cannot appear in a path, mapped to plain ASCII.
///
/// Never a look-alike. An earlier version mapped `:` to U+A789 MODIFIER
/// LETTER COLON so that `Hellboy: Seed` and `Hellboy - Seed` could not
/// converge — which worked, at the cost of a character nobody can type into
/// a real filename. The convergence it guarded against is no longer silent:
/// `find_collisions` groups rows by the series folder's collision key as well
/// as the whole path, so two titles landing on one folder are reported and
/// the rows refused, never merged.
const SUBSTITUTIONS: [(char, &str); 9] = [
    (':', " - "),
    ('/', " - "),
    ('\\', "-"),
    ('|', "-"),
    ('"', "'"),
    ('<', "("),
    ('>', ")"),
    ('?', ""),
    ('*', ""),
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
        //
        // Built from the *sanitized* series. Building it from the raw one
        // made `Hellboy` and `Hellboy.` share a folder while keeping distinct
        // leaves, so two series merged on disk and the collision detector,
        // which keys on the whole path, reported nothing.
        let mut stem = series.clone();
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
        // saturating: a pathological extension longer than the whole budget
        // underflowed, which panics in debug and wraps in release.
        let budget = MAX_COMPONENT_BYTES.saturating_sub(suffix.len());
        let stem = truncate_bytes(&sanitize(&stem), budget);

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
        .filter(|c| !c.is_control() && !is_invisible(*c))
        .flat_map(|c| {
            SUBSTITUTIONS
                .iter()
                .find(|(from, _)| *from == c)
                .map_or_else(|| c.to_string(), |(_, to)| (*to).to_string())
                .chars()
                .collect::<Vec<_>>()
        })
        .collect();

    // A substitution next to an existing space leaves a run: `Powers: The`
    // becomes `Powers -  The`.
    let mapped = collapse_spaces(&mapped);

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

/// §6.11 forbids these in contributed rule data. Corpus titles are sanitised
/// rather than rejected, but they must not survive into a path either: Humble
/// prefixes promo items with a "Free Issue" marker containing a zero-width
/// character, and `Hellboy` beside `Hell<U+200B>boy` is two folders no one
/// can tell apart in Finder or in a diff.
pub(crate) fn is_invisible(c: char) -> bool {
    matches!(c,
        '\u{ad}'
        | '\u{200b}'..='\u{200f}'
        | '\u{202a}'..='\u{202e}'
        | '\u{2066}'..='\u{2069}'
        | '\u{feff}')
}

fn collapse_spaces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut was_space = false;
    for c in s.chars() {
        let is_space = c == ' ';
        if !(is_space && was_space) {
            out.push(c);
        }
        was_space = is_space;
    }
    out
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
