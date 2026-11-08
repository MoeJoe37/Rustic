//! Punycode (RFC 3492) with checked arithmetic and explicit work limits.
//!
//! Both directions are quadratic in the label length by design of the
//! algorithm. The limits below bound that work for content-controlled hosts;
//! real DNS labels are at most 63 octets, so they only reject inputs that can
//! never resolve. See docs/URL.md for the recorded decision.

const BASE: u32 = 36;
const T_MIN: u32 = 1;
const T_MAX: u32 = 26;
const SKEW: u32 = 38;
const DAMP: u32 = 700;
const INITIAL_BIAS: u32 = 72;
const INITIAL_N: u32 = 0x80;

/// Maximum Punycode bytes (after `xn--`) decoded for one label.
pub(super) const MAX_DECODE_INPUT: usize = 2_000;
/// Maximum code points encoded for one label.
pub(super) const MAX_ENCODE_INPUT: usize = 1_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PunycodeError {
    Invalid,
    TooLong,
}

fn adapt(mut delta: u32, points: u32, first: bool) -> u32 {
    delta /= if first { DAMP } else { 2 };
    delta += delta / points;
    let mut k = 0;
    while delta > ((BASE - T_MIN) * T_MAX) / 2 {
        delta /= BASE - T_MIN;
        k += BASE;
    }
    k + (BASE - T_MIN + 1) * delta / (delta + SKEW)
}

fn threshold(k: u32, bias: u32) -> u32 {
    if k <= bias {
        T_MIN
    } else if k >= bias + T_MAX {
        T_MAX
    } else {
        k - bias
    }
}

fn digit_value(byte: u8) -> Option<u32> {
    match byte {
        b'a'..=b'z' => Some(u32::from(byte - b'a')),
        b'A'..=b'Z' => Some(u32::from(byte - b'A')),
        b'0'..=b'9' => Some(u32::from(byte - b'0') + 26),
        _ => None,
    }
}

fn digit(value: u32) -> char {
    const DIGITS: &[u8; BASE as usize] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    // Callers pass values below BASE; the modulo keeps this total anyway.
    let index = usize::try_from(value % BASE).unwrap_or_default();
    char::from(DIGITS[index])
}

/// Decode the part of an A-label after `xn--`.
pub(super) fn decode(input: &str) -> Result<Vec<char>, PunycodeError> {
    if input.len() > MAX_DECODE_INPUT {
        return Err(PunycodeError::TooLong);
    }
    if !input.is_ascii() {
        return Err(PunycodeError::Invalid);
    }
    let (mut output, extended): (Vec<char>, &[u8]) = match input.rfind('-') {
        Some(position) => (
            input[..position].chars().collect(),
            &input.as_bytes()[position + 1..],
        ),
        None => (Vec::new(), input.as_bytes()),
    };
    let mut n = INITIAL_N;
    let mut bias = INITIAL_BIAS;
    let mut i: u32 = 0;
    let mut bytes = extended.iter().copied().peekable();
    while bytes.peek().is_some() {
        let old_i = i;
        let mut weight: u32 = 1;
        let mut k = BASE;
        loop {
            let value = bytes
                .next()
                .and_then(digit_value)
                .ok_or(PunycodeError::Invalid)?;
            i = value
                .checked_mul(weight)
                .and_then(|step| i.checked_add(step))
                .ok_or(PunycodeError::Invalid)?;
            let t = threshold(k, bias);
            if value < t {
                break;
            }
            weight = weight.checked_mul(BASE - t).ok_or(PunycodeError::Invalid)?;
            k += BASE;
        }
        let length = u32::try_from(output.len() + 1).map_err(|_| PunycodeError::Invalid)?;
        bias = adapt(i - old_i, length, old_i == 0);
        n = n.checked_add(i / length).ok_or(PunycodeError::Invalid)?;
        i %= length;
        let value = char::from_u32(n).ok_or(PunycodeError::Invalid)?;
        // i < length = output.len() + 1 after the modulo above.
        let position = usize::try_from(i).map_err(|_| PunycodeError::Invalid)?;
        output.insert(position, value);
        i += 1;
    }
    Ok(output)
}

