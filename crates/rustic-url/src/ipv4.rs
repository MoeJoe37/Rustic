use crate::{ParseError, ValidationError};

pub(crate) fn ends_in_number(input: &str) -> bool {
    let without_trailing_dot = input.strip_suffix('.').unwrap_or(input);
    let last = without_trailing_dot.rsplit('.').next().unwrap_or("");
    if last.is_empty() {
        return false;
    }
    if last.bytes().all(|byte| byte.is_ascii_digit()) {
        return true;
    }
    last.strip_prefix("0x")
        .or_else(|| last.strip_prefix("0X"))
        .is_some_and(|digits| digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

pub(crate) fn parse(input: &str) -> Result<u32, ParseError> {
    analyze(input, &mut |_| {}).map_err(|_| ParseError::InvalidIpv4)
}

/// The IPv4 parser; `report` receives non-failure validation errors in order.
pub(crate) fn analyze(
    input: &str,
    report: &mut dyn FnMut(ValidationError),
) -> Result<u32, ValidationError> {
    let input = match input.strip_suffix('.') {
        Some(stripped) => {
            report(ValidationError::Ipv4EmptyPart);
            stripped
        }
        None => input,
    };
    if input.split('.').count() > 4 {
        return Err(ValidationError::Ipv4TooManyParts);
    }
    let mut numbers = [0u64; 4];
    let mut count = 0;
    for part in input.split('.') {
        let (value, non_decimal) = number(part).ok_or(ValidationError::Ipv4NonNumericPart)?;
        if non_decimal {
            report(ValidationError::Ipv4NonDecimalPart);
        }
        numbers[count] = value;
        count += 1;
    }
    let numbers = &numbers[..count];
    if numbers.iter().any(|number| *number > 255) {
        report(ValidationError::Ipv4OutOfRangePart);
    }
    let (last, leading) = numbers.split_last().expect("split yields one part");
    if leading.iter().any(|number| *number > 255) || *last >= 256u64.pow((5 - count) as u32) {
        return Err(ValidationError::Ipv4OutOfRangePart);
    }
    let mut address = *last;
    for (index, number) in leading.iter().enumerate() {
        address += number << (24 - index * 8);
    }
    u32::try_from(address).map_err(|_| ValidationError::Ipv4OutOfRangePart)
}

/// Parse one part, returning its value (saturated, since any value that large
/// is out of range) and whether it used a non-decimal radix.
fn number(input: &str) -> Option<(u64, bool)> {
    if input.is_empty() {
        return None;
    }
    let (digits, radix) = if let Some(hex) = input
        .strip_prefix("0x")
        .or_else(|| input.strip_prefix("0X"))
    {
        (hex, 16)
    } else if input.len() >= 2 && input.starts_with('0') {
        (&input[1..], 8)
    } else {
        (input, 10)
    };
    let mut value = 0u64;
    for byte in digits.bytes() {
        let digit = match byte {
            b'0'..=b'9' => u64::from(byte - b'0'),
            b'a'..=b'f' => u64::from(byte - b'a' + 10),
            b'A'..=b'F' => u64::from(byte - b'A' + 10),
            _ => return None,
        };
        if digit >= radix {
            return None;
        }
        value = value.saturating_mul(radix).saturating_add(digit);
    }
    Some((value, radix != 10))
}
