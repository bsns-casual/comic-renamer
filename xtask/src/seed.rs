//! `cargo xtask seed` -- writes the marketplace tables into `data/mappings/`
//! and the Humble candidates to `target/seed/humble.tsv`.
//!
//! Only the marketplace rows ship: the maintainer confirmed each one title by
//! title. The Humble titles are verified too, but their split into series /
//! volume / subtitle is `parse_title`'s guess, and nothing is stored without a
//! keystroke (D50, D51), so that file stays out of the tree until the owner
//! picks a review method.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use comic_renamer_core::corpus::Corpus;
use comic_renamer_core::mapping::{find_key_collisions, key, to_tsv, MappingRow};
use comic_renamer_core::normalize::squash;
use comic_renamer_core::title::{is_volume_word, number_value, parse_title};

use xtask::strip_promo;

use super::{percent, GROUND_TRUTH, LANG, MARKETPLACE};

/// Rows by marketplace. A short line is an error, never a panic.
fn marketplace_rows(text: &str) -> Result<BTreeMap<String, Vec<MappingRow>>, String> {
    let mut by_market: BTreeMap<String, Vec<MappingRow>> = BTreeMap::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split('\t').collect();
        let [market, title, stem] = f[..f.len().min(3)] else {
            return Err(format!("bad marketplace row {line:?}"));
        };
        // Column 5, the confirmed series, is optional and overrides the
        // parser's split. Only rows the maintainer has ruled on carry it.
        let series = f.get(4).map(|s| s.trim()).filter(|s| !s.is_empty());
        by_market
            .entry(market.to_string())
            .or_default()
            // No corpus here. These 12 rows ship, and the maintainer
            // confirmed each one title by title, so a lookup could only
            // overrule a keystroke (D50, D51).
            .push(build_row(title, stem, market, series, None)?);
    }
    Ok(by_market)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_marketplace_line_is_an_error_not_a_panic() {
        let text = "marketplace\ttitle\tstem\tformats\nFanatical\tOnly a title\n";
        assert!(marketplace_rows(text).is_err());
    }

    /// `DIE: Loaded` and `Glarien: Origins` are their own series, which the
    /// parser cannot know: the same colon is a subtitle boundary in
    /// `Harbinger, Vol. 2: Renegades`. The fifth column is the maintainer's
    /// ruling, and without it the parser files both as arcs within `DIE` and
    /// `Glarien`.
    #[test]
    fn a_confirmed_series_overrides_the_parsers_colon() {
        let rows = marketplace_rows(MARKETPLACE).unwrap();
        let sweet = &rows["Sweet Shop"];
        let die = sweet.iter().find(|r| r.key == "dieloadedissue1").expect("the DIE row");
        assert_eq!(die.series, "DIE: Loaded");
        assert_eq!(die.subtitle.as_deref(), Some("#1"));
        assert_eq!(die.volume, None, "#1 is an issue, not a volume");

        let g = sweet.iter().find(|r| r.key == "glarienoriginsissue1").expect("the Glarien row");
        assert_eq!(g.series, "Glarien: Origins");
        assert_eq!(g.subtitle.as_deref(), Some("#1"));

        // Left to the parser, the colon splits and both become arcs.
        assert_eq!(build_row("DIE: Loaded #1", "d", "x", None, None).unwrap().series, "DIE");
    }

    /// A series that is not a prefix of its own title means one of the two is
    /// wrong, and only a human knows which.
    #[test]
    fn a_confirmed_series_that_is_not_a_prefix_is_an_error() {
        assert!(build_row("DIE: Loaded #1", "d", "x", Some("Glarien"), None).is_err());
    }

    /// The shape the parser cannot see: it cuts at the first colon, so a
    /// marker that follows one is past the end of what it reads.
    #[test]
    fn a_trailing_volume_marker_is_found_past_the_colon() {
        assert_eq!(
            trailing_volume("Attack on Titan: Before the Fall Vol. 9"),
            Some(("Attack on Titan: Before the Fall", 9))
        );
        // Nothing may follow the number, or the tail is a subtitle and the
        // number is the volume of an arc the parser already splits correctly.
        assert_eq!(trailing_volume("Saga Vol. 3: Planet Death"), None);
        // No marker word: a bare year is part of the name (D44's own case).
        assert_eq!(trailing_volume("Hellboy Winter Special 2018"), None);
        // Nothing left to be a series.
        assert_eq!(trailing_volume("Vol. 1"), None);
    }

    /// `Attack on Titan: Before the Fall` is a series of its own in the
    /// corpus, with 17 issues. Left to the parser the colon splits, the
    /// marker strands in the subtitle, and volume 9 of a real series becomes
    /// an unnumbered arc of another one.
    #[test]
    fn a_corpus_confirmed_stem_keeps_its_colon_and_takes_the_volume() {
        let title = "Attack on Titan: Before the Fall Vol. 9";
        let guessed = build_row(title, "aotbtf09", "Humble", None, None).unwrap();
        assert_eq!(guessed.series, "Attack on Titan");
        assert_eq!(guessed.volume, None);
        assert_eq!(guessed.subtitle.as_deref(), Some("Before the Fall Vol. 9"));

        let found = Some(("Attack on Titan: Before the Fall".to_string(), 9));
        let row = build_row(title, "aotbtf09", "Humble", None, found).unwrap();
        assert_eq!(row.series, "Attack on Titan: Before the Fall");
        assert_eq!(row.volume, Some(9));
        assert_eq!(row.subtitle, None, "the marker is the volume, not a subtitle");
    }

    /// The corpus is an authority on what series exist, never on what the
    /// maintainer ruled. A row carrying a fifth column was reviewed by hand
    /// (D50, D51), and a lookup must not quietly replace that.
    #[test]
    fn a_humans_ruling_outranks_the_corpus() {
        let row = build_row(
            "DIE: Loaded Vol. 2",
            "dieloaded2",
            "x",
            Some("DIE: Loaded"),
            Some(("Something Else Entirely".to_string(), 2)),
        )
        .unwrap();
        assert_eq!(row.series, "DIE: Loaded");
    }

    #[test]
    fn the_real_marketplace_fixture_builds() {
        assert_eq!(marketplace_rows(MARKETPLACE).unwrap().values().map(Vec::len).sum::<usize>(), 12);
    }
}

