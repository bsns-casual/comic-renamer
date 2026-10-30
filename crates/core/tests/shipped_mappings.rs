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
        // Resolved by key alone (D64). The marketplace is still read from the
        // fixture and still checked against `EXPECTED` above, because the
        // fixture is organised by marketplace — but it is no longer something
        // a row carries or a lookup needs.
        let row = m
            .lookup(&key(stem).unwrap())
            .unwrap_or_else(|| panic!("{stem} did not resolve"));
        assert_eq!(row.series, series, "{stem}");
        assert_eq!(row.volume, volume, "{stem}");
        assert_eq!(row.issue, issue, "{stem}");
        assert_eq!(row.subtitle.as_deref(), subtitle, "{stem}");
    }
}

/// `data/decisions.tsv` is the table, and the only one (D63). This replaces
/// `every_tsv_in_data_mappings_is_embedded` and
/// `every_shipped_marketplace_has_a_file_and_the_reverse`: with one
/// unconditional file there is no hand-kept list to drift from the directory,
/// which is what both of those existed to catch.
///
/// It still guards the thing that mattered — a TSV sitting under `data/` doing
/// nothing silently — because a second one is a file the embed does not read.
#[test]
fn the_decisions_file_is_the_one_that_ships() {
    let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
    let shipped = all_rows();

    let on_disk: Vec<std::path::PathBuf> = std::fs::read_dir(&data)
        .unwrap()
        .filter_map(|e| {
            let p = e.unwrap().path();
            (p.extension().is_some_and(|x| x == "tsv")).then_some(p)
        })
        .collect();
    assert_eq!(on_disk.len(), 1, "exactly one shipped table: {on_disk:?}");
    assert_eq!(on_disk[0].file_name().unwrap(), "decisions.tsv");

    let text = std::fs::read_to_string(&on_disk[0]).unwrap();
    let rows = parse_tsv(&text).unwrap();
    assert!(!rows.is_empty());
    for row in &rows {
        assert!(shipped.contains(row), "{} is not embedded", row.key);
    }
    assert_eq!(rows.len(), shipped.len(), "and the embed holds nothing extra");
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
