use comic_renamer_core::mapping::{find_key_collisions, key, MappingRow, Mappings};
use std::path::Path;

const HEADER: &str = "key\tseries\tvolume\tsubtitle\tlang\tvolume_count\tbundle\n";

fn tsv(rows: &[(&str, &str, &str)]) -> String {
    let mut s = HEADER.to_string();
    for (k, series, bundle) in rows {
        s.push_str(&format!("{k}\t{series}\t1\t\ten\t\t{bundle}\n"));
    }
    s
}

fn row(k: &str, series: &str, bundle: &str) -> MappingRow {
    MappingRow {
        key: k.into(),
        series: series.into(),
        volume: Some(1),
        subtitle: None,
        lang: "en".into(),
        volume_count: None,
        bundle: bundle.into(),
    }
}

fn humble_rows() -> Vec<MappingRow> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/humble-ground-truth.tsv");
    let text = std::fs::read_to_string(path).unwrap();
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            Some(row(&key(f.get(2)?)?, f[1], f[0]))
        })
        .collect()
}

#[test]
fn a_single_row_matches_whatever_the_bundle_is() {
    let m = Mappings::load(&tsv(&[("alpha", "Alpha", "humble")]), None).unwrap();
    for bundle in ["humble", "other", ""] {
        assert_eq!(m.lookup("alpha", bundle).unwrap().series, "Alpha");
    }
    assert!(m.lookup("missing", "humble").is_none());
}

#[test]
fn two_rows_for_one_key_are_disambiguated_by_bundle() {
    let m = Mappings::load(&tsv(&[("alpha", "One", "a"), ("alpha", "Two", "b")]), None).unwrap();
    assert_eq!(m.lookup("alpha", "a").unwrap().series, "One");
    assert_eq!(m.lookup("alpha", "b").unwrap().series, "Two");
    assert!(m.lookup("alpha", "c").is_none(), "never silently pick");
    assert!(m.lookup("alpha", "").is_none());
    assert_eq!(m.len(), 2);
}

#[test]
fn local_rows_outrank_shipped_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("local.tsv");
    std::fs::write(&path, tsv(&[("alpha", "Fixed", "")])).unwrap();
    // Shipped has two bundles; the local row has neither, and still wins.
    let shipped = tsv(&[("alpha", "One", "a"), ("alpha", "Two", "b"), ("beta", "Beta", "a")]);
    let m = Mappings::load(&shipped, Some(&path)).unwrap();
    assert_eq!(m.lookup("alpha", "a").unwrap().series, "Fixed");
    assert_eq!(m.lookup("alpha", "zzz").unwrap().series, "Fixed");
    assert_eq!(m.lookup("beta", "a").unwrap().series, "Beta");
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
    std::fs::write(&path, tsv(&[("alpha", "Ok", ""), ("beta", "   ", "")])).unwrap();
    let msg = Mappings::load(&tsv(&[]), Some(&path)).err().unwrap().to_string();
    assert!(msg.contains("line 3"), "{msg}");
    assert!(msg.contains("local"), "{msg}");
}

#[test]
fn load_validates_every_shipped_row() {
    let msg = Mappings::load(&tsv(&[("alpha", "Ok", ""), ("Bad Key", "X", "")]), None)
        .err()
        .unwrap()
        .to_string();
    assert!(msg.contains("line 3"), "{msg}");
    assert!(Mappings::load(&tsv(&[("alpha", "a\u{200b}b", "")]), None).is_err());
}

#[test]
fn collisions_ignore_rows_that_merely_repeat_themselves() {
    let rows = humble_rows();
    assert!(rows.len() > 2000, "fixture read: {}", rows.len());
    assert!(find_key_collisions(&rows).is_empty());
    // Same key, two bundles, same fields apart from bundle: fine.
    assert!(find_key_collisions(&[row("k", "S", "a"), row("k", "S", "b")]).is_empty());
    // Exact repeat: fine.
    assert!(find_key_collisions(&[row("k", "S", "a"), row("k", "S", "a")]).is_empty());
}

#[test]
fn collisions_report_same_key_and_bundle_with_different_fields() {
    let mut other = row("k", "S", "a");
    other.volume = Some(2);
    let rows = [row("k", "S", "a"), other, row("j", "T", "a"), row("j", "U", "b")];
    assert_eq!(find_key_collisions(&rows), vec![("k".to_string(), "a".to_string())]);
}

/// `find_key_collisions` existed and was run only by CI, so a file nobody
/// checked loaded both rows and `lookup` returned the first — a user's later
/// correction in their own file was ignored with no sign of it.
#[test]
fn a_layer_holding_two_disagreeing_rows_for_one_key_fails_to_load() {
    let both = tsv(&[("alpha", "First", "humble"), ("alpha", "Second", "humble")]);
    let msg = Mappings::load(&both, None).err().expect("two disagreeing rows").to_string();
    assert!(msg.contains("alpha") && msg.contains("humble"), "{msg}");
    assert!(msg.contains("line 3"), "the row that would be ignored: {msg}");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("local.tsv");
    std::fs::write(&path, &both).unwrap();
    let msg = Mappings::load(&tsv(&[]), Some(&path)).err().expect("the local layer").to_string();
    assert!(msg.contains("local mappings"), "{msg}");

    // An exact repeat loses nothing, and two bundles are not a collision.
    assert!(Mappings::load(&tsv(&[("alpha", "A", "h"), ("alpha", "A", "h")]), None).is_ok());
    assert!(Mappings::load(&tsv(&[("alpha", "A", "h"), ("alpha", "B", "f")]), None).is_ok());
}

#[test]
fn a_local_row_shadows_its_shipped_siblings() {
    // Deliberate cost of "local outranks shipped": a local row replaces EVERY shipped
    // row for its key. If shipped holds A and B and the user corrects only A, the
    // lookup for B returns the local row, not shipped B. Merging instead would ignore
    // the correction whenever the bundle matched a shipped row, and that is silent;
    // an orphaned sibling surfaces in the plan. Multi-row keys do not exist in
    // shipped data today.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("local.tsv");
    std::fs::write(&path, tsv(&[("alpha", "Fixed", "a")])).unwrap();
    let shipped = tsv(&[("alpha", "One", "a"), ("alpha", "Two", "b")]);
    let m = Mappings::load(&shipped, Some(&path)).unwrap();
    assert_eq!(m.lookup("alpha", "a").unwrap().series, "Fixed");
    // One local row left for the key, so it answers any bundle, B included.
    assert_eq!(m.lookup("alpha", "b").unwrap().series, "Fixed");
}
