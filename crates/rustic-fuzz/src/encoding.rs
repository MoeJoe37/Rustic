//! Encoding Standard decoders and encoders (`rustic-encoding`): crash-freedom
//! and three differentials.
//!
//! The first byte selects an encoding, the rest is the input. Invariants:
//! decoding in chunks (split points derived from the input) gives the same
//! text as decoding in one call; fatal mode succeeds exactly when
//! replacement mode reports no error, with the same text; and when the
//! output encoding can encode every decoded scalar value, decoding the
//! encoded bytes gives the text back (ISO-2022-JP excepted, whose escapes do
//! not concatenate, and the gb18030 Private Use side table, which is
//! deliberately asymmetric).
use crate::Target;
use crate::fingerprint;
use rustic_encoding::{DecoderErrorMode, Encoding};

pub(crate) struct EncodingTarget;

/// A few inputs per encoding: ASCII, a valid sequence and an invalid one.
pub(crate) fn seeds() -> Vec<Vec<u8>> {
    const BODIES: &[&[u8]] = &[
        b"plain ascii <p>",
        b"\xE3\x81\x82\xA4\xA2\x82\xA0\xB0\xA1\x81\x30\x81\x30",
        b"\x1B$B$\"\x1B(J\\\x1B(I!\x1B(B",
        b"\xFF\xFE\x00\xD8\x41\x00\xF0\x9F\x98",
    ];
    let mut seeds = Vec::new();
    for index in 0..Encoding::all().len() {
        for body in BODIES {
            let mut seed = vec![index as u8];
            seed.extend_from_slice(body);
            seeds.push(seed);
        }
    }
    seeds
}

impl Target for EncodingTarget {
    fn name(&self) -> &'static str {
        "encoding"
    }

    fn max_len(&self) -> usize {
        4096
    }

    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[
            b"\x1B$B",
            b"\x1B$@",
            b"\x1B(B",
            b"\x1B(J",
            b"\x1B(I",
            b"\xEF\xBB\xBF",
            b"\xFE\xFF",
            b"\x81\x30\x81\x30",
            b"\xE3\x32\x9A\x35",
            b"\x84\x31\xA4\x39",
            b"\x8F\xA1\xA1",
            b"\x8E\xA1",
            b"\x88\x62",
            b"\xF0\x40",
            b"\x00\xD8\x00\xDC",
            b"\xF4\x8F\xBF\xBF",
        ]
    }

    fn check(&self, input: &[u8]) -> Result<(), String> {
        let Some((&selector, bytes)) = input.split_first() else {
            return Ok(());
        };
        let encodings = Encoding::all();
        let encoding = encodings[usize::from(selector) % encodings.len()];
        let whole = encoding.decode_without_bom(bytes);

        let mut decoder = encoding.new_decoder(DecoderErrorMode::Replacement);
        let mut chunked = String::new();
        let mut split = fingerprint(input);
        let mut position = 0;
        while position < bytes.len() {
            let step = 1 + (split % 7) as usize;
            split = split.rotate_right(3) ^ 0x9E37_79B9;
            let end = (position + step).min(bytes.len());
            let _ = decoder.decode(&bytes[position..end], false, &mut chunked);
            position = end;
        }
        let _ = decoder.decode(&[], true, &mut chunked);
        if chunked != whole {
            return Err(format!(
                "{encoding}: chunked decode {chunked:?} differs from {whole:?}"
            ));
        }

        let fatal = encoding.decode_without_bom_or_fail(bytes);
        match (&fatal, decoder.had_errors()) {
            (Some(text), false) if *text == whole => {}
            (None, true) => {}
            _ => {
                return Err(format!(
                    "{encoding}: fatal mode {fatal:?} disagrees with replacement errors {}",
                    decoder.had_errors()
                ));
            }
        }

        let output = encoding.output_encoding();
        if output == Encoding::Iso2022Jp
            || (matches!(output, Encoding::Gbk | Encoding::Gb18030)
                && whole
                    .chars()
                    .any(|c| ('\u{E78D}'..='\u{E864}').contains(&c)))
        {
            return Ok(());
        }
        let mut encoder = output.new_encoder();
        let mut encoded = Vec::new();
        for character in whole.chars() {
            if encoder.encode_char(character, &mut encoded).is_err() {
                return Ok(());
            }
        }
        encoder.finish(&mut encoded);
        let again = output.decode_without_bom(&encoded);
        if again != whole {
            return Err(format!(
                "{encoding}: {whole:?} encoded as {encoded:02X?} decodes to {again:?}"
            ));
        }
        Ok(())
    }
}
