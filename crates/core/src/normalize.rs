//! Pure. `squash` strips; `collision_key` preserves separators — they are not
//! the same function and conflating them merges two series (invariant 7).

use std::collections::BTreeSet;

/// Lowercase, punctuation and whitespace removed.
pub fn squash(s: &str) -> String {
    todo!()
}

/// `squash` plus punctuation expansions: `&`, `+`, `/` → `and`/`plus`/`slash`
/// (D16, measured at 3.4 points). A set, so `contains("…")` takes a `&str`.
pub fn key_variants(s: &str) -> BTreeSet<String> {
    todo!()
}

/// NFC then case-fold, separators preserved (D2, D36).
pub fn collision_key(s: &str) -> String {
    todo!()
}

/// Emitted paths are always NFC.
pub fn nfc(s: &str) -> String {
    todo!()
}