pub fn seed() -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let by_market = marketplace_rows(MARKETPLACE)?;
    for (market, rows) in &by_market {
        let slug = market.to_lowercase().replace(' ', "-");
        let path = root.join("data/mappings").join(format!("{slug}.tsv"));
        std::fs::write(&path, to_tsv(rows)).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("{:3} rows  {}", rows.len(), path.display());
    }

    // Optional on purpose: the tables above need none, and a maintainer
    // without the download still gets the parser's guesses (D49).
    let corpus = match super::open_corpus() {
        Ok(corpus) => Some(corpus),
        Err(e) => {
            println!("\nno corpus ({e}); every split below is the parser's guess");
            None
        }
    };

    let mut rows = Vec::new();
    let mut classes: BTreeMap<&'static str, Vec<(String, MappingRow)>> = BTreeMap::new();
    let mut rejected: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in GROUND_TRUTH.lines().skip(1) {
        let f: Vec<&str> = line.split('\t').collect();
        let [bundle, title, stem] = f[..f.len().min(3)] else {
            rejected.entry("short line".into()).or_default().push(line.to_string());
            continue;
        };
        // No confirmed column here: the Humble table is the parser's guess by
        // construction, and its known-bad splits are what the review in a
        // `map` session is for. The corpus narrows that review: where it
        // recognises the whole stem as a series, the split is confirmed
        // against the authority this project already ships rather than
        // guessed from the string.
        let found = corpus.as_ref().and_then(|c| corpus_split(&strip_promo(title), c));
        let confirmed = found.is_some();
        let row = match build_row(title, stem, bundle, None, found) {
            Ok(row) => row,
            Err(e) => {
                // The error names the title; group by what follows it.
                let why = e.split_once("\": ").map_or(e.as_str(), |(_, w)| w).to_string();
                rejected.entry(why).or_default().push(title.to_string());
                continue;
            }
        };
        let class = if confirmed { CORPUS_CONFIRMED } else { classify(title, &row) };
        classes.entry(class).or_default().push((title.to_string(), row.clone()));
        rows.push(row);
    }
    // Exact repeats collapse; a pair that disagrees survives to be counted below.
    let mut seen = HashSet::new();
    rows.retain(|r| seen.insert((r.key.clone(), r.bundle.clone(), r.series.clone(), r.volume, r.subtitle.clone())));
    let collisions = find_key_collisions(&rows);
    let out = root.join("target/seed/humble.tsv");
    std::fs::create_dir_all(out.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&out, to_tsv(&rows)).map_err(|e| e.to_string())?;

    let total: usize = classes.values().map(Vec::len).sum();
    println!("\nhumble candidates (NOT shipped): {} unique rows -> {}", rows.len(), out.display());
    println!(
        "{total} rows built; {} rejected; {} key collisions",
        rejected.values().map(Vec::len).sum::<usize>(),
        collisions.len()
    );
    for (why, titles) in &rejected {
        println!("  rejected: {why} ({}) e.g. {:?}", titles.len(), titles[0]);
    }
    for (class, members) in &classes {
        println!("  {:5}  {:5.1}%  {class}", members.len(), percent(members.len(), total));
    }
    for (class, members) in &classes {
        if *class == "clean" {
            continue;
        }
        println!("\n{class} -- sample");
        for (title, r) in members.iter().take(8) {
            println!("  {title:?} -> {:?} / {:?} / {:?}", r.series, r.volume, r.subtitle);
        }
    }
    Ok(())
}

/// The confirmed series and whatever the title leaves after it. A strip, not a
/// guess: the series is always a prefix of the verified title, so anything
/// after it is the rest of that title and the separator between them is
/// punctuation the preset would render anyway. Refuses rather than guesses if
/// the series is not a prefix, because then one of the two is wrong and only a
/// human knows which.
fn confirmed_split(title: &str, series: &str) -> Result<(String, Option<String>), String> {
    let rest = title
        .strip_prefix(series)
        .ok_or_else(|| format!("{series:?} is not how {title:?} starts"))?
        .trim_start_matches([' ', ':', '-', ','])
        .trim();
    Ok((series.to_string(), (!rest.is_empty()).then(|| rest.to_string())))
}

