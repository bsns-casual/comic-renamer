//! The suggestion layer (D53). Filenames only -- never opens a file (invariant 5).
//!
//! Nothing here is an authority. `suggest` proposes an answer for a file no
//! mapping covers, and a human confirms it; nothing it returns is stored
//! without a keystroke (D49). The strategy depends on the marketplace, because
//! the corpus is the wrong engine for most of them.

use crate::corpus::Corpus;
use crate::normalize::squash;
use crate::overrides::Overrides;
use crate::residue::{eat, strip_cruft, Token};
use crate::title::parse_title;
use crate::types::{PublisherHint, SeriesRecord, Suggestion, SuggestionSource};

/// What the corpus strategy needs beyond the stem. `suggest` uses the default
/// (English, no folder hint, no Layer B); `suggest_with` takes the real thing.
pub struct Context<'a> {
    /// Layer B keys on it: a bare filename key recurs across publishers.
    pub source_folder: &'a str,
    pub hint: PublisherHint,
    pub overrides: Option<&'a Overrides>,
    pub lang: &'a str,
}

impl Default for Context<'_> {
    fn default() -> Self {
        Context { source_folder: "", hint: PublisherHint(None), overrides: None, lang: "en" }
    }
}

/// Proposes answers for `stem`. Never errors: with no corpus, or no match, the
/// filename strategy still produces a guess, and "no guess" is an empty list.
pub fn suggest(stem: &str, marketplace: &str, corpus: Option<&Corpus>) -> Vec<Suggestion> {
    suggest_with(stem, marketplace, corpus, &Context::default())
}

/// The context for one bundle, built from the source folder's name. §6.3's
/// folder-name wiring in one place: both the Layer B key and the publisher
/// hint come from `folder`, and a caller cannot supply one without the other.
///
/// It lives here rather than in the CLI so that a test with a corpus can hold
/// the wiring. Without a corpus there is no hint to derive, which is §6.3's
/// "the no-publisher case is the common case" and not an error.
pub fn context_for<'a>(
    folder: &'a str,
    lang: &'a str,
    overrides: &'a Overrides,
    corpus: Option<&Corpus>,
) -> Context<'a> {
    Context {
        source_folder: folder,
        hint: corpus.map_or(PublisherHint(None), |c| publisher_hint(folder, c)),
        overrides: Some(overrides),
        lang,
    }
}

pub fn suggest_with(
    stem: &str,
    marketplace: &str,
    corpus: Option<&Corpus>,
    ctx: &Context,
) -> Vec<Suggestion> {
    // Layer B needs no corpus, so it comes before anything that might lack
    // one: a failed fetch must not cost the one answer a human verified.
    if let Some(found) = override_suggestion(stem, ctx) {
        return vec![found];
    }
    let is_humble = marketplace.to_ascii_lowercase().starts_with("humble");
    if is_humble {
        if let Some(found) = corpus.and_then(|c| corpus_ladder(stem, c, ctx)) {
            // `Some(empty)` is a refusal (wrong language, infeasible volume):
            // the corpus knew the series and said no. A filename guess would
            // be the silent fallback D33 forbids.
            return found;
        }
    }
    filename_suggestion(stem, marketplace, is_humble).into_iter().collect()
}

/// Title-case a kebab or snake stem and parse it. The marketplace only decides
/// what noise to drop first (§5.3.1): nothing is stripped from the mapping key,
/// but a *guess* is better without a release counter or an ISBN in it.
fn filename_suggestion(stem: &str, marketplace: &str, humble: bool) -> Option<Suggestion> {
    let mk = marketplace.to_ascii_lowercase();
    let mut base = stem.to_string();
    if humble {
        base = strip_cruft(&base);
    } else if mk.starts_with("global") {
        // `hammer-head-1_release-0001_978-...`: the title is the first segment,
        // and its trailing number is the issue.
        base = base.split('_').next().unwrap_or("").to_string();
        base = trailing_number_to_issue(&base);
    } else if mk.starts_with("sweet") {
        base = sweet_shop_title(&base);
    }

    let words: Vec<&str> = base
        .split(|c: char| c == '-' || c == '_' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        return None;
    }
    // A stem that already carries capitals is the title (Fanatical); only an
    // all-lowercase kebab name needs casing.
    let text = if base.chars().any(|c| c.is_uppercase()) {
        words.join(" ")
    } else {
        title_case(&words)
    };
    let parsed = parse_title(&text);
    if parsed.series.is_empty() {
        return None;
    }
    Some(Suggestion {
        series: parsed.series,
        volume: parsed.volume,
        subtitle: parsed.subtitle,
        volume_count: None,
        source: SuggestionSource::Filename,
    })
}

/// `-issue-3`, `-anthology`, `-graphicnovel`: the type suffix is not a title.
/// The issue number is kept as `#3`, which the title parser drops.
fn sweet_shop_title(s: &str) -> String {
    for suffix in ["-anthology", "-graphicnovel"] {
        if let Some(t) = s.strip_suffix(suffix) {
            return t.to_string();
        }
    }
    if let Some((t, n)) = s.rsplit_once("-issue-") {
        if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) {
            return format!("{t}-#{n}");
        }
    }
    s.to_string()
}

