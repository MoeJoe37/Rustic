use crate::ParseError;

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
    let input = input.strip_suffix('.').unwrap_or(input);
    let mut numbers = [0u64; 4];
    let mut count = 0;
    for part in input.split('.') {
        if count == 4 {
            return Err(ParseError::InvalidIpv4);
        }
        numbers[count] = number(part)?;
        count += 1;
    }
    if count == 0 {
        return Err(ParseError::InvalidIpv4);
    }
    let last = numbers[count - 1];
    if numbers[..count - 1].iter().any(|number| *number > 255)
        || last >= 256u64.pow((5 - count) as u32)
    {
        return Err(ParseError::InvalidIpv4);
    }
    let mut address = last;
    for (index, number) in numbers[..count - 1].iter().enumerate() {
        address += number << (24 - index * 8);
    }
    u32::try_from(address).map_err(|_| ParseError::InvalidIpv4)
}

fn number(input: &str) -> Result<u64, ParseError> {
    if input.is_empty() {
        return Err(ParseError::InvalidIpv4);
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
            _ => return Err(ParseError::InvalidIpv4),
        };
        if digit >= radix {
            return Err(ParseError::InvalidIpv4);
        }
        value = value
            .checked_mul(radix)
            .and_then(|value| value.checked_add(digit))
            .ok_or(ParseError::InvalidIpv4)?;
    }
    Ok(value)
}
