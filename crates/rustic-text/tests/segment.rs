//! UAX #14 and UAX #29 conformance against the complete Unicode 17.0.0
//! LineBreakTest.txt and GraphemeBreakTest.txt (see fixtures/).

use rustic_text::segment::{BreakOpportunity, grapheme_boundaries, line_breaks};

struct Case {
    text: Vec<char>,
    breaks: Vec<usize>,
}

fn cases(source: &str) -> Vec<Case> {
    source
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| {
            let (points, breaks) = line.split_once(';').expect("fixture line");
            Case {
                text: points
                    .split(' ')
                    .map(|p| char::from_u32(u32::from_str_radix(p, 16).unwrap()).unwrap())
                    .collect(),
                breaks: breaks.split(',').map(|b| b.parse().unwrap()).collect(),
            }
        })
        .collect()
}

fn hex(text: &[char]) -> String {
    text.iter()
        .map(|c| format!("{:04X}", u32::from(*c)))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn line_break_test_passes_completely() {
    let cases = cases(include_str!("fixtures/line_break_cases.txt"));
    assert_eq!(cases.len(), 19_338);
    let mut failures = Vec::new();
    for case in &cases {
        let got: Vec<usize> = line_breaks(&case.text)
            .iter()
            .enumerate()
            .filter(|(_, o)| o.is_break())
            .map(|(i, _)| i)
            .collect();
        if got != case.breaks {
            failures.push(format!(
                "{}: want {:?} got {got:?}",
                hex(&case.text),
                case.breaks
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures, first: {:#?}",
        failures.len(),
        &failures[..failures.len().min(20)]
    );
}

#[test]
fn grapheme_break_test_passes_completely() {
    let cases = cases(include_str!("fixtures/grapheme_cases.txt"));
    assert_eq!(cases.len(), 766);
    for case in &cases {
        let got: Vec<usize> = grapheme_boundaries(&case.text)
            .iter()
            .enumerate()
            .filter(|(_, b)| **b)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(got, case.breaks, "{}", hex(&case.text));
    }
}

#[test]
fn mandatory_breaks_and_edges() {
    use BreakOpportunity::{Allowed, Mandatory, Prohibited};
    assert_eq!(line_breaks(&[]), vec![Mandatory]);
    assert_eq!(grapheme_boundaries(&[]), vec![true]);
    let text: Vec<char> = "a b\nc\r\nd".chars().collect();
    assert_eq!(
        line_breaks(&text),
        vec![
            Prohibited, Prohibited, Allowed, Prohibited, Mandatory, Prohibited, Prohibited,
            Mandatory, Mandatory
        ]
    );
}

#[test]
fn long_runs_stay_linear() {
    // Regional indicators, spaces and numeric chains are counted once.
    for c in ['\u{1F1E6}', ' ', '/', '\u{0308}', '1'] {
        let text = vec![c; 200_000];
        assert_eq!(line_breaks(&text).len(), 200_001);
        assert_eq!(grapheme_boundaries(&text).len(), 200_001);
    }
}
