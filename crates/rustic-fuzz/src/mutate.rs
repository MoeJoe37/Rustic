//! Structure-unaware byte mutations with target dictionaries.

use crate::Rng;

/// Apply between one and four stacked mutations, never exceeding `max_len`.
///
/// `other` is another corpus entry for splicing. Every choice comes from `rng`,
/// so the output depends only on the inputs and the generator state.
pub fn mutate(
    rng: &mut Rng,
    input: &[u8],
    other: &[u8],
    dictionary: &[&[u8]],
    max_len: usize,
) -> Vec<u8> {
    let mut data = input.to_vec();
    let rounds = 1 + rng.below(4);
    for _ in 0..rounds {
        mutate_once(rng, &mut data, other, dictionary);
    }
    data.truncate(max_len);
    data
}

fn mutate_once(rng: &mut Rng, data: &mut Vec<u8>, other: &[u8], dictionary: &[&[u8]]) {
    match rng.below(10) {
        0 if !data.is_empty() => {
            let at = rng.below(data.len());
            data[at] ^= 1 << rng.below(8);
        }
        1 if !data.is_empty() => {
            let at = rng.below(data.len());
            data[at] = interesting_byte(rng);
        }
        2 => {
            let at = rng.below(data.len() + 1);
            let count = 1 + rng.below(8);
            let bytes: Vec<u8> = (0..count).map(|_| interesting_byte(rng)).collect();
            data.splice(at..at, bytes);
        }
        3 if !data.is_empty() => {
            let (start, end) = range(rng, data.len());
            data.drain(start..end);
        }
        4 if !data.is_empty() => {
            // Duplicate a range, sometimes many times, to build deep nesting.
            let (start, end) = range(rng, data.len());
            let copy = data[start..end].to_vec();
            let repeats = if rng.one_in(8) { 1 + rng.below(64) } else { 1 };
            let at = rng.below(data.len() + 1);
            let mut inserted = Vec::with_capacity(copy.len() * repeats);
            for _ in 0..repeats {
                inserted.extend_from_slice(&copy);
            }
            data.splice(at..at, inserted);
        }
        5 if !other.is_empty() => {
            let (start, end) = range(rng, other.len());
            let at = rng.below(data.len() + 1);
            data.splice(at..at, other[start..end].iter().copied());
        }
        6 if !data.is_empty() => {
            let at = rng.below(data.len() + 1);
            data.truncate(at);
        }
        7 if !data.is_empty() => {
            let (start, end) = range(rng, data.len());
            data[start..end].reverse();
        }
        _ if !dictionary.is_empty() => {
            let word = dictionary[rng.below(dictionary.len())];
            let at = rng.below(data.len() + 1);
            data.splice(at..at, word.iter().copied());
        }
        _ => data.push(interesting_byte(rng)),
    }
}

/// A nonempty half-open range inside `0..len`, biased toward short ranges.
fn range(rng: &mut Rng, len: usize) -> (usize, usize) {
    let start = rng.below(len);
    let max = (len - start).min(if rng.one_in(4) { len } else { 16 });
    let end = start + 1 + rng.below(max);
    (start, end)
}

/// Bytes that commonly change parser state, plus arbitrary values.
fn interesting_byte(rng: &mut Rng) -> u8 {
    const BYTES: &[u8] = b"<>/!?-=&#;'\"\0\r\n\t \x0c[]xX\xef\xbb\xbf\xc3\xa9\xf0\x9f\x80\xed\xa0";
    if rng.one_in(4) {
        rng.below(256) as u8
    } else {
        BYTES[rng.below(BYTES.len())]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_is_deterministic_and_bounded() {
        let dictionary: &[&[u8]] = &[b"<table>", b"&amp;"];
        for seed in 0..500 {
            let a = mutate(&mut Rng::new(seed), b"<p>abc", b"<b>x</b>", dictionary, 32);
            let b = mutate(&mut Rng::new(seed), b"<p>abc", b"<b>x</b>", dictionary, 32);
            assert_eq!(a, b);
            assert!(a.len() <= 32);
        }
    }

    #[test]
    fn empty_inputs_still_grow() {
        let mut grew = false;
        for seed in 0..100 {
            grew |= !mutate(&mut Rng::new(seed), b"", b"", &[], 64).is_empty();
        }
        assert!(grew);
    }
}
