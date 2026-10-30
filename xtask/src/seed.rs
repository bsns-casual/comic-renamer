//! `cargo xtask seed` -- writes the shipped table to `data/decisions.tsv` and
//! the Humble candidates to `target/seed/humble.tsv`.
//!
//! The two have different shapes on purpose since D63/D64. The shipped table is
//! seven columns, one row per key, no bundle. The Humble intermediate keeps the
//! old eight-column shape, its row order and its row count, because
//! `scripts/review-rows.py` reads those eight fields in order and the review
//! artifact keys 2,204 stored decisions on a row's ordinal here plus its
//! `(key, bundle)`. See the spec's §2.1.
//!
//! Only the marketplace rows ship: the maintainer confirmed each one title by
//! title. The Humble titles are verified too, but their split into series /
//! volume / subtitle is `parse_title`'s guess, and nothing is stored without a
//! keystroke (D50, D51), so that file stays out of the tree until the owner
//! picks a review method.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use comic_renamer_core::corpus::Corpus;
use comic_renamer_core::mapping::{find_key_collisions, key, to_tsv, MappingRow, MAX_ISSUE};
use comic_renamer_core::normalize::{collision_key, squash};
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
            .push(build_row(title, stem, series, None)?);
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
        // This comment used to sit beside `subtitle == Some("#1")`, and said
        // what the row could not: "#1 is an issue, not a volume". D60 gave it
        // a field, so it is the row that says it now.
        assert_eq!((die.volume, die.issue), (None, Some(1)), "#1 is an issue, not a volume");
        assert_eq!(die.subtitle, None);

        let g = sweet.iter().find(|r| r.key == "glarienoriginsissue1").expect("the Glarien row");
        assert_eq!(g.series, "Glarien: Origins");
        assert_eq!((g.volume, g.issue), (None, Some(1)));

        // Left to the parser, the colon splits and both become arcs.
        assert_eq!(build_row("DIE: Loaded #1", "d", None, None).unwrap().series, "DIE");
    }

    /// A series that is not a prefix of its own title means one of the two is
    /// wrong, and only a human knows which.
    #[test]
    fn a_confirmed_series_that_is_not_a_prefix_is_an_error() {
        assert!(build_row("DIE: Loaded #1", "d", Some("Glarien"), None).is_err());
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
        let guessed = build_row(title, "aotbtf09", None, None).unwrap();
        assert_eq!(guessed.series, "Attack on Titan");
        assert_eq!(guessed.volume, None);
        assert_eq!(guessed.subtitle.as_deref(), Some("Before the Fall Vol. 9"));

        let found = Some(("Attack on Titan: Before the Fall".to_string(), 9));
        let row = build_row(title, "aotbtf09", None, found).unwrap();
        assert_eq!(row.series, "Attack on Titan: Before the Fall");
        assert_eq!(row.volume, Some(9));
        assert_eq!(row.subtitle, None, "the marker is the volume, not a subtitle");
    }

    /// The corpus is an authority on what series exist, never on what the
    /// maintainer ruled. A row carrying a fifth column was reviewed by hand
    /// (D50, D51), and a lookup must not quietly replace that.
    #[test]
    fn a_humans_ruling_outranks_the_corpus() {
        let row = build_row("DIE: Loaded Vol. 2", "dieloaded2", Some("DIE: Loaded"),
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
    // One table now (D63). The marketplaces still structure the *fixture* --
    // its first column is the marketplace -- but they no longer structure the
    // output, so every row joins one file sorted by key.
    let by_market = marketplace_rows(MARKETPLACE)?;
    let mut shipped: Vec<MappingRow> = by_market.values().flatten().cloned().collect();
    shipped.sort_by(|a, b| a.key.cmp(&b.key));
    if let Some(key) = find_key_collisions(&shipped).into_iter().next() {
        return Err(format!("two fixture rows share key {key:?}; identity is the key alone (D64)"));
    }
    let path = root.join("data/decisions.tsv");
    std::fs::write(&path, to_tsv(&shipped)).map_err(|e| format!("{}: {e}", path.display()))?;
    println!("{:3} rows  {}", shipped.len(), path.display());

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
    let mut sidecar: Vec<String> = vec!["key\tbundle\tstem\ttitle\tclass".to_string()];
    let mut built: Vec<Built> = Vec::new();
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
        let row = match build_row(title, stem, None, found) {
            Ok(row) => row,
            Err(e) => {
                // The error names the title; group by what follows it.
                let why = e.split_once("\": ").map_or(e.as_str(), |(_, w)| w).to_string();
                rejected.entry(why).or_default().push(title.to_string());
                continue;
            }
        };
        let class = if confirmed {
            CORPUS_CONFIRMED
        } else if row.issue.is_some() {
            // The parser owns a `#N`; `issue_from_stem` refuses any title
            // carrying one, so the `#` says which of the two fired.
            if title.contains('#') {
                ISSUE_IN_TITLE
            } else {
                ISSUE_FROM_FILENAME
            }
        } else {
            classify(title, &row)
        };
        let mut row = row;
        let class = settle_casing(&mut row, class, corpus.as_ref());
        built.push(Built {
            title: title.to_string(),
            stem: stem.to_string(),
            row,
            bundle: bundle.to_string(),
            class,
        });
    }

    let propagated = propagate_within_titles(&mut built)?;
    flag_disagreeing_siblings(&mut built);

    for b in built {
        classes.entry(b.class).or_default().push((b.title.clone(), b.row.clone()));
        // The class, beside the row it describes. Without this the review
        // page re-derived it in Python from the same titles, and the two
        // disagreed: that copy's volume-word list carried `book`, `part` and
        // `v`, which `title::is_volume_word` does not match. A second
        // implementation of a judgement is a second thing to be wrong, and a
        // page meant to drive 2,204 decisions should agree with the report.
        sidecar.push(format!(
            "{}\t{}\t{}\t{}\t{}",
            b.row.key, b.bundle, b.stem, b.title, b.class
        ));
        rows.push((b.row, b.bundle));
    }
    // Exact repeats collapse; a pair that disagrees survives to be counted below.
    let mut seen = HashSet::new();
    rows.retain(|(r, bundle)| seen.insert(dedup_identity(r, bundle)));
    let collisions = disagreeing_within_a_bundle(&rows);
    let out = root.join("target/seed/humble.tsv");
    std::fs::create_dir_all(out.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&out, to_tsv_with_bundle(&rows)).map_err(|e| e.to_string())?;
    let classes_out = root.join("target/seed/humble-classes.tsv");
    std::fs::write(&classes_out, sidecar.join("\n") + "\n").map_err(|e| e.to_string())?;

    let total: usize = classes.values().map(Vec::len).sum();
    println!("\nhumble candidates (NOT shipped): {} unique rows -> {}", rows.len(), out.display());
    println!("classes -> {}", classes_out.display());
    println!(
        "{propagated} rows took a sibling's decomposition: one verified title, one split (D61 as amended)"
    );
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

/// Every semantic field of a row in this path. `lang` is the constant `LANG`
/// and `volume_count` is always `None` here, which is why neither appears.
///
/// Named rather than inlined because the property matters: an exact repeat
/// collapses, and a pair that *disagrees* must survive to
/// `find_key_collisions`, the gate whose job is to refuse it. Adding `issue`
/// to the row broke that silently — the tuple had been every field, and a
/// pair differing only by issue folded before the gate ran. No fixture pair
/// exercises it, which is exactly why it needed a test rather than a reader.
/// `target/seed/humble.tsv` in the pre-D64 eight-column shape, bundle last.
///
/// Deliberately not `mapping::to_tsv`, which now emits seven columns. This file
/// is the generator's intermediate and the review page's input:
/// `scripts/review-rows.py` reads exactly these eight fields in order and
/// `sys.exit`s otherwise, and the review artifact keys its 2,204 stored
/// decisions on a row's ordinal here plus its `(key, bundle)`. Changing the
/// shape, the order or the count of these rows orphans all of them (spec §2.1).
fn to_tsv_with_bundle(rows: &[(MappingRow, String)]) -> String {
    let n = |v: Option<u32>| v.map(|v| v.to_string()).unwrap_or_default();
    let mut out =
        String::from("key\tseries\tvolume\tissue\tsubtitle\tlang\tvolume_count\tbundle\n");
    for (r, bundle) in rows {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            r.key,
            r.series,
            n(r.volume),
            n(r.issue),
            r.subtitle.as_deref().unwrap_or(""),
            r.lang,
            n(r.volume_count),
            bundle
        ));
    }
    out
}

