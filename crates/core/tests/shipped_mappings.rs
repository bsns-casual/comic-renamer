use comic_renamer_core::mapping::{
    find_key_collisions, key, parse_tsv, shipped_mappings, MappingRow, Mappings,
};

/// The 12 marketplace titles the maintainer confirmed one by one on
/// 2026-10-23, with the decomposition checked by hand: (marketplace, stem,
/// series, volume, subtitle).
/// `(marketplace, stem, series, volume, issue, subtitle)`. The issue column
/// is D60's: four of these rows carried `#1` as subtitle text before it
/// existed, which is the shape that sorts wrongly outside Komga.
const EXPECTED: [(&str, &str, &str, Option<u32>, Option<u32>, Option<&str>); 12] = [
    ("Fanatical", "68 Vol. 6 Last Rites", "'68", Some(6), None, Some("Last Rites")),
    ("Fanatical", "Dracula Motherfker", "Dracula, Motherf**ker!", None, None, None),
    ("Fanatical", "HackSlash Back to School", "Hack/Slash", None, None, Some("Back to School")),
    ("Fanatical", "Killadelphia Vol. 4 The End of All", "Killadelphia", Some(4), None, Some("The End of All")),
    ("Fanatical", "Moonshine Vol. 1", "Moonshine", Some(1), None, None),
    ("Global Comix", "hammer-head-1_release-0001_978-1-5458-3063-5", "Hammer Head", None, Some(1), None),
    ("Global Comix", "toxic-man_release-0001", "Toxic Man", None, None, None),
    // Its own series, not an arc within `DIE` (maintainer's ruling,
    // 2026-10-24). The colon is part of the name, which is why the fixture
    // carries a confirmed series and the parser is not asked to guess. The
    // confirmed series owns the series only: the `#1` after it still becomes
    // an issue, or it would keep a shape D60 replaced.
    ("Sweet Shop", "die-loaded-issue-1", "DIE: Loaded", None, Some(1), None),
    ("Sweet Shop", "glarien-origins-issue-1", "Glarien: Origins", None, Some(1), None),
    ("Sweet Shop", "minotaur-issue-1", "Minotaur", None, Some(1), None),
    ("Sweet Shop", "the-spawn-of-venus-and-other-stories-anthology", "The Spawn of Venus and Other Stories", None, None, None),
    ("Sweet Shop", "young-shadow-the-watchdogs-graphicnovel", "Young Shadow", None, None, Some("The Watchdogs")),
];

fn all_rows() -> Vec<MappingRow> {
    parse_tsv(&shipped_mappings()).unwrap()
}

#[test]
fn every_shipped_row_validates_and_no_key_collides() {
    let rows = all_rows();
    assert!(rows.len() >= EXPECTED.len());
    for r in &rows {
        r.validate().unwrap_or_else(|e| panic!("{}: {e}", r.key));
    }
    // Across all shipped files together, not per file.
    assert!(find_key_collisions(&rows).is_empty());
}

#[test]
fn the_marketplace_fixture_is_fully_covered() {
    let fixture = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/marketplace-ground-truth.tsv"),
    )
    .unwrap();
    let stems: Vec<(&str, &str)> = fixture
        .lines()
        .skip(1)
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            (f[0], f[2])
        })
        .collect();
    assert_eq!(stems.len(), EXPECTED.len());

    let m = Mappings::load(&shipped_mappings(), None).unwrap();
    for ((market, stem), (e_market, e_stem, series, volume, issue, subtitle)) in
        stems.iter().zip(EXPECTED)
    {
        assert_eq!((*market, *stem), (e_market, e_stem));
        let row = m
            .lookup(&key(stem).unwrap(), market)
            .unwrap_or_else(|| panic!("{stem} did not resolve"));
        assert_eq!(row.series, series, "{stem}");
        assert_eq!(row.volume, volume, "{stem}");
        assert_eq!(row.issue, issue, "{stem}");
        assert_eq!(row.subtitle.as_deref(), subtitle, "{stem}");
        assert_eq!(row.bundle, *market, "{stem}");
    }
}

#[test]
fn every_tsv_in_data_mappings_is_embedded() {
    // SHIPPED_FILES is a hand-kept list. A mapping file added to the directory
    // and not to the list would sit in the repo doing nothing, silently.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/mappings");
    let shipped = all_rows();
    let mut files = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "tsv") {
            continue;
        }
        files += 1;
        for row in parse_tsv(&std::fs::read_to_string(&path).unwrap()).unwrap() {
            assert!(shipped.contains(&row), "{} is not in SHIPPED_FILES: {}", path.display(), row.key);
        }
    }
    assert!(files >= 3);
}

/// `SHIPPED_MARKETPLACES` tells `map` which contributions have a file to be
/// appended to. A marketplace listed with no file sends a contributor to a
/// `>>` that creates an unembedded TSV, which is exactly what
/// `every_tsv_in_data_mappings_is_embedded` above refuses — measured
/// 2026-10-29 by following the printed commands literally. A file with no
/// marketplace listed is a channel that is silently closed.
#[test]
fn every_shipped_marketplace_has_a_file_and_the_reverse() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/mappings");

    let listed: std::collections::BTreeSet<String> =
        comic_renamer_core::mapping::SHIPPED_MARKETPLACES
            .iter()
            .map(|m| comic_renamer_core::mapping::file_slug(m))
            .collect();

    let on_disk: std::collections::BTreeSet<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| {
            let path = e.unwrap().path();
            (path.extension()? == "tsv")
                .then(|| path.file_stem().unwrap().to_string_lossy().into_owned())
        })
        .collect();

    assert_eq!(listed, on_disk, "SHIPPED_MARKETPLACES and data/mappings/ disagree");

    // The path each one resolves to really is the file, and an unshipped
    // marketplace really does resolve to nothing. Humble is the one that
    // matters: it is `map`'s default.
    for m in comic_renamer_core::mapping::SHIPPED_MARKETPLACES {
        let path = comic_renamer_core::mapping::shipped_mapping_path(m).expect(m);
        assert!(dir.join(path.strip_prefix("data/mappings/").unwrap()).is_file(), "{m}");
    }
    assert_eq!(comic_renamer_core::mapping::shipped_mapping_path("Humble"), None);
}

#[test]
fn shipped_files_are_embedded_not_read_from_the_cwd() {
    // include_str! bakes the text in, so a changed working directory changes
    // nothing. (Process-wide cwd is safe here: no other test in this binary
    // reads a relative path.)
    let before = shipped_mappings();
    let cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(std::env::temp_dir()).unwrap();
    let after = shipped_mappings();
    std::env::set_current_dir(cwd).unwrap();
    assert_eq!(before, after);
    assert!(!before.is_empty());
    assert_eq!(before.matches("key\tseries").count(), 1, "one header after joining");
}
