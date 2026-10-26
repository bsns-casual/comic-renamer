//! Flat `<Series>/<Book>.<ext>` — Komga reads only the leaf folder (§6.8.1).

use super::Preset;
use crate::normalize::{is_invisible, nfc};
use crate::overrides::DEVICE_NAMES;
use crate::types::Resolved;
use std::path::PathBuf;

/// A path component's byte budget on every filesystem this targets.
const MAX_COMPONENT_BYTES: usize = 255;

/// Characters that cannot appear in a path, or can only appear in one as a
/// look-alike, mapped to plain ASCII.
///
/// The space variants are the second kind: a no-break space is legal in a
/// filename and reads as a space, so `Hell<U+00A0>boy` beside `Hell boy` is
/// one series to anyone looking and two on disk. Rejecting them would lose a
/// title that legitimately uses one, which is why they are here and not in
/// `normalize::is_invisible`.
///
/// Never a look-alike. An earlier version mapped `:` to U+A789 MODIFIER
/// LETTER COLON so that `Hellboy: Seed` and `Hellboy - Seed` could not
/// converge — which worked, at the cost of a character nobody can type into
/// a real filename. The convergence it guarded against is no longer silent:
/// `find_collisions` groups rows by the series folder's collision key as well
/// as the whole path, so two titles landing on one folder are reported and
/// the rows refused, never merged.
const SUBSTITUTIONS: [(char, &str); 25] = [
    (':', " - "),
    ('/', " - "),
    ('\\', "-"),
    ('|', "-"),
    ('"', "'"),
    ('<', "("),
    ('>', ")"),
    ('?', ""),
    ('*', ""),
    ('\u{a0}', " "),
    ('\u{2000}', " "),
    ('\u{2001}', " "),
    ('\u{2002}', " "),
    ('\u{2003}', " "),
    ('\u{2004}', " "),
    ('\u{2005}', " "),
    ('\u{2006}', " "),
    ('\u{2007}', " "),
    ('\u{2008}', " "),
    ('\u{2009}', " "),
    ('\u{200a}', " "),
    ('\u{202f}', " "),
    ('\u{205f}', " "),
    ('\u{3000}', " "),
    ('\u{1680}', " "),
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
        // After the volume and before the subtitle (D60): a volume's issues
        // have to sort inside that volume, and the subtitle has to stay last
        // because filename sort order is reading order (§6.8.1).
        if let Some(issue) = resolved.issue {
            stem.push_str(&format!(" {}", pad_issue(issue)));
        }
        if let Some(subtitle) = &resolved.subtitle {
            stem.push_str(" - ");
            stem.push_str(subtitle);
        }

        // Sanitised like every other part of the name. Unsanitised, `.{ext}`
        // went straight into `join`, so `cbz/sub/dir` added three components
        // and a 254-byte extension drove the budget to zero, emptied the stem
        // and left a leaf of `.` plus the extension — a preset emitting `..`,
        // which invariant 1 forbids in its own words. The `..` and `.` cases
        // sanitise to `_`, so the suffix is visible and inert rather than gone.
        let suffix = if ext.is_empty() {
            String::new()
        } else {
            format!(".{}", sanitize(ext))
        };
        // Half the component, so neither the title nor the extension can
        // squeeze the other out of the name entirely. saturating: a
        // pathological extension longer than the whole budget underflowed,
        // which panics in debug and wraps in release.
        let suffix = truncate_bytes(&suffix, MAX_COMPONENT_BYTES / 2);
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

/// Zero-pads an issue to a fixed four digits, with no `#` (D60).
///
/// Takes no run width, unlike `pad`: the ceiling is known and low — the
/// largest issue number in the corpus is 666 — so four digits covers every
/// real issue and there is no per-series count to store or get wrong. A `#`
/// would sort and display badly everywhere outside Komga and buys nothing
/// inside it, where the number comes from filename order anyway.
pub fn pad_issue(issue: u32) -> String {
    format!("{issue:04}")
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