/// `(key, bundle)` pairs whose rows disagree on a field — the semantics
/// `mapping::find_key_collisions` had before D64.
///
/// The shipped table's gate is now "a key appears twice", which this file
/// cannot use: the Humble intermediate is per bundle and 158 of its keys
/// legitimately appear under more than one, agreeing every time. What would be
/// a defect here is two rows for one key *in one bundle* that disagree.
fn disagreeing_within_a_bundle(rows: &[(MappingRow, String)]) -> Vec<(String, String)> {
    let mut first: HashMap<(&str, String), &MappingRow> = HashMap::new();
    let mut bad = BTreeSet::new();
    for (r, bundle) in rows {
        let slot = first.entry((&r.key, collision_key(bundle))).or_insert(r);
        if *slot != r {
            bad.insert((r.key.clone(), bundle.clone()));
        }
    }
    bad.into_iter().collect()
}

fn dedup_identity(
    r: &MappingRow,
    bundle: &str,
) -> (String, String, String, Option<u32>, Option<u32>, Option<String>) {
    (
        r.key.clone(),
        // Still per bundle, because this folds the *generator's* repeats and
        // its input really does have bundles (spec §2.1). The shipped table's
        // own one-row-per-key rule is `find_key_collisions`, one line later.
        bundle.to_string(),
        r.series.clone(),
        r.volume,
        r.issue,
        r.subtitle.clone(),
    )
}

/// A trailing `#N` taken out of a leftover, the way `parse_title` takes one
/// out of a whole title. `("#1")` leaves nothing behind, which is a subtitle
/// of `None` rather than an empty one.
fn take_hash_issue(text: Option<String>) -> (Option<String>, Option<u32>) {
    let Some(t) = text else {
        return (None, None);
    };
    let trimmed = t.trim_end();
    // `rsplit_once` yields nothing when there is no whitespace, which is the
    // bare `#1` case — consumed here, where a whole title would keep it.
    let (head, last) = trimmed
        .rsplit_once(char::is_whitespace)
        .map_or(("", trimmed), |(head, last)| (head, last));
    let Some(issue) = last.strip_prefix('#').and_then(|d| d.parse::<u32>().ok()) else {
        return (Some(t), None);
    };
    // Above the ceiling `MappingRow::validate` refuses the row outright, so
    // taking the number would drop the whole file out of the table — worse
    // than leaving it as the text it used to ship as.
    if issue > MAX_ISSUE {
        return (Some(t), None);
    }
    let rest = head.trim_end();
    // `Vol. #3` is a volume written with a hash, not an issue — the same
    // shape `take_trailing_issue` declines in core. This arm discards the
    // volume, so taking it would lose the number and strand `Vol.` in the
    // subtitle.
    if rest.split_whitespace().next_back().is_some_and(is_volume_word) {
        return (Some(t), None);
    }
    ((!rest.is_empty()).then_some(rest.to_string()), Some(issue))
}

