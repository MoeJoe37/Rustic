use std::fmt::Write;

use crate::ParseError;

pub(crate) fn parse(input: &str) -> Result<[u16; 8], ParseError> {
    let mut address = [0; 8];
    if let Some((left, right)) = input.split_once("::") {
        if right.contains("::") {
            return Err(ParseError::InvalidIpv6);
        }
        let mut tail = [0; 8];
        let head_len = pieces(left, &mut address, false)?;
        let tail_len = pieces(right, &mut tail, true)?;
        if head_len + tail_len >= 8 {
            return Err(ParseError::InvalidIpv6);
        }
        address[8 - tail_len..].copy_from_slice(&tail[..tail_len]);
    } else if input.is_empty() || pieces(input, &mut address, true)? != 8 {
        return Err(ParseError::InvalidIpv6);
    }
    Ok(address)
}

fn pieces(input: &str, output: &mut [u16; 8], allow_ipv4: bool) -> Result<usize, ParseError> {
    if input.is_empty() {
        return Ok(0);
    }
    let mut count = 0;
    let mut parts = input.split(':').peekable();
    while let Some(part) = parts.next() {
        if count >= 8 || part.is_empty() {
            return Err(ParseError::InvalidIpv6);
        }
        if part.contains('.') {
            if !allow_ipv4 || parts.peek().is_some() || count > 6 {
                return Err(ParseError::InvalidIpv6);
            }
            let mut bytes = [0u8; 4];
            let mut byte_count = 0;
            for decimal in part.split('.') {
                if byte_count == 4
                    || decimal.is_empty()
                    || (decimal.len() > 1 && decimal.starts_with('0'))
                    || !decimal.bytes().all(|byte| byte.is_ascii_digit())
                {
                    return Err(ParseError::InvalidIpv6);
                }
                bytes[byte_count] = decimal.parse::<u8>().map_err(|_| ParseError::InvalidIpv6)?;
                byte_count += 1;
            }
            if byte_count != 4 {
                return Err(ParseError::InvalidIpv6);
            }
            output[count] = u16::from_be_bytes([bytes[0], bytes[1]]);
            output[count + 1] = u16::from_be_bytes([bytes[2], bytes[3]]);
            count += 2;
        } else {
            if part.len() > 4 || !part.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(ParseError::InvalidIpv6);
            }
            output[count] = u16::from_str_radix(part, 16).map_err(|_| ParseError::InvalidIpv6)?;
            count += 1;
        }
    }
    Ok(count)
}

pub(crate) fn serialize(address: &[u16; 8]) -> String {
    let mut best = None;
    let mut best_len = 1;
    let mut index = 0;
    while index < 8 {
        if address[index] != 0 {
            index += 1;
            continue;
        }
        let start = index;
        while index < 8 && address[index] == 0 {
            index += 1;
        }
        if index - start > best_len {
            best = Some(start);
            best_len = index - start;
        }
    }
    let mut output = String::new();
    index = 0;
    while index < 8 {
        if best == Some(index) {
            output.push_str("::");
            index += best_len;
            continue;
        }
        if index != 0 && best.map(|start| start + best_len) != Some(index) {
            output.push(':');
        }
        write!(output, "{:x}", address[index]).expect("String writes cannot fail");
        index += 1;
    }
    output
}
