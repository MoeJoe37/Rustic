//! RFC 1952 gzip decoding over the bounded DEFLATE decoder.
//!
//! Concatenated members decode to the concatenation of their contents, as
//! `gzip -d` and browsers do. Each member's CRC-32 and size are checked.

use rustic_base::CancellationToken;

use crate::inflate::{InflateError, raw};
use crate::png::crc32;

const FHCRC: u8 = 0x02;
const FEXTRA: u8 = 0x04;
const FNAME: u8 = 0x08;
const FCOMMENT: u8 = 0x10;
const RESERVED: u8 = 0xe0;

/// Decode a gzip stream, producing at most `max_output` bytes.
///
/// Trailing zero bytes after the last member are ignored (some servers
/// pad); any other trailing data is invalid.
pub fn gzip_decompress(
    data: &[u8],
    max_output: usize,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, InflateError> {
    let mut output = Vec::new();
    let mut rest = data;
    loop {
        let (member, used) = member(rest, max_output - output.len(), cancellation)?;
        output.extend_from_slice(&member);
        rest = &rest[used..];
        if rest.iter().all(|&byte| byte == 0) {
            return Ok(output);
        }
    }
}

fn member(
    data: &[u8],
    max_output: usize,
    cancellation: &CancellationToken,
) -> Result<(Vec<u8>, usize), InflateError> {
    let header = data.get(..10).ok_or(InflateError::Truncated)?;
    if header[0] != 0x1f || header[1] != 0x8b || header[2] != 8 {
        return Err(InflateError::Invalid);
    }
    let flags = header[3];
    if flags & RESERVED != 0 {
        return Err(InflateError::Invalid);
    }
    let mut position = 10;
    if flags & FEXTRA != 0 {
        let length = data
            .get(position..position + 2)
            .ok_or(InflateError::Truncated)?;
        position += 2 + usize::from(u16::from_le_bytes([length[0], length[1]]));
    }
    for flag in [FNAME, FCOMMENT] {
        if flags & flag != 0 {
            let text = data.get(position..).ok_or(InflateError::Truncated)?;
            let end = text
                .iter()
                .position(|&byte| byte == 0)
                .ok_or(InflateError::Truncated)?;
            position += end + 1;
        }
    }
    if flags & FHCRC != 0 {
        let stored = data
            .get(position..position + 2)
            .ok_or(InflateError::Truncated)?;
        let actual = crc32(&[], &data[..position]) & 0xffff;
        if u32::from(u16::from_le_bytes([stored[0], stored[1]])) != actual {
            return Err(InflateError::Checksum);
        }
        position += 2;
    }
    let body = data.get(position..).ok_or(InflateError::Truncated)?;
    let (content, used) = raw(body, max_output, cancellation)?;
    position += used;
    let trailer = data
        .get(position..position + 8)
        .ok_or(InflateError::Truncated)?;
    let crc = u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
    let size = u32::from_le_bytes([trailer[4], trailer[5], trailer[6], trailer[7]]);
    // ISIZE is the length modulo 2^32.
    if crc32(&[], &content) != crc || (content.len() & 0xffff_ffff) as u32 != size {
        return Err(InflateError::Checksum);
    }
    Ok((content, position + 8))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(data: &[u8], limit: usize) -> Result<Vec<u8>, InflateError> {
        gzip_decompress(data, limit, &CancellationToken::default())
    }

    // `gzip.compress(b"hello hello hello\n", mtime=0)`, then with FNAME
    // "a.txt", FCOMMENT "c" and FHCRC set by hand.
    const PLAIN: &[u8] = include_bytes!("../tests/fixtures/hello.gz");
    const FLAGS: &[u8] = include_bytes!("../tests/fixtures/hello-flags.gz");

    #[test]
    fn members_flags_and_trailers() {
        assert_eq!(decode(PLAIN, 1 << 20).unwrap(), b"hello hello hello\n");
        assert_eq!(decode(FLAGS, 1 << 20).unwrap(), b"hello hello hello\n");
        let mut twice = PLAIN.to_vec();
        twice.extend_from_slice(FLAGS);
        twice.extend_from_slice(&[0, 0]);
        assert_eq!(
            decode(&twice, 1 << 20).unwrap(),
            b"hello hello hello\nhello hello hello\n"
        );
    }

    #[test]
    fn corruption_truncation_and_limits() {
        let mut bad_crc = PLAIN.to_vec();
        let at = bad_crc.len() - 8;
        bad_crc[at] ^= 1;
        assert_eq!(decode(&bad_crc, 1 << 20), Err(InflateError::Checksum));
        let mut bad_size = PLAIN.to_vec();
        let at = bad_size.len() - 4;
        bad_size[at] ^= 1;
        assert_eq!(decode(&bad_size, 1 << 20), Err(InflateError::Checksum));
        let mut bad_header_crc = FLAGS.to_vec();
        let name_end = 10 + b"a.txt\0c\0".len();
        bad_header_crc[name_end] ^= 1;
        assert_eq!(
            decode(&bad_header_crc, 1 << 20),
            Err(InflateError::Checksum)
        );
        assert_eq!(
            decode(&PLAIN[..PLAIN.len() - 1], 1 << 20),
            Err(InflateError::Truncated)
        );
        assert_eq!(decode(&PLAIN[..5], 1 << 20), Err(InflateError::Truncated));
        assert_eq!(
            decode(b"\x1f\x8c\x08\0\0\0\0\0\0\0", 1 << 20),
            Err(InflateError::Invalid)
        );
        let mut reserved = PLAIN.to_vec();
        reserved[3] = 0x20;
        assert_eq!(decode(&reserved, 1 << 20), Err(InflateError::Invalid));
        let mut trailing = PLAIN.to_vec();
        trailing.push(7);
        assert_eq!(decode(&trailing, 1 << 20), Err(InflateError::Truncated));
        assert_eq!(decode(PLAIN, 17), Err(InflateError::OutputLimit));
        assert_eq!(decode(PLAIN, 18).unwrap().len(), 18);
    }
}