/// The issue number the FILENAME records, when the verified title ends in that
/// same bare number (D61).
///
/// Three conditions, each one paid for. The stem must say `issue<N>`, because
/// a trailing digit alone is not evidence: the corpus confirms `RoboCop 2` and
/// `B.P.R.D.: 1948` as whole titles, so inferring from the number would cost
/// real titles to save typing. The title must end in that same number, so a
/// filename naming a different issue cannot rewrite it. And the title must
/// carry no `#`, because `parse_title` already takes those into
/// `ParsedTitle::issue` and taking one twice would strip the series down.
fn issue_from_stem(stem: &str, title: &str) -> Option<u32> {
    if title.contains('#') {
        return None;
    }
    let (_, tail) = split_trailing_number(title)?;
    (filename_issue(stem)? == tail).then_some(tail)
}

/// The number after the last `issue` or `issues` in `stem`, however it is
/// separated: `_issue1_`, `-issue3`, `issue635`. No regex crate in this
/// workspace, and this needs no dependency.
fn filename_issue(stem: &str) -> Option<u32> {
    let lower = stem.to_ascii_lowercase();
    let at = lower.rfind("issue")?;
    let rest = &lower[at + "issue".len()..];
    let rest = rest.strip_prefix('s').unwrap_or(rest);
    let rest = rest.trim_start_matches(['-', '_']);
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// The bare whitespace-delimited number a string ends in, and what comes
/// before it. `None` when it does not end in one.
///
/// `rsplit_once` rather than `rsplit().next()`: the latter always yields, so
/// its failure arm was unreachable. The `map_or` keeps the lone-number case —
/// a title that is nothing but `8` has an empty head, not no match.
fn split_trailing_number(text: &str) -> Option<(&str, u32)> {
    let trimmed = text.trim_end();
    let (head, last) = trimmed
        .rsplit_once(char::is_whitespace)
        .map_or(("", trimmed), |(head, last)| (head, last));
    Some((head.trim_end(), last.parse().ok()?))
}

/// One verified title, one decomposition.
///
/// Humble ships the same comic under several stems and only the `issue<N>`
/// spelling corroborates, so D61 applied per filename split 57 titles:
/// `hacktivistissue1…` became `Hacktivist` issue 1 while `hacktivist001…`
/// stayed `Hacktivist 1`, and one comic landed in two Komga series folders.
/// Invisible to every check the project has — the full paths differ and so do
/// the folder collision keys (`hacktivist` against `hacktivist1`), so
/// `find_collisions` reports nothing.
///
/// The identical **verified** title is the authority here, not an inference:
/// two files Humble published under one title are one comic, which is
/// stronger evidence than either the filename or the corpus. Amends D61,
/// which considered one filename at a time.
///
/// Returns how many rows took a sibling's decomposition.
fn propagate_within_titles(built: &mut [Built]) -> Result<usize, String> {
    let derived: BTreeMap<(String, String), ParsedFields> = built
        .iter()
        .filter(|b| b.class == ISSUE_FROM_FILENAME)
        .map(|b| {
            (
                (b.bundle.clone(), b.title.clone()),
                (b.row.series.clone(), b.row.volume, b.row.issue, b.row.subtitle.clone()),
            )
        })
        .collect();
    let mut propagated = 0;
    for b in built.iter_mut() {
        if b.class == ISSUE_FROM_FILENAME {
            continue;
        }
        let Some((series, volume, issue, subtitle)) =
            derived.get(&(b.bundle.clone(), b.title.clone()))
        else {
            continue;
        };
        b.row.series = series.clone();
        b.row.volume = *volume;
        b.row.issue = *issue;
        b.row.subtitle = subtitle.clone();
        b.row.validate().map_err(|e| format!("{:?}: {e}", b.title))?;
        b.class = ISSUE_FROM_SIBLING;
        propagated += 1;
    }
    Ok(propagated)
}

/// Anything still disagreeing per title after that, reported as its own class
/// rather than left for a reader to notice. Zero today; the check is what
/// makes that a fact rather than an assumption.
fn flag_disagreeing_siblings(built: &mut [Built]) {
    let mut shapes: BTreeMap<(String, String), BTreeSet<ParsedFields>> = BTreeMap::new();
    for b in built.iter() {
        shapes.entry((b.bundle.clone(), b.title.clone())).or_default().insert((
            b.row.series.clone(),
            b.row.volume,
            b.row.issue,
            b.row.subtitle.clone(),
        ));
    }
    for b in built.iter_mut() {
        if shapes[&(b.bundle.clone(), b.title.clone())].len() > 1 {
            b.class = SIBLINGS_DISAGREE;
        }
    }
}

/// A row and what `seed` decided about it, held until every row exists: the
/// per-title checks below cannot run inside the per-row loop.
struct Built {
    title: String,
    stem: String,
    row: MappingRow,
    /// The bundle this candidate came from. Not a field of `MappingRow` since
    /// D64 — the shipped table has no such column — but the generator still
    /// needs it: sibling detection groups on `(bundle, title)`, and the review
    /// page identifies its 2,204 stored decisions by `(key, bundle)`, so the
    /// sidecar and `target/seed/humble.tsv` keep theirs (spec §2.1).
    bundle: String,
    class: &'static str,
}

/// The four fields a decomposition is, for comparing two rows of one title.
type ParsedFields = (String, Option<u32>, Option<u32>, Option<String>);

/// Neither is a known-bad class: the title said so, or the filename did.
/// Kept apart because they are different evidence — the first is written in
/// the verified title, the second is an inference D61 bounds.
const ISSUE_IN_TITLE: &str = "issue written in the verified title (#N)";
const ISSUE_FROM_FILENAME: &str = "issue taken from the filename (D61)";
/// The same verified title in the same bundle, decomposed from a sibling
/// whose filename corroborated it.
const ISSUE_FROM_SIBLING: &str = "issue copied from a sibling of the same title";
/// Still disagreeing after that. A known-bad class: one title must not
/// produce two decompositions, or one comic splits across two series folders.
const SIBLINGS_DISAGREE: &str = "siblings of one title decomposed differently";

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

/// No `bundle` parameter since D64: the field is gone from `MappingRow`, and
/// with it the only thing that ever validated a folder name. Nothing stores one
/// now — `Built` carries the bundle for the generator's own grouping, Layer B
/// uses it as a map key, and `branch_for` slugifies it.
fn build_row(
    title: &str,
    stem: &str,
    confirmed: Option<&str>,
    corpus_series: Option<(String, u32)>,
) -> Result<MappingRow, String> {
    let key = key(stem).ok_or_else(|| format!("{stem:?} has no key"))?;
    let parsed = parse_title(&strip_promo(title));
    // Either source replaces the split whole. The fifth column drops the
    // parser's volume with it, because that volume came from reading the
    // title the other way; the corpus supplies its own.
    let (series, subtitle, volume, issue) = match (confirmed, corpus_series) {
        // A keystroke outranks a lookup, and the fifth column is a keystroke.
        (Some(c), _) => {
            let (s, sub) = confirmed_split(&strip_promo(title), c)?;
            // The fifth column confirms the *series*. Whatever follows it is
            // still a remainder that decomposes, and a trailing `#N` left
            // sitting in the subtitle would keep exactly the shape D60
            // replaced — unpadded, with a `#`, sorting wrongly outside Komga.
            let (sub, issue) = take_hash_issue(sub);
            (s, sub, None, issue)
        }
        // The stem is the whole title bar the marker, so nothing is left over.
        (None, Some((series, volume))) => (series, None, Some(volume), None),
        (None, None) => {
            let mut series = parsed.series;
            let mut subtitle = parsed.subtitle;
            let mut issue = parsed.issue;
            // Only one of the two can fire: the parser takes a `#N`, and
            // `issue_from_stem` refuses any title carrying a `#`.
            if issue.is_none() {
                if let Some(n) = issue_from_stem(stem, &strip_promo(title)) {
                    // The title's trailing number sits in the last field with
                    // content: the subtitle where there is one, else the
                    // series. Taking it from the series is the case that
                    // matters — `Bee and PuppyCat 1` gave every issue its own
                    // series folder.
                    let taken = match subtitle.clone() {
                        Some(sub) => match split_trailing_number(&sub) {
                            Some((rest, _)) => {
                                subtitle = (!rest.is_empty()).then_some(rest.to_string());
                                true
                            }
                            None => false,
                        },
                        None => match split_trailing_number(&series) {
                            // Never empty the series: that row has no
                            // destination, and `validate` refuses it.
                            Some((rest, _)) if !rest.is_empty() => {
                                series = rest.to_string();
                                true
                            }
                            _ => false,
                        },
                    };
                    if taken {
                        issue = Some(n);
                    }
                }
            }
            (series, subtitle, parsed.volume, issue)
        }
    };
    let row = MappingRow {
        key,
        series,
        volume,
        issue,
        subtitle,
        lang: LANG.to_string(),
        volume_count: None,
    };
    row.validate().map_err(|e| format!("{title:?}: {e}"))?;
    Ok(row)
}

/// Not a known-bad class: these rows were decided by the corpus, not guessed.
const CORPUS_CONFIRMED: &str = "corpus confirms the series (colon kept, volume recovered)";

/// A series carrying no lowercase letter at all. Humble typesets some of its
/// own titles in full caps (`BLACK SCIENCE, Vol. 6`) and the parser keeps the
/// casing it is handed, so the row is well-formed and still wrong.
///
/// The casing is then settled by `settle_casing`, which asks the corpus first.
/// This class survives only on the rows it could not answer for, where the
/// series carries a guess instead.
const SERIES_ALL_CAPS: &str = "series casing guessed, corpus silent";

/// The corpus recorded the name, so nobody has to rule on its casing: either
/// it spells it in mixed case and the row took that spelling, or it shouts the
/// same way and the row was right as it stood.
const CORPUS_CASING: &str = "corpus settled the series casing";

/// What the corpus records for a shouting series.
enum Casing {
    /// The corpus spells it in capitals too, so the row needs no change.
    Shouts,
    /// The corpus spells it in mixed case, and this is that spelling.
    Mixed(String),
}

/// The corpus's own spelling of `series`, when it has exactly one.
///
/// Accepts a name only when it differs from `series` by letter case alone.
/// Squashed keys alone are too loose: they match `Fear Agent: Final Edition`
/// to `FEAR AGENT FINAL EDITION`, `D'Orc` to `D ORC` and `Id` to `I.D.`. The
/// first two insert punctuation the verified title does not have, and the last
/// is a different series. Case alone keeps every real win -- `Blame!`,
/// `Ody-C`, `W0rldtr33`, `Seven to Eternity` -- and lets a reader handle the
/// rest.
///
/// Two mixed-case spellings that disagree answer nothing, so they count as no
/// answer. Measured over the 2,223 generated rows: none disagree.
fn corpus_casing(series: &str, corpus: &Corpus) -> Option<Casing> {
    let key = squash(series);
    if key.is_empty() {
        return None;
    }
    let upper = series.to_uppercase();
    let names: BTreeSet<&str> = corpus
        .lookup(&key)
        .iter()
        .filter(|r| r.language == LANG && r.name.to_uppercase() == upper)
        .map(|r| r.name.as_str())
        .collect();
    if names.contains(series) {
        return Some(Casing::Shouts);
    }
    let mut mixed = names.into_iter().filter(|n| !is_shouting(n));
    match (mixed.next(), mixed.next()) {
        (Some(one), None) => Some(Casing::Mixed(one.to_string())),
        _ => None,
    }
}

/// A token to leave exactly as it came: it holds a digit or a dot, or has no
/// vowel. `DLX`, `SFSX`, `C.O.W.L.` and `W0RLDTR33` are initialisms or
/// deliberate orthography rather than words, and a change of case invents a
/// spelling nobody uses.
fn is_styled(token: &str) -> bool {
    let lower: String = token.chars().flat_map(char::to_lowercase).collect();
    token.chars().any(|c| c.is_ascii_digit() || c == '.')
        || !lower.chars().any(|c| "aeiouy".contains(c))
}

/// Words that stay lowercase inside a title.
const SMALL_WORDS: &[&str] = &[
    "a", "an", "and", "as", "at", "but", "by", "for", "from", "in", "nor", "of", "on", "or", "so",
    "the", "to", "up", "via", "vs", "with",
];

/// Title-case for a shouting series the corpus does not record.
///
/// `is_styled` decides which tokens survive untouched. The first and
/// last tokens that carry a letter always capitalise, so a title ending in
/// `UP` does not end in `up`.
fn titlecase(series: &str) -> String {
    let tokens: Vec<&str> = series.split(' ').collect();
    let lettered: Vec<usize> = (0..tokens.len())
        .filter(|&i| tokens[i].chars().any(char::is_alphabetic))
        .collect();
    let (first, last) = match (lettered.first(), lettered.last()) {
        (Some(&f), Some(&l)) => (f, l),
        _ => return series.to_string(),
    };
    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    for (i, token) in tokens.iter().enumerate() {
        let lower: String = token.chars().flat_map(char::to_lowercase).collect();
        out.push(if is_styled(token) {
            (*token).to_string()
        } else if SMALL_WORDS.contains(&lower.as_str()) && i != first && i != last {
            lower
        } else {
            let mut chars = lower.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().chain(chars).collect(),
                None => lower,
            }
        });
    }
    out.join(" ")
}