fn trailing_number_to_issue(s: &str) -> String {
    match s.rsplit_once('-') {
        Some((t, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => {
            format!("{t}-#{n}")
        }
        _ => s.to_string(),
    }
}

/// Capitalises each word, leaving small words lowercase after the first.
// shortcut: a fixed English list; it cannot know `DIE` is an acronym or where
// a colon goes, which is why nothing it returns is stored without a keystroke.
fn title_case(words: &[&str]) -> String {
    const SMALL: [&str; 11] = ["a", "an", "and", "of", "the", "in", "on", "to", "for", "or", "at"];
    words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            if i > 0 && SMALL.contains(w) {
                return w.to_string();
            }
            let mut cs = w.chars();
            match cs.next() {
                Some(f) => f.to_uppercase().chain(cs).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

fn corpus_suggestion(
    series: &str,
    volume: Option<u32>,
    subtitle: &Option<String>,
    run: Option<u32>,
) -> Suggestion {
    Suggestion {
        series: series.to_string(),
        volume,
        subtitle: subtitle.clone(),
        volume_count: run,
        source: SuggestionSource::Corpus,
    }
}

/// Layer B outranks the corpus: it is the only place a wrong corpus match is
/// correctable (§6.2.1).
fn override_suggestion(stem: &str, ctx: &Context) -> Option<Suggestion> {
    let cleaned = strip_cruft(stem);
    let entry = ctx.overrides.and_then(|o| {
        o.lookup(stem, ctx.source_folder)
            .or_else(|| o.lookup(&cleaned, ctx.source_folder))
    })?;
    Some(Suggestion {
        series: entry.series.clone(),
        volume: entry.volume,
        subtitle: entry.subtitle.clone(),
        volume_count: None,
        source: SuggestionSource::Override,
    })
}

/// Longest-prefix with the boundary guard, then language (hard filter, no
/// fallback -- D33), publisher hint, volume feasibility, shortest run last.
/// Shortest-run fires only when a volume token is present.
///
/// `None` means the corpus found nothing at all. `Some(empty)` means it found
/// the series and refused (language, volume). `Some(several)` is ambiguity:
/// every candidate, never a silent pick (§6.3 step 5).
pub fn corpus_ladder(stem: &str, corpus: &Corpus, ctx: &Context) -> Option<Vec<Suggestion>> {
    let cleaned = strip_cruft(stem);

    if let Some(found) = override_suggestion(stem, ctx) {
        return Some(vec![found]);
    }

    let key = squash(&cleaned);
    let (matched, candidates) = longest_clean_prefix(&cleaned, &key, corpus)?;

    let residue = eat(&key[matched..]);
    let volume = residue.tokens.iter().find_map(|t| match t {
        Token::Volume(n) => Some(*n),
        _ => None,
    });
    let subtitle = (!residue.leftover.is_empty()).then(|| residue.leftover.clone());
    let all = |records: &[SeriesRecord]| -> Vec<Suggestion> {
        records
            .iter()
            .map(|r| corpus_suggestion(&r.name, volume, &subtitle, Some(r.run)))
            .collect()
    };

    // 1. Language: a hard filter that never falls back (D33). A silent
    //    fallback caused 6 of the 7 errors across 2,223 graded rows.
    let in_language: Vec<SeriesRecord> = candidates
        .iter()
        .filter(|r| r.language == ctx.lang)
        .cloned()
        .collect();
    if in_language.is_empty() {
        return Some(Vec::new());
    }

    // 2. Publisher inferred from the folder: a preference, not a filter.
    let mut narrowed = in_language.clone();
    if let PublisherHint(Some(publisher)) = &ctx.hint {
        let preferred: Vec<SeriesRecord> = narrowed
            .iter()
            .filter(|r| r.publisher == *publisher)
            .cloned()
            .collect();
        if !preferred.is_empty() && preferred.len() < narrowed.len() {
            narrowed = preferred;
        }
    }

    // 3. Volume feasibility: a hard constraint. A file numbered volume N
    //    cannot belong to a series with fewer than N entries.
    if let Some(v) = volume {
        let feasible: Vec<SeriesRecord> = narrowed.iter().filter(|r| r.run >= v).cloned().collect();
        if feasible.is_empty() {
            // The publisher preference and the feasibility constraint
            // disagree. §6.3 ranks them: step 2 is a preference, step 3 is a
            // hard constraint, so the constraint wins and the preference is
            // dropped rather than the row refused.
            //
            // This matters because `issue_count` counts one publisher's run,
            // not a collected-edition line. *Lady Mechanika* Vol. 6 is real,
            // but the English line spans Aspen, Benitez and Image and no
            // single record reaches 6 -- so narrowing to Image first made a
            // legitimate book infeasible.
            let line: Vec<SeriesRecord> =
                in_language.iter().filter(|r| r.run >= v).cloned().collect();
            if line.is_empty() {
                // Review Focus 2: no record of this series, under any
                // publisher, holds that many volumes.
                return Some(Vec::new());
            }
            narrowed = line;
        } else {
            narrowed = feasible;
        }
    }

    // 4. Shortest feasible run, and only with a volume in hand: without one,
    //    two records are still two records (§6.3).
    if narrowed.len() == 1 {
        return Some(all(&narrowed));
    }
    if volume.is_some() {
        // A tie on run means shortest-run has decided nothing. Breaking it on
        // year_began silently reinstates "prefer the earliest edition", which
        // D28 withdraws *by name* for resolving Velvet_vol1 to Malibu 1993
        // inside an Image bundle. §6.3 step 5: never silently pick.
        let shortest = narrowed.iter().map(|r| r.run).min().expect("non-empty");
        let tied: Vec<SeriesRecord> =
            narrowed.iter().filter(|r| r.run == shortest).cloned().collect();
        if tied.len() == 1 {
            return Some(all(&tied));
        }
    }
    // 5. Still ambiguous: every candidate listed, which is a picker's list.
    Some(all(&narrowed))
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