/// `<stem> Vol. N` with nothing after the number, and the stem itself.
/// `None` when the title does not end in a marker, or when stripping it
/// would leave no series.
///
/// The parser cannot look here: it cuts at the first colon, so in
/// `Attack on Titan: Before the Fall Vol. 9` the marker is past the end of
/// what it reads.
fn trailing_volume(title: &str) -> Option<(&str, u32)> {
    let tokens: Vec<(usize, &str)> = title
        .split_whitespace()
        .map(|t| (t.as_ptr() as usize - title.as_ptr() as usize, t))
        .collect();
    let [.., (marker_at, marker), (_, number)] = tokens[..] else {
        return None;
    };
    if !is_volume_word(marker) {
        return None;
    }
    let volume = number_value(number)?;
    let stem = title[..marker_at].trim_end_matches([' ', ',']);
    (!stem.is_empty()).then_some((stem, volume))
}

/// Whether the corpus records `candidate` as a series of its own, in `LANG`.
///
/// Compares squashed *names* on both sides. The index also holds GCD's own
/// `name_key`, which strips stopwords -- `attackontitanbeforefall`, no "the"
/// -- so a hit reached only through that entry is a different string, and
/// matching against it alone reported this very series as unconfirmed
/// (docs/research/2026-10-25-the-corpus-can-confirm-a-colon.md).
fn is_series(candidate: &str, corpus: &Corpus) -> bool {
    let key = squash(candidate);
    !key.is_empty()
        && corpus
            .lookup(&key)
            .iter()
            .any(|r| r.language == LANG && squash(&r.name) == key)
}

/// The series and volume the corpus confirms for `title`, if any: the colon
/// inside the stem belongs to the series name, and the trailing marker is the
/// volume.
///
/// Only titles whose stem carries a colon are asked about. Without one the
/// parser already splits at the marker and agrees, so a lookup could only
/// restate it -- and restating it over 900 rows would hide which 111 this
/// change is for.
///
/// `issue_count` is deliberately not consulted. A series the corpus records
/// at 15 issues can still have a volume 19 in a bundle: four
/// `Battle Angel Alita: Last Order` rows are exactly that, and gating on the
/// count discards them. The corpus confirms the *series*, never the volume
/// (D44).
fn corpus_split(title: &str, corpus: &Corpus) -> Option<(String, u32)> {
    let (stem, volume) = trailing_volume(title)?;
    (stem.contains(':') && is_series(stem, corpus)).then(|| (stem.to_string(), volume))
}

fn build_row(
    title: &str,
    stem: &str,
    bundle: &str,
    confirmed: Option<&str>,
    corpus_series: Option<(String, u32)>,
) -> Result<MappingRow, String> {
    let key = key(stem).ok_or_else(|| format!("{stem:?} has no key"))?;
    let parsed = parse_title(&strip_promo(title));
    // Either source replaces the split whole. The fifth column drops the
    // parser's volume with it, because that volume came from reading the
    // title the other way; the corpus supplies its own.
    let (series, subtitle, volume) = match (confirmed, corpus_series) {
        // A keystroke outranks a lookup, and the fifth column is a keystroke.
        (Some(c), _) => {
            let (s, sub) = confirmed_split(&strip_promo(title), c)?;
            (s, sub, None)
        }
        // The stem is the whole title bar the marker, so nothing is left over.
        (None, Some((series, volume))) => (series, None, Some(volume)),
        (None, None) => (parsed.series, parsed.subtitle, parsed.volume),
    };
    let row = MappingRow {
        key,
        series,
        volume,
        // Task 8 derives a real issue; the parser cannot see the filename.
        issue: None,
        subtitle,
        lang: LANG.to_string(),
        volume_count: None,
        bundle: bundle.to_string(),
    };
    row.validate().map_err(|e| format!("{title:?}: {e}"))?;
    Ok(row)
}

/// Not a known-bad class: these rows were decided by the corpus, not guessed.
const CORPUS_CONFIRMED: &str = "corpus confirms the series (colon kept, volume recovered)";

/// The known-bad classes, from what the parser is known to get wrong. First
/// match wins; `clean` means none of them fired, not that a person checked.
fn classify(title: &str, row: &MappingRow) -> &'static str {
    let last = title.split_whitespace().last().unwrap_or("");
    if title.matches(':').count() > 1 {
        "multiple colons (series may contain a colon)"
    } else if row.volume.is_none()
        && row.subtitle.as_deref().is_some_and(|s| {
            let t: Vec<&str> = s.split_whitespace().collect();
            t.windows(2).any(|w| is_volume_word(w[0]) && number_value(w[1]).is_some())
        })
    {
        "volume marker swallowed into the subtitle (colon came first)"
    } else if row.volume.is_none() && !last.is_empty() && last.chars().all(|c| c.is_ascii_digit()) {
        "bare trailing number (not a marker)"
    } else {
        "clean"
    }
}