/// Settles the casing of a shouting series and says who decided it.
///
/// Runs after `classify`, and only on the rows `classify` had nothing else to
/// say about: a row flagged for what the parser got wrong keeps that flag and
/// its own casing, because its reader is going to retype the series anyway.
/// Seven of the 188 shouting rows are in that position.
///
/// Recasing cannot break §6.11's per-field rules, so the row is not
/// re-validated: both branches change letter case only, and neither the
/// 120-byte cap nor the forbidden characters can be crossed by a change of
/// case.
fn settle_casing(row: &mut MappingRow, class: &'static str, corpus: Option<&Corpus>) -> &'static str {
    if class != SERIES_ALL_CAPS {
        return class;
    }
    let (class, settled) = casing_outcome(&row.series, corpus.and_then(|c| corpus_casing(&row.series, c)));
    if let Some(series) = settled {
        row.series = series;
    }
    class
}

/// The decision itself, with the lookup already done, so every branch is
/// reachable from a test: the corpus release is behind
/// `COMIC_RENAMER_TEST_ZIP` and `cargo xtask seed` is the only other thing
/// that reaches these arms.
fn casing_outcome(series: &str, found: Option<Casing>) -> (&'static str, Option<String>) {
    match found {
        // Agreement needs no veto: the corpus spells it in capitals too, so
        // there is nothing for a reader to rule on. `B.P.R.D.` is 20 of these
        // rows and sending them back would be the opposite of help.
        Some(Casing::Shouts) => (CORPUS_CASING, None),
        // A styled token is orthography rather than casing. The corpus holds
        // `Bttm Fdrs` for `BTTM FDRS` and `W0rldtr33` for `W0RLDTR33` -- its
        // house style applied to names their covers spell in capitals on
        // purpose, which is a question nobody asked. Four rows, and the
        // reader keeps them as they are.
        Some(Casing::Mixed(_)) if series.split(' ').any(is_styled) => (SERIES_ALL_CAPS, None),
        Some(Casing::Mixed(name)) => (CORPUS_CASING, Some(name)),
        None => (SERIES_ALL_CAPS, Some(titlecase(series))),
    }
}

/// Alphabetic and nowhere lowercase. Digits and punctuation decide nothing, so
/// `W0RLDTR33` shouts and `1948` does not.
fn is_shouting(series: &str) -> bool {
    series.chars().any(char::is_alphabetic) && !series.chars().any(char::is_lowercase)
}

/// The known-bad classes, from what the parser is known to get wrong. First
/// match wins; `clean` means none of them fired, not that a person checked.
///
/// `SERIES_ALL_CAPS` comes last, so it claims only rows no known-bad class
/// took. It is a property of the source's casing rather than of the split, and
/// a row already in the review pile needs no second reason to be there. Two
/// rows escape it for the same reason -- `C.O.W.L. #1` and `ODY-C #1` carry an
/// issue, so `classify` never sees them -- and both are correct as they stand.
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
    } else if is_shouting(&row.series) {
        SERIES_ALL_CAPS
    } else {
        "clean"
    }
}

