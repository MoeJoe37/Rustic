//! UAX #9 conformance: committed samples of Unicode 17.0.0 BidiTest.txt and
//! BidiCharacterTest.txt, plus `#[ignore]`d full runs that read the original
//! files from `RUSTIC_UCD_DIR`.

use rustic_text::bidi::{BidiInfo, Level};
use rustic_text::unicode::BidiClass;

/// Levels after L1 (`None` for X9-removed characters) and the L2 order of
/// the retained ones, treating the whole input as one line.
fn line(info: &BidiInfo, n: usize) -> (Vec<Option<u8>>, Vec<usize>) {
    let levels = info.line_levels(0..n);
    let shown: Vec<Option<u8>> = (0..n)
        .map(|i| (!info.is_removed(i)).then(|| levels[i].number()))
        .collect();
    let order = BidiInfo::reorder(&levels)
        .into_iter()
        .filter(|&i| !info.is_removed(i))
        .collect();
    (shown, order)
}

fn parse_levels(field: &str) -> Vec<Option<u8>> {
    field
        .split_whitespace()
        .map(|l| {
            if l == "x" {
                None
            } else {
                Some(l.parse().unwrap())
            }
        })
        .collect()
}

fn parse_order(field: &str) -> Vec<usize> {
    field
        .split_whitespace()
        .map(|o| o.parse().unwrap())
        .collect()
}

fn class_cases(source: &str) -> (usize, Vec<String>) {
    let mut checked = 0;
    let mut failures = Vec::new();
    for raw in source
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let fields: Vec<&str> = raw.split(';').collect();
        let classes: Vec<BidiClass> = fields[0]
            .split_whitespace()
            .map(|c| BidiClass::from_name(c).unwrap())
            .collect();
        let bitset: u8 = fields[1].trim().parse().unwrap();
        let want = (parse_levels(fields[2]), parse_order(fields[3]));
        for (bit, base) in [(1, None), (2, Some(Level::LTR)), (4, Some(Level::RTL))] {
            if bitset & bit == 0 {
                continue;
            }
            checked += 1;
            let info = BidiInfo::from_classes(&classes, base);
            let got = line(&info, classes.len());
            if got != want {
                failures.push(format!("{raw} base {base:?}: got {got:?}"));
            }
        }
    }
    (checked, failures)
}

fn character_cases(source: &str) -> (usize, Vec<String>) {
    let mut checked = 0;
    let mut failures = Vec::new();
    for raw in source
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let fields: Vec<&str> = raw.split(';').collect();
        let text: Vec<char> = fields[0]
            .split_whitespace()
            .map(|p| char::from_u32(u32::from_str_radix(p, 16).unwrap()).unwrap())
            .collect();
        let base = match fields[1] {
            "0" => Some(Level::LTR),
            "1" => Some(Level::RTL),
            _ => None,
        };
        let para: u8 = fields[2].parse().unwrap();
        let want = (parse_levels(fields[3]), parse_order(fields[4]));
        checked += 1;
        let info = BidiInfo::new(&text, base);
        let got = line(&info, text.len());
        let got_para = info.paragraphs()[0].level.number();
        if got != want || got_para != para {
            failures.push(format!("{raw}: got {got_para} {got:?}"));
        }
    }
    (checked, failures)
}

fn assert_clean((checked, failures): (usize, Vec<String>), minimum: usize) {
    assert!(checked >= minimum, "only {checked} checks");
    assert!(
        failures.is_empty(),
        "{} of {checked} failed, first: {:#?}",
        failures.len(),
        &failures[..failures.len().min(10)]
    );
}

#[test]
fn bidi_test_sample_passes() {
    assert_clean(
        class_cases(include_str!("fixtures/bidi_class_cases.txt")),
        7_670,
    );
}

#[test]
fn bidi_character_test_sample_passes() {
    assert_clean(
        character_cases(include_str!("fixtures/bidi_character_cases.txt")),
        5_732,
    );
}

fn full_file(name: &str) -> String {
    let dir = std::env::var("RUSTIC_UCD_DIR")
        .expect("set RUSTIC_UCD_DIR to the Unicode 17.0.0 UCD files");
    std::fs::read_to_string(std::path::Path::new(&dir).join(name)).expect("readable UCD file")
}

#[test]
#[ignore = "needs RUSTIC_UCD_DIR with the full Unicode test files"]
fn full_bidi_test_passes() {
    // Expand BidiTest.txt's @Levels/@Reorder blocks into the fixture format.
    let mut expanded = String::new();
    let (mut levels, mut order) = (String::new(), String::new());
    for raw in full_file("BidiTest.txt").lines() {
        let body = raw.split('#').next().unwrap_or("").trim();
        if let Some(v) = body.strip_prefix("@Levels:") {
            levels = v.trim().to_owned();
        } else if let Some(v) = body.strip_prefix("@Reorder:") {
            order = v.trim().to_owned();
        } else if !body.is_empty() {
            let (classes, bitset) = body.split_once(';').unwrap();
            expanded.push_str(&format!("{classes};{};{levels};{order}\n", bitset.trim()));
        }
    }
    assert_clean(class_cases(&expanded), 490_000);
}

#[test]
#[ignore = "needs RUSTIC_UCD_DIR with the full Unicode test files"]
fn full_bidi_character_test_passes() {
    assert_clean(character_cases(&full_file("BidiCharacterTest.txt")), 91_000);
}

#[test]
fn paragraphs_split_and_lines_reset_trailing_whitespace() {
    let text: Vec<char> = "abc \u{5D0}\u{5D1} \n\u{5D2} x".chars().collect();
    let info = BidiInfo::new(&text, None);
    assert_eq!(info.paragraphs().len(), 2);
    assert_eq!(info.paragraphs()[0].level, Level::LTR);
    assert_eq!(info.paragraphs()[1].level, Level::RTL);
    assert_eq!(info.paragraphs()[0].range, 0..8);
    let levels = info.line_levels(0..8);
    assert_eq!(
        levels.iter().map(|l| l.number()).collect::<Vec<_>>(),
        [0, 0, 0, 0, 1, 1, 0, 0]
    );
    assert_eq!(BidiInfo::reorder(&levels), [0, 1, 2, 3, 5, 4, 6, 7]);
}

#[test]
fn deep_nesting_and_long_inputs_stay_bounded() {
    let mut text = vec!['\u{202B}'; 300];
    text.extend(vec!['\u{2067}'; 300]);
    text.extend(['(', 'a', ')'].repeat(10_000));
    text.extend(vec!['\u{2069}'; 300]);
    let info = BidiInfo::new(&text, Some(Level::LTR));
    assert!(info.levels().iter().all(|l| l.number() <= 126));
    assert_eq!(BidiInfo::reorder(info.levels()).len(), text.len());
}
