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
    assert_eq!(p.subtitle.as_deref(), Some("Loaded"), "trailing #1 is dropped");
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
    let p = parse_title("Red Sonja #7");
    assert_eq!(p.series, "Red Sonja");
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