#[cfg(test)]
mod issue_tests {
    use super::*;

    #[test]
    fn a_filename_that_corroborates_the_trailing_number_gives_an_issue() {
        assert_eq!(issue_from_stem("beeandpuppycat_issue1_1406925491", "Bee and PuppyCat 1"), Some(1));
        assert_eq!(issue_from_stem("protocol_orphans_issue4_1406925491", "Protocol: Orphans 4"), Some(4));
        assert_eq!(issue_from_stem("Robocop_issue2_1406925491", "RoboCop 2"), Some(2));
        assert_eq!(issue_from_stem("rai-issue3_1410824697", "Rai 3"), Some(3));
    }

    #[test]
    fn a_bare_trailing_number_with_no_corroborating_filename_is_left_alone() {
        // D61: the corpus confirms some of these as whole titles, so a
        // trailing digit alone is not evidence.
        assert_eq!(issue_from_stem("10dance_1", "10 Dance 1"), None);
        assert_eq!(issue_from_stem("altair_arecordofbattles_vol1", "Altair: A Record of Battles Vol. 1"), None);
        assert_eq!(issue_from_stem("elephantmen2260_bookone", "Elephantmen 2260 Book One"), None);
    }

    #[test]
    fn a_filename_issue_that_disagrees_with_the_title_is_left_alone() {
        // The filename says issue 1 and the title does not end in 1 at all.
        assert_eq!(issue_from_stem("fairyquest_issue1_1406925491", "Fairy Quest Outlaws"), None);
        assert_eq!(issue_from_stem("robocop_vol3_laststand_issue4-optimized", "RoboCop: Last Stand"), None);
        // Says 9, ends in 8.
        assert_eq!(issue_from_stem("curse_issue9", "Curse 8"), None);
    }

