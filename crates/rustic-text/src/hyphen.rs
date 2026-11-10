//! Bounded Knuth–Liang matching over pinned, licensed English patterns.
use std::{collections::BTreeMap, sync::OnceLock};
#[derive(Default)]
struct Node {
    edges: BTreeMap<u8, usize>,
    weights: Vec<u8>,
}
struct Dictionary {
    nodes: Vec<Node>,
    exceptions: BTreeMap<String, Vec<usize>>,
}
fn section<'a>(text: &'a str, name: &str) -> &'a str {
    text.split_once(name)
        .and_then(|(_, s)| s.split_once('{'))
        .and_then(|(_, s)| s.split_once('}'))
        .map_or("", |(s, _)| s)
}
fn dictionary() -> &'static Dictionary {
    static DATA: OnceLock<Dictionary> = OnceLock::new();
    DATA.get_or_init(|| {
        let raw = include_str!("../data/hyphenation/en-us.tex")
            .lines()
            .map(|l| l.split('%').next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        let mut d = Dictionary {
            nodes: vec![Node::default()],
            exceptions: BTreeMap::new(),
        };
        for pattern in section(&raw, "\\patterns").split_whitespace() {
            let mut letters = Vec::new();
            let mut weights = vec![0];
            for b in pattern.bytes() {
                if b.is_ascii_digit() {
                    if let Some(w) = weights.last_mut() {
                        *w = b - b'0';
                    }
                } else if b.is_ascii_alphabetic() || b == b'.' {
                    letters.push(b);
                    weights.push(0);
                }
            }
            let mut node = 0;
            for b in letters {
                node = if let Some(&next) = d.nodes[node].edges.get(&b) {
                    next
                } else {
                    let next = d.nodes.len();
                    d.nodes.push(Node::default());
                    d.nodes[node].edges.insert(b, next);
                    next
                };
            }
            d.nodes[node].weights = weights;
        }
        for exception in section(&raw, "\\hyphenation").split_whitespace() {
            let mut word = String::new();
            let mut breaks = Vec::new();
            for c in exception.chars() {
                if c == '-' {
                    breaks.push(word.len());
                } else {
                    word.push(c);
                }
            }
            d.exceptions.insert(word, breaks);
        }
        d
    })
}
/// Automatic discretionary points in original scalar indexes. Only `en` and
/// `en-US` have a shipped dictionary; unknown languages return no guesses.
/// ASCII words over 256 characters are left unhyphenated (TD-87).
pub fn hyphenation_points(word: &str, language: &str) -> Vec<usize> {
    if !(language.eq_ignore_ascii_case("en") || language.eq_ignore_ascii_case("en-us"))
        || word.len() > 256
        || word.len() < 5
        || !word.bytes().all(|b| b.is_ascii_alphabetic())
    {
        return Vec::new();
    }
    let word = word.to_ascii_lowercase();
    let d = dictionary();
    if let Some(points) = d.exceptions.get(&word) {
        return points
            .iter()
            .copied()
            .filter(|&p| p >= 2 && p + 3 <= word.len())
            .collect();
    }
    let bytes = format!(".{word}.").into_bytes();
    let mut weights = vec![0; bytes.len() + 1];
    for start in 0..bytes.len() {
        let mut node = 0;
        for (offset, &b) in bytes[start..].iter().enumerate() {
            let Some(&next) = d.nodes[node].edges.get(&b) else {
                break;
            };
            node = next;
            for (k, &v) in d.nodes[node].weights.iter().enumerate() {
                if let Some(w) = weights.get_mut(start + k) {
                    *w = (*w).max(v);
                }
            }
            if offset >= 64 {
                break;
            }
        }
    }
    (2..=word.len() - 3)
        .filter(|&p| weights[p + 1] % 2 == 1)
        .collect()
}
