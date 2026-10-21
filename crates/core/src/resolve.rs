//! The ladder (§6.3). Filenames only — never opens a file (invariant 5).

use crate::corpus::Corpus;
use crate::normalize::squash;
use crate::overrides::Overrides;
use crate::residue::{eat, strip_cruft, Token};
use crate::types::{Basis, PublisherHint, Resolved, ReviewReason, SeriesRecord, Status};

/// Longest-prefix with the boundary guard, then language (hard filter, no
/// fallback — D33), publisher hint, volume feasibility, shortest run last.
/// Shortest-run fires only when a volume token is present.
///
/// `source_folder` is here because Layer B keys on it: a bare filename key
/// recurs across publishers, so an override is unreachable without it.
pub fn resolve(
    stem: &str,
    source_folder: &str,
    hint: &PublisherHint,
    corpus: &Corpus,
    overrides: &Overrides,
    lang: &str,
) -> Resolved {
    let cleaned = strip_cruft(stem);

    // Layer B first: it is the only place a wrong corpus match is correctable
    // (§6.2.1), so it has to outrank the corpus, not supplement it.
    if let Some(entry) = overrides
        .lookup(stem, source_folder)
        .or_else(|| overrides.lookup(&cleaned, source_folder))
    {
        return Resolved {
            series: entry.series.clone(),
            volume: entry.volume,
            subtitle: entry.subtitle.clone(),
            run: 0,
            status: Status::Ready(Basis::Override),
            record: None,
        };
    }

    let key = squash(&cleaned);
    let Some((matched, candidates)) = longest_clean_prefix(&cleaned, &key, corpus) else {
        return failed();
    };

    let residue = eat(&key[matched..]);
    let volume = residue.tokens.iter().find_map(|t| match t {
        Token::Volume(n) => Some(*n),
        _ => None,
    });
    let subtitle = (!residue.leftover.is_empty()).then(|| residue.leftover.clone());

    // 1. Language: a hard filter that never falls back (D33). A silent
    //    fallback caused 6 of the 7 errors across 2,223 graded rows.
    let in_language: Vec<SeriesRecord> = candidates
        .iter()
        .filter(|r| r.language == lang)
        .cloned()
        .collect();
    if in_language.is_empty() {
        return Resolved {
            series: String::new(),
            volume,
            subtitle,
            run: 0,
            status: Status::Review(ReviewReason::NoLanguageMatch),
            record: None,
        };
    }

    // 2. Publisher inferred from the folder: a preference, not a filter.
    let mut narrowed = in_language.clone();
    let mut basis = Basis::Unambiguous;
    if let PublisherHint(Some(publisher)) = hint {
        let preferred: Vec<SeriesRecord> = narrowed
            .iter()
            .filter(|r| r.publisher == *publisher)
            .cloned()
            .collect();
        if !preferred.is_empty() && preferred.len() < narrowed.len() {
            narrowed = preferred;
            basis = Basis::Publisher;
        }
    }

    // 3. Volume feasibility: a hard constraint. A file numbered volume N
    //    cannot belong to a series with fewer than N entries.
    if let Some(v) = volume {
        let feasible: Vec<SeriesRecord> = narrowed.iter().filter(|r| r.run >= v).cloned().collect();
        if feasible.is_empty() {
            return Resolved {
                series: String::new(),
                volume,
                subtitle,
                run: 0,
                status: Status::Review(ReviewReason::VolumeInfeasible),
                record: None,
            };
        }
        if feasible.len() < narrowed.len() {
            basis = Basis::VolumeFeasibility;
        }
        narrowed = feasible;
    }

    // 4. Shortest feasible run, and only with a volume in hand: without one,
    //    two records are still two records (§6.3).
    let chosen = if narrowed.len() == 1 {
        narrowed.remove(0)
    } else if volume.is_some() {
        // A tie on run means shortest-run has decided nothing. Breaking it on
        // year_began silently reinstates "prefer the earliest edition", which
        // D28 withdraws *by name* for resolving Velvet_vol1 to Malibu 1993
        // inside an Image bundle. §6.3 step 5: never silently pick.
        let shortest = narrowed.iter().map(|r| r.run).min().expect("non-empty");
        let mut tied: Vec<SeriesRecord> = narrowed
            .iter()
            .filter(|r| r.run == shortest)
            .cloned()
            .collect();
        if tied.len() > 1 {
            return Resolved {
                series: String::new(),
                volume,
                subtitle,
                run: 0,
                status: Status::Review(ReviewReason::Ambiguous(narrowed)),
                record: None,
            };
        }
        basis = Basis::ShortestRun;
        tied.remove(0)
    } else {
        // 5. Still ambiguous → review, every candidate listed. Never pick.
        return Resolved {
            series: String::new(),
            volume,
            subtitle,
            run: 0,
            status: Status::Review(ReviewReason::Ambiguous(narrowed)),
            record: None,
        };
    };

    // D15: a subtitle arrives squashed and Phase 1 has no source for the
    // split, so it is carried whole and the row is reviewed.
    let status = match &subtitle {
        Some(s) => Status::Review(ReviewReason::UnsplitSubtitle(s.clone())),
        None => Status::Ready(basis),
    };
    Resolved {
        series: chosen.name.clone(),
        volume,
        subtitle,
        run: chosen.run,
        status,
        record: Some(chosen),
    }
}

