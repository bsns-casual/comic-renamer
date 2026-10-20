//! The ladder (§6.3). Filenames only — never opens a file (invariant 5).

use crate::corpus::Corpus;
use crate::overrides::Overrides;
use crate::types::{PublisherHint, Resolved};

/// Longest-prefix with the boundary guard, then language (hard filter, no
/// fallback — D33), publisher hint, volume feasibility, shortest run last.
/// Shortest-run fires only when a volume token is present.
pub fn resolve(
    stem: &str,
    hint: &PublisherHint,
    corpus: &Corpus,
    overrides: &Overrides,
    lang: &str,
) -> Resolved {
    todo!()
}

/// From the source folder's own name (§6.3).
pub fn publisher_hint(source_folder: &str, corpus: &Corpus) -> PublisherHint {
    todo!()
}
