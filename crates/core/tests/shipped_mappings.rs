use comic_renamer_core::mapping::{
    find_key_collisions, key, parse_tsv, shipped_mappings, MappingRow, Mappings,
};

/// The 12 marketplace titles the maintainer confirmed one by one on
/// 2026-10-23, with the decomposition checked by hand: (marketplace, stem,
/// series, volume, subtitle).
const EXPECTED: [(&str, &str, &str, Option<u32>, Option<&str>); 12] = [
    ("Fanatical", "68 Vol. 6 Last Rites", "'68", Some(6), Some("Last Rites")),
    ("Fanatical", "Dracula Motherfker", "Dracula, Motherf**ker!", None, None),
    ("Fanatical", "HackSlash Back to School", "Hack/Slash", None, Some("Back to School")),
    ("Fanatical", "Killadelphia Vol. 4 The End of All", "Killadelphia", Some(4), Some("The End of All")),
    ("Fanatical", "Moonshine Vol. 1", "Moonshine", Some(1), None),
    ("Global Comix", "hammer-head-1_release-0001_978-1-5458-3063-5", "Hammer Head", None, None),
    ("Global Comix", "toxic-man_release-0001", "Toxic Man", None, None),
    ("Sweet Shop", "die-loaded-issue-1", "DIE", None, Some("Loaded")),
    ("Sweet Shop", "glarien-origins-issue-1", "Glarien", None, Some("Origins")),
    ("Sweet Shop", "minotaur-issue-1", "Minotaur", None, None),
    ("Sweet Shop", "the-spawn-of-venus-and-other-stories-anthology", "The Spawn of Venus and Other Stories", None, None),
    ("Sweet Shop", "young-shadow-the-watchdogs-graphicnovel", "Young Shadow", None, Some("The Watchdogs")),
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
    for ((market, stem), (e_market, e_stem, series, volume, subtitle)) in stems.iter().zip(EXPECTED) {
        assert_eq!((*market, *stem), (e_market, e_stem));
        let row = m
            .lookup(&key(stem).unwrap(), market)
            .unwrap_or_else(|| panic!("{stem} did not resolve"));
        assert_eq!(row.series, series, "{stem}");
        assert_eq!(row.volume, volume, "{stem}");
        assert_eq!(row.subtitle.as_deref(), subtitle, "{stem}");
        assert_eq!(row.bundle, *market, "{stem}");
    }
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