/// A failed row carries no guessed title — that is the whole point of it.
fn failed() -> Resolved {
    Resolved {
        series: String::new(),
        volume: None,
        subtitle: None,
        run: 0,
        status: Status::Failed,
        record: None,
    }
}

/// Longest prefix of `key` that hits the index **and** ends on a clean
/// boundary. Indexing `squash(name)` and `name_key` together is what makes
/// `witcheromnibus` reach *The Witcher Omnibus* rather than *Witch* (§6.1).
fn longest_clean_prefix<'c>(
    original: &str,
    key: &str,
    corpus: &'c Corpus,
) -> Option<(usize, &'c [SeriesRecord])> {
    let char_ends: Vec<usize> = key.char_indices().map(|(i, c)| i + c.len_utf8()).collect();
    for (chars, &end) in char_ends.iter().enumerate().rev() {
        let records = corpus.lookup(&key[..end]);
        if records.is_empty() {
            continue;
        }
        if boundary_is_clean(original, chars + 1, &key[end..]) {
            return Some((end, records));
        }
    }
    None
}

/// A prefix match counts only if the boundary coincides with a separator in
/// the original filename or the residue begins with a recognised token
/// (§6.1). Zero false positives across the reference bundle; without it,
/// `witcheromnibus` resolves to *Witch* with residue `eromnibus`.
fn boundary_is_clean(original: &str, matched_chars: usize, residue: &str) -> bool {
    if residue.is_empty() {
        return true;
    }
    if !eat(residue).tokens.is_empty() {
        return true;
    }
    // shortcut: counts alphanumerics in the original against squashed chars.
    // A character whose lowercase is longer than itself (`İ`) shifts the
    // count; those do not occur in the corpus's Latin names.
    let mut kept = 0usize;
    for c in original.chars() {
        if c.is_alphanumeric() {
            kept += 1;
            if kept > matched_chars {
                return false;
            }
        } else if kept == matched_chars && kept > 0 {
            return true;
        }
    }
    false
}

/// From the source folder's own name (§6.3). Free signal when present, and
/// absent for anyone running against `~/Downloads` — which is the common case.
///
/// Matched on **word boundaries**, not as a squashed substring. The corpus
/// ships two-letter publishers (`DC`, `EC`, `IC`, …) and 15,558 distinct names
/// in all, so substring matching fired on every row of the ground-truth set:
/// `Humble Valiant Bundle` matched *Lian* inside va-**LIAN**-t, and
/// `Hellboy Expanded Universe` matched *Pan*. Taking the longest match made it
/// worse, preferring a tiny publisher called *Humble Comics* over the IDW
/// named in the same folder.
///
/// When two publishers both match, there is no hint. §6.3 step 2 lets the hint
/// promote a row to `ready`, so a wrong one is a silent-wrongness path, not
/// noise — and step 5 says never silently pick.
pub fn publisher_hint(source_folder: &str, corpus: &Corpus) -> PublisherHint {
    let mut folder = words(source_folder);
    // The distributor's own name is not a publisher signal, and the corpus
    // contains a small publisher literally called "Humble Comics" — so
    // `Humble Comics Bundle: …` resolved to it in preference to the IDW or
    // Image named later in the same folder.
    if folder.first().is_some_and(|w| DISTRIBUTORS.contains(&w.as_str())) {
        folder.remove(0);
    }
    if folder.is_empty() {
        return PublisherHint(None);
    }
    let mut matched: Vec<&String> = corpus
        .publishers()
        .iter()
        .filter(|p| {
            let needle = words(p);
            !needle.is_empty() && folder.windows(needle.len()).any(|w| w == needle.as_slice())
        })
        .collect();
    matched.dedup();
    match matched.as_slice() {
        [only] => PublisherHint(Some((*only).clone())),
        _ => PublisherHint(None),
    }
}

/// Bundle names begin with the shop, not the publisher (§7, D12: evaluation
/// input is distributor bundles).
const DISTRIBUTORS: [&str; 3] = ["humble", "fanatical", "indiegala"];

/// Splits a folder name into lowercase words, on punctuation and on camelCase
/// boundaries: `HumbleDarkHorseAnniversaryBundle` becomes
/// `[humble, dark, horse, anniversary, bundle]`, and `IDWPublishing` becomes
/// `[idw, publishing]`.
fn words(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut current = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            continue;
        }
        let boundary = c.is_uppercase()
            && !current.is_empty()
            && (chars[i - 1].is_lowercase()
                || chars[i - 1].is_numeric()
                || chars.get(i + 1).is_some_and(|n| n.is_lowercase()));
        if boundary {
            out.push(std::mem::take(&mut current));
        }
        current.extend(c.to_lowercase());
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}
