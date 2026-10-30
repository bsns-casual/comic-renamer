use comic_renamer_core::mapping::{find_key_collisions, key, MappingRow, Mappings};
use std::path::Path;

const HEADER: &str = "key\tseries\tvolume\tissue\tsubtitle\tlang\tvolume_count\n";

fn tsv(rows: &[(&str, &str)]) -> String {
    let mut s = HEADER.to_string();
    for (k, series) in rows {
        s.push_str(&format!("{k}\t{series}\t1\t\t\ten\t\n"));
    }
    s
}

fn row(k: &str, series: &str) -> MappingRow {
    MappingRow {
        key: k.into(),
        series: series.into(),
        volume: Some(1),
        issue: None,
        subtitle: None,
        lang: "en".into(),
        volume_count: None,
    }
}

fn humble_rows() -> Vec<MappingRow> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/humble-ground-truth.tsv");
    let text = std::fs::read_to_string(path).unwrap();
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            Some(row(&key(f.get(2)?)?, f[1]))
        })
        .collect()
}

/// Identity is the key alone (D64). There is no second argument to match on and
/// nothing a caller can get wrong.
#[test]
fn a_key_is_the_whole_identity() {
    let m = Mappings::load(&tsv(&[("alpha", "Alpha")]), None).unwrap();
    assert_eq!(m.lookup("alpha").unwrap().series, "Alpha");
    assert!(m.lookup("missing").is_none());
}

/// This replaces `two_rows_for_one_key_are_disambiguated_by_bundle` and its
/// `lookup("alpha", "c").is_none(), "never silently pick"`.
///
/// Both are gone for the same reason: there is nothing left to pick between.
/// The old behaviour was that a key carrying two rows resolved only for a user
/// whose folder name matched one of them — measured over every decision this
/// project holds, 158 of 2,043 keys were in that state, agreeing on every field,
/// and so resolved for the contributor and for nobody else. A second row is now
/// refused when the layer loads, which is the same refusal moved to where
/// somebody can act on it.
#[test]
fn a_second_row_for_one_key_is_refused_at_load_not_disambiguated() {
    let two = tsv(&[("alpha", "One"), ("alpha", "Two")]);
    let msg = Mappings::load(&two, None).err().expect("two rows for one key").to_string();
    assert!(msg.contains("alpha"), "{msg}");
    assert!(msg.contains("line 3"), "pointing at the row that would be ignored: {msg}");
    assert!(msg.contains("correct the first row"), "and saying what to do: {msg}");

    // An exact repeat is refused too, where it used to be tolerated: under one
    // row per key a duplicate is a mistake whatever its fields say, and a
    // correction is an edit to the existing line.
    assert!(Mappings::load(&tsv(&[("alpha", "A"), ("alpha", "A")]), None).is_err());
}

#[test]
fn local_rows_outrank_shipped_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("local.tsv");
    std::fs::write(&path, tsv(&[("alpha", "Fixed")])).unwrap();
    let shipped = tsv(&[("alpha", "One"), ("beta", "Beta")]);
    let m = Mappings::load(&shipped, Some(&path)).unwrap();
    assert_eq!(m.lookup("alpha").unwrap().series, "Fixed", "the local row wins");
    assert_eq!(m.lookup("beta").unwrap().series, "Beta", "and does not displace others");
    assert_eq!(m.len(), 2);
}

#[test]
fn an_unreadable_local_file_is_an_error_not_a_silent_skip() {
    let err = Mappings::load(&tsv(&[]), Some(Path::new("/nonexistent/x.tsv")));
    assert!(err.is_err(), "the local file holds the user's own corrections");
    assert!(Mappings::load(&tsv(&[]), None).is_ok());
}

#[test]
fn a_malformed_or_invalid_local_file_is_an_error_naming_its_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("local.tsv");
    std::fs::write(&path, "not a header\n").unwrap();
    assert!(Mappings::load(&tsv(&[]), Some(&path)).is_err());
    std::fs::write(&path, tsv(&[("alpha", "Ok"), ("beta", "   ")])).unwrap();
    let msg = Mappings::load(&tsv(&[]), Some(&path)).err().unwrap().to_string();
    assert!(msg.contains("line 3"), "{msg}");
    assert!(msg.contains("local"), "{msg}");
}

