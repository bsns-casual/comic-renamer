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

use comic_renamer_core::mapping::{find_key_collisions, key, to_tsv, MappingRow};
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
            .push(build_row(title, stem, market, series)?);
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
        assert_eq!(build_row("DIE: Loaded #1", "d", "x", None).unwrap().series, "DIE");
    }

    /// A series that is not a prefix of its own title means one of the two is
    /// wrong, and only a human knows which.
    #[test]
    fn a_confirmed_series_that_is_not_a_prefix_is_an_error() {
        assert!(build_row("DIE: Loaded #1", "d", "x", Some("Glarien")).is_err());
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
        // construction, and its 14.7% of known-bad splits are what the review
        // in a `map` session is for.
        let row = match build_row(title, stem, bundle, None) {
            Ok(row) => row,
            Err(e) => {
                // The error names the title; group by what follows it.
                let why = e.split_once("\": ").map_or(e.as_str(), |(_, w)| w).to_string();
                rejected.entry(why).or_default().push(title.to_string());
                continue;
            }
        };
        classes.entry(classify(title, &row)).or_default().push((title.to_string(), row.clone()));
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

fn build_row(
    title: &str,
    stem: &str,
    bundle: &str,
    confirmed: Option<&str>,
) -> Result<MappingRow, String> {
    let key = key(stem).ok_or_else(|| format!("{stem:?} has no key"))?;
    let parsed = parse_title(&strip_promo(title));
    // A confirmed series replaces the split whole, volume included: the
    // parser's volume came from reading the title the other way.
    let (series, subtitle, volume) = match confirmed {
        Some(c) => {
            let (s, sub) = confirmed_split(&strip_promo(title), c)?;
            (s, sub, None)
        }
        None => (parsed.series, parsed.subtitle, parsed.volume),
    };
    let row = MappingRow {
        key,
        series,
        volume,
        subtitle,
        lang: LANG.to_string(),
        volume_count: None,
        bundle: bundle.to_string(),
    };
    row.validate().map_err(|e| format!("{title:?}: {e}"))?;
    Ok(row)
}

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
