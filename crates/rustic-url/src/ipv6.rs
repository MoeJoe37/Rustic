use std::fmt::Write;

use crate::{ParseError, ValidationError};

pub(crate) fn parse(input: &str) -> Result<[u16; 8], ParseError> {
    analyze(input).map_err(|_| ParseError::InvalidIpv6)
}

/// The IPv6 parser, naming the standard validation error for each failure.
pub(crate) fn analyze(input: &str) -> Result<[u16; 8], ValidationError> {
    let mut address = [0; 8];
    if let Some((left, right)) = input.split_once("::") {
        if right.contains("::") || right.starts_with(':') {
            return Err(ValidationError::Ipv6MultipleCompression);
        }
        let mut tail = [0; 8];
        let head_len = pieces(left, &mut address, false)?;
        let tail_len = pieces(right, &mut tail, true)?;
        if head_len + tail_len >= 8 {
            return Err(ValidationError::Ipv6TooManyPieces);
        }
        address[8 - tail_len..].copy_from_slice(&tail[..tail_len]);
    } else if input.is_empty() || pieces(input, &mut address, true)? != 8 {
        return Err(ValidationError::Ipv6TooFewPieces);
    }
    Ok(address)
}

fn pieces(input: &str, output: &mut [u16; 8], allow_ipv4: bool) -> Result<usize, ValidationError> {
    use ValidationError::{
        Ipv4InIpv6InvalidCodePoint, Ipv4InIpv6OutOfRangePart, Ipv4InIpv6TooFewParts,
        Ipv4InIpv6TooManyPieces, Ipv6InvalidCodePoint, Ipv6InvalidCompression, Ipv6TooManyPieces,
    };
    if input.is_empty() {
        return Ok(0);
    }
    let mut count = 0;
    let mut parts = input.split(':').peekable();
    while let Some(part) = parts.next() {
        if count >= 8 {
            return Err(Ipv6TooManyPieces);
        }
        if part.is_empty() {
            // Only a leading or trailing empty part survives the `::` split.
            return Err(if count == 0 {
                Ipv6InvalidCompression
            } else {
                Ipv6InvalidCodePoint
            });
        }
        if part.contains('.') {
            if !allow_ipv4 || parts.peek().is_some() {
                return Err(Ipv4InIpv6InvalidCodePoint);
            }
            if count > 6 {
                return Err(Ipv4InIpv6TooManyPieces);
            }
            let mut bytes = [0u8; 4];
            let mut byte_count = 0;
            for decimal in part.split('.') {
                if byte_count == 4
                    || decimal.is_empty()
                    || (decimal.len() > 1 && decimal.starts_with('0'))
                    || !decimal.bytes().all(|byte| byte.is_ascii_digit())
                {
                    return Err(Ipv4InIpv6InvalidCodePoint);
                }
                bytes[byte_count] = decimal
                    .parse::<u8>()
                    .map_err(|_| Ipv4InIpv6OutOfRangePart)?;
                byte_count += 1;
            }
            if byte_count != 4 {
                return Err(Ipv4InIpv6TooFewParts);
            }
            output[count] = u16::from_be_bytes([bytes[0], bytes[1]]);
            output[count + 1] = u16::from_be_bytes([bytes[2], bytes[3]]);
            count += 2;
        } else {
            if part.len() > 4 || !part.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(Ipv6InvalidCodePoint);
            }
            output[count] = u16::from_str_radix(part, 16).map_err(|_| Ipv6InvalidCodePoint)?;
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