    #[test]
    fn a_title_humble_already_hashed_needs_no_filename_rule() {
        // `parse_title` owns these, and taking the number twice would strip
        // the series down.
        assert_eq!(issue_from_stem("archie_issue635", "Archie #635"), None);
        assert_eq!(issue_from_stem("lumberjanes_issue1_1406925491", "Lumberjanes #1"), None);
    }

    #[test]
    fn a_derived_issue_strips_the_number_from_the_series() {
        let row = build_row("Bee and PuppyCat 1", "beeandpuppycat_issue1_1406925491", None,
            None,
        )
        .unwrap();
        assert_eq!(row.series, "Bee and PuppyCat");
        assert_eq!((row.volume, row.issue), (None, Some(1)));
        assert_eq!(row.subtitle, None);
    }

    #[test]
    fn a_derived_issue_strips_the_number_from_the_subtitle_when_that_is_where_it_sits() {
        let row = build_row("Protocol: Orphans 4", "protocol_orphans_issue4_1406925491", None,
            None,
        )
        .unwrap();
        assert_eq!(row.series, "Protocol");
        assert_eq!(row.subtitle.as_deref(), Some("Orphans"));
        assert_eq!((row.volume, row.issue), (None, Some(4)));
    }

    #[test]
    fn a_keystroke_still_outranks_the_filename_rule() {
        // D50/D51: the fifth column is a confirmed series and replaces the
        // split whole, so the rule must not reach past it.
        let row = build_row("Bee and PuppyCat 1", "beeandpuppycat_issue1_1406925491", Some("Bee and PuppyCat 1"),
            None,
        )
        .unwrap();
        assert_eq!(row.series, "Bee and PuppyCat 1");
        assert_eq!(row.issue, None);
    }
}

#[cfg(test)]
mod confirmed_issue_tests {
    use super::*;

    /// A confirmed series does not make the leftover exempt from D60.
    #[test]
    fn a_confirmed_series_still_gives_up_a_trailing_hash_issue() {
        let row = build_row("DIE: Loaded #1", "die-loaded-issue-1", Some("DIE: Loaded"),
            None,
        )
        .unwrap();
        assert_eq!(row.series, "DIE: Loaded", "the keystroke still owns the series");
        assert_eq!((row.volume, row.issue), (None, Some(1)));
        assert_eq!(row.subtitle, None, "nothing is left of the leftover");
    }

    #[test]
    fn a_confirmed_series_keeps_a_real_subtitle_beside_the_issue() {
        let row = build_row("Glarien: Origins The Dig #2", "glarien-origins-issue-2", Some("Glarien: Origins"),
            None,
        )
        .unwrap();
        assert_eq!(row.series, "Glarien: Origins");
        assert_eq!(row.subtitle.as_deref(), Some("The Dig"));
        assert_eq!(row.issue, Some(2));
    }
}

#[cfg(test)]
mod take_hash_issue_tests {
    use super::*;

    /// Its only coverage was through `build_row` on the two marketplace rows
    /// that carry a fifth column. Both divergences below go live the moment
    /// a third such entry is added, so they are pinned before that happens.
    #[test]
    fn a_plain_trailing_hash_number_is_taken() {
        assert_eq!(take_hash_issue(Some("#1".into())), (None, Some(1)));
        assert_eq!(
            take_hash_issue(Some("The Dig #2".into())),
            (Some("The Dig".into()), Some(2))
        );
    }

    #[test]
    fn a_volume_written_with_a_hash_is_not_an_issue() {
        // `take_trailing_issue` in core declines exactly this shape. Without
        // the same guard here the confirmed arm turned a volume marker into
        // an issue and stranded "Vol." in the subtitle — and that arm drops
        // the volume, so the number would have been lost entirely.
        assert_eq!(
            take_hash_issue(Some("Vol. #3".into())),
            (Some("Vol. #3".into()), None)
        );
        assert_eq!(
            take_hash_issue(Some("Volume #12".into())),
            (Some("Volume #12".into()), None)
        );
    }

