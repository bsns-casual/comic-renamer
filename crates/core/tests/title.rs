use comic_renamer_core::title::{parse_title, split_at_boundary};

#[test]
fn parses_the_elfquest_example_from_the_spec() {
    let p = parse_title("The Complete ElfQuest Volume 1: The Original Quest TPB");
    assert_eq!(p.series, "The Complete ElfQuest");
    assert_eq!(p.volume, Some(1));
    assert_eq!(p.subtitle.as_deref(), Some("The Original Quest TPB"));
}

#[test]
fn preserves_case_and_punctuation_verbatim() {
    let p = parse_title("DIE: Loaded #1");
    assert_eq!(p.series, "DIE", "case is never folded");
    // Still what tells two issues apart, now as a field rather than text.
    assert_eq!(p.issue, Some(1), "the issue number is what tells issues apart");
    assert_eq!(p.subtitle.as_deref(), Some("Loaded"), "the arc is the subtitle");
    let p = parse_title("Hack/Slash: Back to School");
    assert_eq!(p.series, "Hack/Slash");
    assert_eq!(p.subtitle.as_deref(), Some("Back to School"));
    let p = parse_title("Black is the Color Vol. 2: Who's There?");
    assert_eq!(p.series, "Black is the Color");
    assert_eq!(p.subtitle.as_deref(), Some("Who's There?"));
}

#[test]
fn handles_every_humble_volume_form() {
    let p = parse_title("Harbinger, Vol. 2: Renegades");
    assert_eq!((p.series.as_str(), p.volume), ("Harbinger", Some(2)));
    assert_eq!(p.subtitle.as_deref(), Some("Renegades"));
    let p = parse_title("Love & Rockets Vol. IV");
    assert_eq!(
        (p.series.as_str(), p.volume, p.subtitle),
        ("Love & Rockets", Some(4), None)
    );
    let p = parse_title("The Mask Omnibus Volume 1 (Second Edition)");
    assert_eq!((p.series.as_str(), p.volume), ("The Mask Omnibus", Some(1)));
    let p = parse_title("X-O Manowar, Vol. 3:Planet Death");
    assert_eq!((p.series.as_str(), p.volume), ("X-O Manowar", Some(3)));
    assert_eq!(p.subtitle.as_deref(), Some("Planet Death"));
    let p = parse_title("Minimum Wage Vol. 1 (#1-6)");
    assert_eq!(
        (p.series.as_str(), p.volume, p.subtitle),
        ("Minimum Wage", Some(1), None)
    );
}

#[test]
fn the_series_ends_at_whichever_boundary_comes_first() {
    // A colon before any volume marker: the later marker belongs to the subtitle.
    let p = parse_title("Foo: Bar Vol. 2");
    assert_eq!(p.series, "Foo");
    assert_eq!(p.volume, None);
    assert_eq!(p.subtitle.as_deref(), Some("Bar Vol. 2"));
}

#[test]
fn a_roman_numeral_must_be_the_whole_token() {
    let p = parse_title("Volume IVory Tower");
    assert_eq!(p.volume, None, "splitting IVory at IV would invent a volume");
    assert_eq!(p.series, "Volume IVory Tower");
    assert_eq!(parse_title("Foo Vol. iv Thing").volume, None, "lowercase is not a numeral");
}

#[test]
fn no_boundary_means_the_whole_text_is_the_series() {
    let p = parse_title("Saga");
    assert_eq!((p.series.as_str(), p.volume, p.subtitle), ("Saga", None, None));
    // The trailing number used to land in the subtitle, which is where it
    // lived before it had a field (D60).
    let p = parse_title("Red Sonja #7");
    assert_eq!((p.series.as_str(), p.volume, p.issue, p.subtitle), ("Red Sonja", None, Some(7), None));
}

#[test]
fn an_issue_number_before_a_colon_does_not_fragment_the_series() {
    // Every issue of an anthology must share one series folder.
    let p = parse_title("Baltic Comics Anthology š! #26: dADa");
    assert_eq!(p.series, "Baltic Comics Anthology š!");
    assert_eq!(p.subtitle.as_deref(), Some("dADa"));
    assert_eq!(parse_title("X #26: Y").series, "X");
}