/// Encode a label's code points; the caller adds the `xn--` prefix.
pub(super) fn encode(input: &[char]) -> Result<String, PunycodeError> {
    if input.len() > MAX_ENCODE_INPUT {
        return Err(PunycodeError::TooLong);
    }
    let mut output: String = input.iter().filter(|value| value.is_ascii()).collect();
    let basic = u32::try_from(output.len()).map_err(|_| PunycodeError::Invalid)?;
    if basic > 0 {
        output.push('-');
    }
    let total = u32::try_from(input.len()).map_err(|_| PunycodeError::Invalid)?;
    let mut handled = basic;
    let mut n = INITIAL_N;
    let mut delta: u32 = 0;
    let mut bias = INITIAL_BIAS;
    while handled < total {
        let m = input
            .iter()
            .map(|&value| u32::from(value))
            .filter(|&value| value >= n)
            .min()
            .ok_or(PunycodeError::Invalid)?;
        delta = (m - n)
            .checked_mul(handled + 1)
            .and_then(|step| delta.checked_add(step))
            .ok_or(PunycodeError::Invalid)?;
        n = m;
        for &value in input {
            let value = u32::from(value);
            if value < n {
                delta = delta.checked_add(1).ok_or(PunycodeError::Invalid)?;
            }
            if value == n {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = threshold(k, bias);
                    if q < t {
                        break;
                    }
                    output.push(digit(t + (q - t) % (BASE - t)));
                    q = (q - t) / (BASE - t);
                    k += BASE;
                }
                output.push(digit(q));
                bias = adapt(delta, handled + 1, handled == basic);
                delta = 0;
                handled += 1;
            }
        }
        delta = delta.checked_add(1).ok_or(PunycodeError::Invalid)?;
        n += 1;
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(unicode: &str, ascii: &str) {
        let chars: Vec<char> = unicode.chars().collect();
        assert_eq!(encode(&chars).as_deref(), Ok(ascii));
        assert_eq!(decode(ascii), Ok(chars));
    }

    #[test]
    fn rfc_3492_samples() {
        // RFC 3492 section 7.1 (A) Arabic (Egyptian) and (L) Japanese samples.
        round_trip(
            "\u{644}\u{64A}\u{647}\u{645}\u{627}\u{628}\u{62A}\u{643}\u{644}\u{645}\u{648}\u{634}\u{639}\u{631}\u{628}\u{64A}\u{61F}",
            "egbpdaj6bu4bxfgehfvwxn",
        );
        round_trip(
            "3\u{5E74}B\u{7D44}\u{91D1}\u{516B}\u{5148}\u{751F}",
            "3B-ww4c5e180e575a65lsy2b",
        );
        round_trip("m\u{FC}nchen", "mnchen-3ya");
        round_trip("\u{2615}", "53h");
        round_trip("", "");
    }

    #[test]
    fn short_inputs_never_panic_and_round_trip() {
        const DIGITS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789-";
        for &a in DIGITS {
            for &b in DIGITS {
                for &c in DIGITS {
                    let input = String::from_utf8(vec![a, b, c]).expect("ASCII");
                    if let Ok(decoded) = decode(&input) {
                        if decoded.iter().any(|value| !value.is_ascii()) {
                            let encoded = encode(&decoded).expect("decoded labels re-encode");
                            assert_eq!(decode(&encoded), Ok(decoded));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn rejects_malformed_and_oversized_input() {
        assert_eq!(decode("9"), Err(PunycodeError::Invalid));
        assert_eq!(decode("a"), Ok(vec!['\u{80}']));
        assert_eq!(decode("99999999999"), Err(PunycodeError::Invalid));
        assert_eq!(decode("zz!"), Err(PunycodeError::Invalid));
        assert_eq!(decode("\u{E9}"), Err(PunycodeError::Invalid));
        let long = "a".repeat(MAX_DECODE_INPUT + 1);
        assert_eq!(decode(&long), Err(PunycodeError::TooLong));
        let wide = vec!['\u{E9}'; MAX_ENCODE_INPUT + 1];
        assert_eq!(encode(&wide), Err(PunycodeError::TooLong));
        let edge = vec!['\u{E9}'; MAX_ENCODE_INPUT];
        let encoded = encode(&edge).expect("limit is inclusive");
        assert_eq!(decode(&encoded), Ok(edge));
    }
}
