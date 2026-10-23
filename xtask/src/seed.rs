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

use super::{percent, GROUND_TRUTH, LANG, MARKETPLACE};

pub fn seed() -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut by_market: BTreeMap<String, Vec<MappingRow>> = BTreeMap::new();
    for line in MARKETPLACE.lines().skip(1) {
        let f: Vec<&str> = line.split('\t').collect();
        let [market, title, stem] = f[..3] else {
            return Err(format!("bad marketplace row {line:?}"));
        };
        by_market
            .entry(market.to_string())
            .or_default()
            .push(build_row(title, stem, market)?);
    }
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
        let row = match build_row(title, stem, bundle) {
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

fn build_row(title: &str, stem: &str, bundle: &str) -> Result<MappingRow, String> {
    let key = key(stem).ok_or_else(|| format!("{stem:?} has no key"))?;
    let parsed = parse_title(title);
    let row = MappingRow {
        key,
        series: parsed.series,
        volume: parsed.volume,
        subtitle: parsed.subtitle,
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
    } else if title.split_whitespace().any(|t| t.starts_with('#')) {
        "issue number kept (subtitle, or range dropped beside a volume)"
    } else {
        "clean"
    }
}