#[test]
fn two_issues_of_one_series_render_to_different_paths() {
    use comic_renamer_core::preset::{komga::Komga, Preset};
    use comic_renamer_core::types::{Resolved, Status};
    let dest = |title: &str| {
        let p = parse_title(title);
        let r = Resolved {
            series: p.series,
            volume: p.volume,
            issue: p.issue,
            subtitle: p.subtitle,
            run: 0,
            status: Status::Mapped,
            record: None,
        };
        Komga.destination(&r, "pdf")
    };
    for (a, b) in [("Minotaur #1", "Minotaur #2"), ("DIE: Loaded #1", "DIE: Loaded #2")] {
        assert_ne!(dest(a), dest(b), "{a} vs {b}");
    }
    // D60's convention: four digits, no `#`, and the subtitle stays last.
    assert_eq!(dest("Minotaur #1").to_string_lossy(), "Minotaur/Minotaur 0001.pdf");
    assert_eq!(dest("DIE: Loaded #1").to_string_lossy(), "DIE/DIE 0001 - Loaded.pdf");
}

#[test]
fn a_volume_word_without_a_number_is_not_a_boundary() {
    let p = parse_title("Volume Control");
    assert_eq!((p.series.as_str(), p.volume), ("Volume Control", None));
}

#[test]
fn a_volume_too_big_for_u32_is_not_a_volume() {
    assert_eq!(parse_title("Foo Vol. 99999999999").volume, None);
}

/// A range or a decimal has no single volume number. Storing 810 for
/// `Vol. 8-10` would file confidently wrong; no volume sends the row to a human.
#[test]
fn a_range_or_decimal_is_not_a_volume() {
    for text in ["Foo Vols. 1-3", "Foo Vol. 8-10", "Foo Vol. 2.5", "Foo Vol. I-V", "Foo Vol1-01"] {
        assert_eq!(parse_title(text).volume, None, "{text}");
    }
    // Pinned: with no boundary found, the whole text stays in the series.
    let p = parse_title("Foo Vol. 8-10");
    assert_eq!((p.series.as_str(), p.subtitle), ("Foo Vol. 8-10", None));
}

#[test]
fn wrapping_punctuation_around_the_number_is_stripped() {
    assert_eq!(parse_title("Foo Vol. (3)").volume, Some(3));
    assert_eq!(parse_title("Foo Vol. #3").volume, Some(3));
    assert_eq!(parse_title("Foo Vol. 3, Bar").volume, Some(3));
    assert_eq!(parse_title("Foo Vol. IV.").volume, Some(4));
}

#[test]
fn split_at_boundary_returns_slices_of_the_input() {
    let (series, volume, subtitle) = split_at_boundary("Harbinger, Vol. 2: Renegades");
    assert_eq!((series, volume, subtitle), ("Harbinger", Some(2), "Renegades"));
}

// --- D60: a trailing issue number is its own field ------------------------

#[test]
fn a_trailing_hash_number_becomes_the_issue_not_the_subtitle() {
    let p = parse_title("Red Sonja #7");
    assert_eq!(p.series, "Red Sonja");
    assert_eq!((p.volume, p.issue), (None, Some(7)));
    assert_eq!(p.subtitle, None, "the issue has a field now, so it is not text");
}

#[test]
fn a_volume_and_a_trailing_issue_both_survive() {
    // 17 rows of the Humble table are both at once.
    let p = parse_title("Love and Rockets Vol. IV #9");
    assert_eq!(p.series, "Love and Rockets");
    assert_eq!((p.volume, p.issue), (Some(4), Some(9)));
}

#[test]
fn an_issue_past_a_colon_still_leaves_the_arc_as_the_subtitle() {
    let p = parse_title("DIE: Loaded #1");
    assert_eq!(p.series, "DIE");
    assert_eq!(p.issue, Some(1));
    assert_eq!(p.subtitle.as_deref(), Some("Loaded"));
}

#[test]
fn a_collected_editions_issue_range_is_not_an_issue() {
    // 31 titles carry a range. It describes what the collection contains,
    // not which issue this is, and stays dropped as before.
    for title in ["Alex + Ada Vol. 1 (#1-5)", "Invincible Compendium (Issues #1-47)"] {
        let p = parse_title(title);
        assert_eq!(p.issue, None, "{title}: a range is not an issue number");
    }
}

#[test]
fn a_hash_that_is_the_whole_title_yields_no_issue_and_no_series() {
    // The rule declines it, because taking the number would leave no series.
    // `drop_trailing_issue` then empties the series anyway, which predates
    // D60 — and an empty series is refused downstream by both
    // `MappingRow::validate` and `Entry::accept_edit`, so this is a title
    // that cannot become a row rather than one that becomes a wrong row.
    let p = parse_title("#1");
    assert_eq!(p.issue, None, "the rule needs something in front of the number");
    assert_eq!(p.series, "", "and the series is empty, which is refused later");
}
