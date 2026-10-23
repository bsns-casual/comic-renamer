//! Pure. `squash` strips; `collision_key` preserves separators — they are not
//! the same function and conflating them merges two series (invariant 7).

use std::collections::BTreeSet;

use unicode_normalization::UnicodeNormalization;

/// Distributors transliterate these three marks into words, inconsistently
/// within one catalog (D16, §6.1).
const EXPANDABLE: [char; 3] = ['&', '+', '/'];
const EXPANSIONS: [&str; 3] = ["and", "plus", "slash"];

/// Lowercase, punctuation and whitespace removed.
pub fn squash(s: &str) -> String {
    nfc(s)
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// `squash` plus punctuation expansions: `&`, `+`, `/` → `and`/`plus`/`slash`
/// (D16, measured at 3.4 points). A set, so `contains("…")` takes a `&str`.
pub fn key_variants(s: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::from([squash(s)]);
    if !s.contains(EXPANDABLE) {
        return out;
    }
    for word in EXPANSIONS {
        let mut expanded = String::with_capacity(s.len() + word.len());
        for c in s.chars() {
            if EXPANDABLE.contains(&c) {
                expanded.push_str(word);
            } else {
                expanded.push(c);
            }
        }
        out.insert(squash(&expanded));
    }
    out
}

/// NFC then case-fold, separators preserved (D2, D36). §6.10 additionally
/// requires `'` and `U+2019` to compare equal.
pub fn collision_key(s: &str) -> String {
    // shortcut: `to_lowercase` is simple case mapping, not full case folding —
    // it leaves `ß` and `ss` distinct where APFS folds them together. Swap in a
    // caseless-folding crate if a real library ever collides on that pair.
    nfc(s)
        .chars()
        .map(|c| if c == '\u{2019}' { '\'' } else { c })
        .flat_map(char::to_lowercase)
        .collect()
}

/// Emitted paths are always NFC.
pub fn nfc(s: &str) -> String {
    s.nfc().collect()
}

/// Invisible to a human reviewer, so rejected in contributed rule data (§7.3)
/// and stripped on the way to a path. One definition, used by the validator,
/// the preset and the grader: written three times at three widths, the
/// narrowest let a character through that the widest refused, so a promo title
/// carrying a bidi control was partly cleaned and then rejected.
///
/// Lives here rather than beside a preset because none of this is one preset's
/// business: `Hell<U+2060>boy` reads as `Hellboy` whatever the path layout.
/// A space look-alike is *not* here — it is visible as a space, and the preset
/// maps it to one.
pub fn is_invisible(c: char) -> bool {
    matches!(c,
        '\u{ad}'            // soft hyphen
        | '\u{34f}'         // combining grapheme joiner
        | '\u{61c}'         // Arabic letter mark
        | '\u{115f}'..='\u{1160}' // Hangul fillers
        | '\u{17b4}'..='\u{17b5}' // Khmer inherent vowels
        | '\u{180b}'..='\u{180f}' // Mongolian variation selectors, vowel separator
        | '\u{200b}'..='\u{200f}' // zero-width space through RLM
        | '\u{2028}'..='\u{202e}' // line/paragraph separators, bidi embedding
        | '\u{2060}'..='\u{2064}' // word joiner, invisible operators
        | '\u{2066}'..='\u{206f}' // bidi isolates, deprecated format characters
        | '\u{3164}'            // Hangul filler
        | '\u{fe00}'..='\u{fe0f}' // variation selectors
        | '\u{feff}'            // byte-order mark
        | '\u{ffa0}'            // halfwidth Hangul filler
        | '\u{fff9}'..='\u{fffb}' // interlinear annotation
        | '\u{e0000}'..='\u{e007f}') // tag characters
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squash_strips_punctuation_and_case() {
        assert_eq!(squash("Nemesis: Rogues' Gallery"), "nemesisroguesgallery");
    }

    #[test]
    fn key_variants_expand_punctuation() {
        assert!(key_variants("Hack/Slash").contains("hackandslash"));
        let w = key_variants("The Wicked + The Divine");
        assert!(w.contains("thewickedandthedivine") && w.contains("thewickedplusthedivine"));
    }

    #[test]
    fn collision_key_folds_case_and_normalises_but_keeps_spaces() {
        assert_eq!(collision_key("Hellboy"), collision_key("hellboy"));
        assert_eq!(collision_key("Pok\u{e9}mon"), collision_key("Poke\u{301}mon"));
        assert_ne!(collision_key("Hellboy"), collision_key("Hell boy"));
    }

    /// §6.10 requires `'` and `U+2019` to compare equal; Task 9's Bourdain
    /// collision test depends on it and NFC alone does not do it.
    #[test]
    fn collision_key_folds_the_typographic_apostrophe() {
        assert_eq!(collision_key("Bourdain's"), collision_key("Bourdain\u{2019}s"));
    }
}