    #[test]
    fn a_number_past_the_ceiling_is_left_as_text() {
        // `MappingRow::validate` refuses an issue above 9999, which made
        // `build_row` error and dropped the whole row out of the table —
        // worse than the subtitle it used to ship with.
        assert_eq!(
            take_hash_issue(Some("#99999".into())),
            (Some("#99999".into()), None)
        );
        assert_eq!(take_hash_issue(Some("#9999".into())), (None, Some(9999)));
    }

    #[test]
    fn anything_that_is_not_a_trailing_bare_number_is_untouched() {
        for text in ["Issue #3 Special", "#", "#-1", "#1.5", "#1-6", "#4294967296", "Orphans"] {
            assert_eq!(
                take_hash_issue(Some(text.into())),
                (Some(text.into()), None),
                "{text:?} is not a trailing issue number"
            );
        }
        assert_eq!(take_hash_issue(None), (None, None));
    }
}

#[cfg(test)]
mod dedup_tests {
    use super::*;

    fn row(issue: Option<u32>) -> MappingRow {
        MappingRow {
            key: "k".into(),
            series: "S".into(),
            volume: None,
            issue,
            subtitle: None,
            lang: LANG.to_string(),
            volume_count: None,
        }
    }

    /// The de-duplication tuple must carry every semantic field, or a pair
    /// that *disagrees* is folded away before `find_key_collisions` — the
    /// gate whose whole job is to refuse exactly that pair — ever runs.
    /// Adding `issue` to the row broke that property silently: no fixture
    /// pair exercises it today, and the next generator change could.
    #[test]
    fn two_rows_differing_only_by_issue_are_not_one_row() {
        let b = "B";
        assert_ne!(dedup_identity(&row(Some(1)), b), dedup_identity(&row(Some(2)), b));
        assert_ne!(dedup_identity(&row(None), b), dedup_identity(&row(Some(1)), b));
        assert_eq!(dedup_identity(&row(Some(1)), b), dedup_identity(&row(Some(1)), b));
        // Still per bundle, because this folds the generator's own repeats and
        // its input really has bundles (spec §2.1).
        assert_ne!(dedup_identity(&row(Some(1)), "A"), dedup_identity(&row(Some(1)), "B"));
    }

    /// And a gate still sees them, which is what makes surviving the fold
    /// worth anything. Two gates now: the generator's own, which is per
    /// bundle, and the shipped table's, which is per key (D64).
    #[test]
    fn a_pair_that_disagrees_on_the_issue_is_still_reported() {
        let pair = [(row(Some(1)), "B".to_string()), (row(Some(2)), "B".to_string())];
        assert!(!disagreeing_within_a_bundle(&pair).is_empty());
        assert!(!find_key_collisions(&[row(Some(1)), row(Some(2))]).is_empty());
    }
}

#[cfg(test)]
mod shouting_tests {
    use super::*;

    fn row(series: &str, volume: Option<u32>, subtitle: Option<&str>) -> MappingRow {
        MappingRow {
            key: key("x").unwrap(),
            series: series.into(),
            volume,
            issue: None,
            subtitle: subtitle.map(str::to_string),
            lang: LANG.to_string(),
            volume_count: None,
        }
    }

    #[test]
    fn a_series_with_no_lowercase_letter_asks_for_a_decision() {
        assert!(is_shouting("BLACK SCIENCE"));
        assert!(is_shouting("W0RLDTR33"), "digits decide nothing");
        assert!(is_shouting("C.O.W.L."));
        assert!(!is_shouting("Black Science"));
        assert!(!is_shouting("RoboCop"), "one lowercase letter is enough");
        assert!(!is_shouting("1948"), "no letter at all is not shouting");
        assert!(!is_shouting(""));
    }

    #[test]
    fn classify_routes_a_shouting_series_out_of_clean() {
        let shouts = row("BLACK SCIENCE", Some(6), None);
        let does_not = row("Black Science", Some(6), None);
        assert_eq!(classify("BLACK SCIENCE, Vol. 6", &shouts), SERIES_ALL_CAPS);
        assert_eq!(classify("Black Science, Vol. 6", &does_not), "clean");
    }

    #[test]
    fn titlecase_lowercases_the_small_words_but_never_the_ends() {
        assert_eq!(titlecase("SEVEN TO ETERNITY"), "Seven to Eternity");
        assert_eq!(titlecase("DEPARTMENT OF TRUTH"), "Department of Truth");
        assert_eq!(titlecase("RIGHTEOUS THIRST FOR VENGEANCE"), "Righteous Thirst for Vengeance");
        assert_eq!(titlecase("THE LAST DAYS OF AMERICAN CRIME"), "The Last Days of American Crime");
        // First and last win over the small-word list.
        assert_eq!(titlecase("OF MICE AND"), "Of Mice And");
    }

    #[test]
    fn titlecase_leaves_a_styled_token_alone() {
        assert_eq!(titlecase("SFSX"), "SFSX", "no vowel, so an initialism");
        assert_eq!(titlecase("W0RLDTR33"), "W0RLDTR33", "a digit, so a styling");
        assert_eq!(titlecase("C.O.W.L."), "C.O.W.L.", "dots, so an initialism");
        assert_eq!(titlecase("EIGHT BILLION GENIES DLX ED"), "Eight Billion Genies DLX Ed");
        assert_eq!(titlecase("KILL 6 BILLION DEMONS"), "Kill 6 Billion Demons");
    }

    /// With no corpus the row keeps the flag and carries a guess, which is the
    /// whole contract: a guess is a typing aid, never a stored fact (D50).
    #[test]
    fn with_no_corpus_the_casing_is_a_guess_and_stays_flagged() {
        let mut r = row("DEADLY CLASS", Some(10), None);
        assert_eq!(settle_casing(&mut r, SERIES_ALL_CAPS, None), SERIES_ALL_CAPS);
        assert_eq!(r.series, "Deadly Class");
    }