#[test]
fn load_validates_every_shipped_row() {
    let msg = Mappings::load(&tsv(&[("alpha", "Ok"), ("Bad Key", "X")]), None)
        .err()
        .unwrap()
        .to_string();
    assert!(msg.contains("line 3"), "{msg}");
    assert!(Mappings::load(&tsv(&[("alpha", "a\u{200b}b")]), None).is_err());
}

/// `collisions_ignore_rows_that_merely_repeat_themselves` was here and it
/// asserted the ground-truth fixture held none. It does hold them — **177 of
/// its 2,036 keys appear more than once**, because the fixture is organised by
/// bundle and one comic is sold in several. The old assertion passed only
/// because `(key, bundle)` was the identity and repeats agreed on every field.
///
/// Under D64 that is exactly what a collision is, so the fixture is the wrong
/// input for the shipped table's gate and the right input for measuring how
/// much the old identity was costing. 177 keys here; 158 over the decided
/// table, where the review collapsed some.
#[test]
fn the_ground_truth_fixture_repeats_keys_across_bundles() {
    let rows = humble_rows();
    assert_eq!(rows.len(), 2223, "fixture read");
    let repeated = find_key_collisions(&rows);
    assert_eq!(repeated.len(), 177, "measured 2026-10-30");

    // Every one of them agrees on every field, which is why `bundle` never
    // disambiguated anything and why dropping it discards nothing.
    for k in &repeated {
        let same: Vec<&MappingRow> = rows.iter().filter(|r| &r.key == k).collect();
        assert!(same.windows(2).all(|w| w[0] == w[1]), "{k} disagrees: {same:?}");
    }
}

#[test]
fn collisions_report_every_repeated_key_once() {
    let mut other = row("k", "S");
    other.volume = Some(2);
    let rows = [row("k", "S"), other, row("j", "T"), row("j", "U"), row("m", "M")];
    assert_eq!(find_key_collisions(&rows), vec!["j".to_string(), "k".to_string()]);
}

/// `find_key_collisions` existed and was run only by CI, so a file nobody
/// checked loaded both rows and `lookup` returned the first — a user's later
/// correction in their own file was ignored with no sign of it.
#[test]
fn a_layer_holding_two_rows_for_one_key_fails_to_load() {
    let both = tsv(&[("alpha", "First"), ("alpha", "Second")]);
    let msg = Mappings::load(&both, None).err().expect("two rows").to_string();
    assert!(msg.contains("alpha"), "{msg}");
    assert!(msg.contains("line 3"), "the row that would be ignored: {msg}");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("local.tsv");
    std::fs::write(&path, &both).unwrap();
    let msg = Mappings::load(&tsv(&[]), Some(&path)).err().expect("the local layer").to_string();
    assert!(msg.contains("local mappings"), "{msg}");
}

/// `a_local_row_shadows_its_shipped_siblings` and
/// `a_bundles_case_does_not_make_a_second_bundle` both lived here and both
/// retire with the `bundle` column (D64, spec §2.2).
///
/// The first described the cost of a local row replacing *every* shipped row
/// for its key. A key cannot hold several rows now, so the cost does not exist.
/// The second pinned that `Fanatical` and `fanatical` are one bundle, which was
/// a real bug fixed on 2026-10-23 — and the field it protected is gone, so
/// there is no bundle comparison left to get wrong. **Invariant 7's path half
/// is untouched**: `preset_komga.rs` and `fsops.rs` still pin that collision
/// comparison over emitted paths is NFC-then-fold, because `Hellboy/` and
/// `hellboy/` are still one directory on APFS.
///
/// What replaces both is `a_second_row_for_one_key_is_refused_at_load_not_disambiguated`
/// above: the single rule that makes either question unanswerable.
#[test]
fn the_frozen_key_still_decides_identity_over_the_real_fixture() {
    let rows = humble_rows();
    assert!(rows.len() > 2000, "the fixture loaded: {}", rows.len());
    // Every row's key is what `key()` makes of its own stem, which is the one
    // thing identity now rests on.
    for r in &rows {
        assert_eq!(key(&r.key).as_deref(), Some(r.key.as_str()), "{:?}", r.key);
    }
}