    #[test]
    fn the_corpus_spelling_wins_when_it_is_only_a_change_of_case() {
        let got = casing_outcome("BLACK SCIENCE", Some(Casing::Mixed("Black Science".into())));
        assert_eq!(got, (CORPUS_CASING, Some("Black Science".to_string())));
    }

    /// The corpus shouting back is an answer, not a silence: these rows leave
    /// the review pile without a change.
    #[test]
    fn a_corpus_that_shouts_too_settles_the_row_unchanged() {
        assert_eq!(casing_outcome("B.P.R.D.", Some(Casing::Shouts)), (CORPUS_CASING, None));
    }

    /// Orthography is not casing. Without this the generator quietly respells
    /// two real titles and the reader never sees them.
    #[test]
    fn a_styled_token_keeps_the_corpus_spelling_out() {
        for (series, corpus_name) in
            [("BTTM FDRS", "Bttm Fdrs"), ("W0RLDTR33", "W0rldtr33"), ("I.D.", "Id")]
        {
            assert_eq!(
                casing_outcome(series, Some(Casing::Mixed(corpus_name.into()))),
                (SERIES_ALL_CAPS, None),
                "{series}"
            );
        }
    }

    /// A row flagged for something else keeps both its class and its casing,
    /// so the reason the parser failed is still on the card.
    #[test]
    fn settle_casing_leaves_another_class_untouched() {
        let mut r = row("SONS OF ANARCHY 10", None, None);
        let bare = "bare trailing number (not a marker)";
        assert_eq!(settle_casing(&mut r, bare, None), bare);
        assert_eq!(r.series, "SONS OF ANARCHY 10");
    }

    /// Last arm, so a row already bound for the review pile keeps the class
    /// that says what the parser got wrong. Without the ordering, a shouting
    /// series would mask the reason.
    #[test]
    fn a_known_bad_class_outranks_the_casing() {
        assert_eq!(
            classify("SONS OF ANARCHY 10", &row("SONS OF ANARCHY 10", None, None)),
            "bare trailing number (not a marker)"
        );
        assert_eq!(
            classify("RESIST!: TALES VOL 2", &row("RESIST!", None, Some("TALES VOL 2"))),
            "volume marker swallowed into the subtitle (colon came first)"
        );
    }
}

#[cfg(test)]
mod sibling_tests {
    use super::*;

    fn built(stem: &str, title: &str, series: &str, issue: Option<u32>, class: &'static str) -> Built {
        Built {
            title: title.into(),
            stem: stem.into(),
            row: MappingRow {
                key: key(stem).unwrap(),
                series: series.into(),
                volume: None,
                issue,
                subtitle: None,
                lang: LANG.to_string(),
                volume_count: None,
            },
            // Sibling detection groups on `(bundle, title)`, so these helpers
            // share one bundle: the tests are about titles, not bundles.
            bundle: "B".to_string(),
            class,
        }
    }

    /// The real shape, from the BOOM! bundle: three files of one comic, and
    /// only the `issue<N>` spelling corroborates. Left alone, one comic lands
    /// in two Komga series folders — `Hacktivist/Hacktivist 0001.cbz` beside
    /// `Hacktivist 1/Hacktivist 1.cbz` — and nothing reports it, because the
    /// paths differ and so do the folder collision keys.
    #[test]
    fn a_sibling_of_the_same_title_takes_the_derived_decomposition() {
        let mut rows = vec![
            built("hacktivist0011407188013", "Hacktivist 1", "Hacktivist 1", None, "bare"),
            built("hacktivist0011407270741", "Hacktivist 1", "Hacktivist 1", None, "bare"),
            built("hacktivistissue11406925491", "Hacktivist 1", "Hacktivist", Some(1), ISSUE_FROM_FILENAME),
        ];
        assert_eq!(propagate_within_titles(&mut rows).unwrap(), 2);
        for b in &rows {
            assert_eq!(b.row.series, "Hacktivist", "{}", b.stem);
            assert_eq!(b.row.issue, Some(1), "{}", b.stem);
        }
        assert_eq!(rows[0].class, ISSUE_FROM_SIBLING);
        assert_eq!(rows[2].class, ISSUE_FROM_FILENAME, "the source keeps its own class");
    }

    /// Only within one title. A different title is a different comic, however
    /// similar its stem.
    #[test]
    fn a_different_title_is_not_a_sibling() {
        let mut rows = vec![
            built("hacktivist0021407188013", "Hacktivist 2", "Hacktivist 2", None, "bare"),
            built("hacktivistissue11406925491", "Hacktivist 1", "Hacktivist", Some(1), ISSUE_FROM_FILENAME),
        ];
        assert_eq!(propagate_within_titles(&mut rows).unwrap(), 0);
        assert_eq!(rows[0].row.series, "Hacktivist 2");
        assert_eq!(rows[0].row.issue, None);
    }

    #[test]
    fn a_title_whose_rows_still_disagree_is_reported() {
        let mut rows = vec![
            built("a", "Curse 1", "Curse", Some(1), "clean"),
            built("b", "Curse 1", "Curse 1", None, "clean"),
        ];
        flag_disagreeing_siblings(&mut rows);
        assert!(rows.iter().all(|b| b.class == SIBLINGS_DISAGREE), "{:?}", rows[0].class);
    }

    #[test]
    fn a_title_whose_rows_agree_keeps_its_class() {
        let mut rows = vec![
            built("a", "Curse 1", "Curse", Some(1), "clean"),
            built("b", "Curse 1", "Curse", Some(1), "clean"),
        ];
        flag_disagreeing_siblings(&mut rows);
        assert!(rows.iter().all(|b| b.class == "clean"));
    }
}
