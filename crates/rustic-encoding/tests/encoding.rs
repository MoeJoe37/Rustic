//! Encoding Standard behavior: labels, every decoder's error paths, encoders
//! and the hooks. The exhaustive comparison with a browser implementation is
//! described in docs/ENCODING.md; these tests pin the cases the standard
//! spells out and the properties every index must keep.

use rustic_encoding::{
    DecoderErrorMode, Encoding, bom_sniff, decode, encode, utf8_decode, utf8_decode_without_bom,
    utf8_decode_without_bom_or_fail,
};

fn decoded(encoding: Encoding, bytes: &[u8]) -> Vec<u32> {
    encoding
        .decode_without_bom(bytes)
        .chars()
        .map(u32::from)
        .collect()
}

/// Decode one byte at a time, so every restore crosses a chunk boundary.
fn decoded_bytewise(encoding: Encoding, bytes: &[u8]) -> String {
    let mut decoder = encoding.new_decoder(DecoderErrorMode::Replacement);
    let mut output = String::new();
    for byte in bytes {
        decoder
            .decode(std::slice::from_ref(byte), false, &mut output)
            .unwrap();
    }
    decoder.decode(&[], true, &mut output).unwrap();
    output
}

const FFFD: u32 = 0xFFFD;

#[test]
fn labels_follow_get_an_encoding() {
    assert_eq!(Encoding::all().len(), 40);
    for &encoding in Encoding::all() {
        assert_eq!(
            Encoding::for_label(encoding.name()),
            Some(encoding),
            "{encoding}"
        );
    }
    assert_eq!(
        Encoding::for_label(" \t\n\x0C\rLATIN1\r\n"),
        Some(Encoding::Windows1252)
    );
    assert_eq!(Encoding::for_label("ascii"), Some(Encoding::Windows1252));
    assert_eq!(
        Encoding::for_label("iso-8859-1"),
        Some(Encoding::Windows1252)
    );
    assert_eq!(Encoding::for_label("utf-16"), Some(Encoding::Utf16Le));
    assert_eq!(
        Encoding::for_label("unicode-1-1-utf-8"),
        Some(Encoding::Utf8)
    );
    assert_eq!(Encoding::for_label("x-sjis"), Some(Encoding::ShiftJis));
    assert_eq!(
        Encoding::for_label("iso-2022-kr"),
        Some(Encoding::Replacement)
    );
    assert_eq!(Encoding::for_label("gb2312"), Some(Encoding::Gbk));
    assert_eq!(
        Encoding::for_label("x-user-defined"),
        Some(Encoding::XUserDefined)
    );
    // Only ASCII whitespace is trimmed, and only ASCII case is folded.
    assert_eq!(Encoding::for_label("\u{A0}utf-8"), None);
    assert_eq!(Encoding::for_label("utf-8\u{0}"), None);
    assert_eq!(Encoding::for_label("UTF-\u{212A}"), None);
    assert_eq!(Encoding::for_label(""), None);
    assert_eq!(Encoding::for_label("utf8 "), Some(Encoding::Utf8));
    assert_eq!(Encoding::for_label("utf 8"), None);
    assert_eq!(Encoding::for_label_bytes(b"\xFFutf-8"), None);
    assert_eq!(Encoding::for_label(&"a".repeat(64)), None);
}

#[test]
fn output_encodings_and_classes() {
    assert_eq!(Encoding::Replacement.output_encoding(), Encoding::Utf8);
    assert_eq!(Encoding::Utf16Be.output_encoding(), Encoding::Utf8);
    assert_eq!(Encoding::Utf16Le.output_encoding(), Encoding::Utf8);
    assert_eq!(Encoding::ShiftJis.output_encoding(), Encoding::ShiftJis);
    let single = Encoding::all()
        .iter()
        .filter(|encoding| encoding.is_single_byte())
        .count();
    assert_eq!(single, 28);
    assert!(!Encoding::Iso2022Jp.is_ascii_compatible());
    assert!(Encoding::Gb18030.is_ascii_compatible());
    // Every ASCII-compatible decoder maps every ASCII byte to itself.
    let ascii: Vec<u8> = (0..0x80).collect();
    for &encoding in Encoding::all()
        .iter()
        .filter(|encoding| encoding.is_ascii_compatible())
    {
        assert_eq!(
            encoding.decode_without_bom(&ascii).as_bytes(),
            &ascii[..],
            "{encoding}"
        );
    }
}

#[test]
fn utf8_decoder_replaces_maximal_subparts() {
    let cases: &[(&[u8], &[u32])] = &[
        (b"a\x80b", &[0x61, FFFD, 0x62]),
        (b"\xC0\xAF", &[FFFD, FFFD]),
        (b"\xE0\x80\xAF", &[FFFD, FFFD, FFFD]),
        (b"\xED\xA0\x80", &[FFFD, FFFD, FFFD]),
        (b"\xF0\x8F\xBF\xBF", &[FFFD, FFFD, FFFD, FFFD]),
        (b"\xF4\x90\x80\x80", &[FFFD, FFFD, FFFD, FFFD]),
        (b"\xF0\x9F\x98", &[FFFD]),
        (b"\xF0\x9F\x98A", &[FFFD, 0x41]),
        (b"\xE2\x82", &[FFFD]),
        (b"\xF0\x9F\x98\x80", &[0x1F600]),
        (b"\xEF\xBF\xBF", &[0xFFFF]),
        (b"\xF5", &[FFFD]),
    ];
    for (bytes, expected) in cases {
        assert_eq!(decoded(Encoding::Utf8, bytes), *expected, "{bytes:02X?}");
        let bytewise: Vec<u32> = decoded_bytewise(Encoding::Utf8, bytes)
            .chars()
            .map(u32::from)
            .collect();
        assert_eq!(bytewise, *expected, "bytewise {bytes:02X?}");
    }
}

#[test]
fn single_byte_decoders_use_their_index() {
    assert_eq!(
        decoded(Encoding::Windows1252, b"\x80\x81\x9F\xFF"),
        [0x20AC, 0x81, 0x178, 0xFF]
    );
    // ISO-8859-3 has unmapped bytes; they are errors.
    assert_eq!(decoded(Encoding::Iso8859_3, b"\xA5\xA1"), [FFFD, 0x126]);
    assert_eq!(
        decoded(Encoding::Iso8859_8I, b"\xE0"),
        decoded(Encoding::Iso8859_8, b"\xE0")
    );
    assert_eq!(decoded(Encoding::Koi8U, b"\xA4"), [0x454]);
    assert_eq!(
        decoded(Encoding::XUserDefined, b"a\x80\xFF"),
        [0x61, 0xF780, 0xF7FF]
    );
}

#[test]
fn gb18030_decoder() {
    let cases: &[(&[u8], &[u32])] = &[
        (b"\x80", &[0x20AC]),
        (b"\xFF", &[FFFD]),
        (b"\xA3\xA0", &[0x3000]),
        (b"\x81\x40", &[0x4E02]),
        (b"\x81\x30\x81\x30", &[0x80]),
        (b"\x84\x31\xA4\x39", &[0xFFFF]),
        (b"\x90\x30\x81\x30", &[0x10000]),
        (b"\xE3\x32\x9A\x35", &[0x10FFFF]),
        (b"\xE3\x32\x9A\x36", &[FFFD]),
        (b"\x84\x31\xA5\x30", &[FFFD]),
        (b"\xA8\xBC", &[0x1E3F]),
        (b"\x81\x35\xF4\x37", &[0xE7C7]),
        // A failed four-byte sequence restores its last three bytes.
        (b"\x81\x30\x81A", &[FFFD, 0x30, 0x4E04]),
        (b"\x81\x30A", &[FFFD, 0x30, 0x41]),
        (b"\x81A", &[0x4E04]),
        (b"\x81\x7F", &[FFFD, 0x7F]),
        (b"\x81", &[FFFD]),
        (b"\x81\x30\x81", &[FFFD]),
    ];
    for (bytes, expected) in cases {
        assert_eq!(decoded(Encoding::Gb18030, bytes), *expected, "{bytes:02X?}");
        assert_eq!(decoded(Encoding::Gbk, bytes), *expected, "GBK {bytes:02X?}");
    }
}

#[test]
fn big5_decoder() {
    assert_eq!(
        decoded(Encoding::Big5, b"\x88\x62\x88\x64"),
        [0xCA, 0x304, 0xCA, 0x30C]
    );
    assert_eq!(
        decoded(Encoding::Big5, b"\x88\xA3\x88\xA5"),
        [0xEA, 0x304, 0xEA, 0x30C]
    );
    assert_eq!(decoded(Encoding::Big5, b"\xA4\x40"), [0x4E00]);
    assert_eq!(decoded(Encoding::Big5, b"\x87\x40"), [0x43F0]);
    assert_eq!(decoded(Encoding::Big5, b"\xA1\x30"), [FFFD, 0x30]);
    assert_eq!(decoded(Encoding::Big5, b"\xA1\x80"), [FFFD]);
    assert_eq!(decoded(Encoding::Big5, b"\x80"), [FFFD]);
    assert_eq!(decoded(Encoding::Big5, b"\xFE"), [FFFD]);
    // Pointers above the BMP decode to supplementary characters.
    let supplementary = (0x81..=0xFEu8)
        .flat_map(|lead| (0x40..=0xFEu8).map(move |trail| [lead, trail]))
        .any(|pair| {
            decoded(Encoding::Big5, &pair)
                .iter()
                .any(|&code_point| code_point > 0xFFFF)
        });
    assert!(supplementary);
}

#[test]
fn japanese_decoders() {
    // EUC-JP: JIS X 0208, half-width katakana, JIS X 0212.
    assert_eq!(decoded(Encoding::EucJp, b"\xA4\xA2"), [0x3042]);
    assert_eq!(decoded(Encoding::EucJp, b"\x8E\xB1"), [0xFF71]);
    assert_eq!(decoded(Encoding::EucJp, b"\x8F\xB0\xA1"), [0x4E02]);
    assert_eq!(decoded(Encoding::EucJp, b"\x8E\xE0"), [FFFD]);
    assert_eq!(decoded(Encoding::EucJp, b"\x8EA"), [FFFD, 0x41]);
    assert_eq!(decoded(Encoding::EucJp, b"\x8F\xA1A"), [FFFD, 0x41]);
    // Shift_JIS: single bytes, half-width katakana, EUDC and the IBM rows.
    assert_eq!(
        decoded(Encoding::ShiftJis, b"\x80\xA1\xDF"),
        [0x80, 0xFF61, 0xFF9F]
    );
    assert_eq!(decoded(Encoding::ShiftJis, b"\x82\xA0"), [0x3042]);
    assert_eq!(
        decoded(Encoding::ShiftJis, b"\xF0\x40\xF9\xFC"),
        [0xE000, 0xE757]
    );
    assert_eq!(decoded(Encoding::ShiftJis, b"\xFA\x40"), [0x2170]);
    assert_eq!(decoded(Encoding::ShiftJis, b"\x81\x20"), [FFFD, 0x20]);
    assert_eq!(
        decoded(Encoding::ShiftJis, b"\xA0\xFD\xFE\xFF"),
        [FFFD, FFFD, FFFD, FFFD]
    );
}

#[test]
fn iso_2022_jp_decoder() {
    let cases: &[(&[u8], &[u32])] = &[
        (b"a\x1B$B0!\x1B(Bb", &[0x61, 0x4E9C, 0x62]),
        (b"\x1B(J\x5C\x7E", &[0xA5, 0x203E]),
        (b"\x1B(I\x21\x5F", &[0xFF61, 0xFF9F]),
        (b"\x1B(I\x60", &[FFFD]),
        // Two escapes with no output between them: the second is an error.
        (b"\x1B(B\x1B(Ba", &[FFFD, 0x61]),
        (b"\x1B(Ja\x1B(Bb", &[0x61, 0x62]),
        // An escape at the end of input.
        (b"\x1B(J", &[]),
        (b"a\x1B", &[0x61, FFFD]),
        (b"a\x1B(", &[0x61, FFFD, 0x28]),
        (b"\x1B(X", &[FFFD, 0x28, 0x58]),
        (b"\x1B$B0", &[FFFD]),
        (b"\x1B$B0\x1B(Ba", &[FFFD, 0x61]),
        (b"\x1B$B0\x0A", &[FFFD]),
        (b"\x0E\x0F\x80", &[FFFD, FFFD, FFFD]),
        (b"\x1B$B\x22\x2F", &[FFFD]),
    ];
    for (bytes, expected) in cases {
        assert_eq!(
            decoded(Encoding::Iso2022Jp, bytes),
            *expected,
            "{bytes:02X?}"
        );
        let bytewise: Vec<u32> = decoded_bytewise(Encoding::Iso2022Jp, bytes)
            .chars()
            .map(u32::from)
            .collect();
        assert_eq!(bytewise, *expected, "bytewise {bytes:02X?}");
    }
    // The standard's note: the encoder's output for U+00A5, twice, decodes
    // with an error between the two.
    let once = encode("\u{A5}", Encoding::Iso2022Jp);
    assert_eq!(once, b"\x1B(J\x5C\x1B(B");
    let twice = [once.clone(), once].concat();
    assert_eq!(decoded(Encoding::Iso2022Jp, &twice), [0xA5, FFFD, 0xA5]);
}

#[test]
fn euc_kr_decoder() {
    assert_eq!(decoded(Encoding::EucKr, b"\xB0\xA1"), [0xAC00]);
    assert_eq!(decoded(Encoding::EucKr, b"\x81\x41"), [0xAC02]);
    assert_eq!(decoded(Encoding::EucKr, b"\x81\x40"), [FFFD, 0x40]);
    assert_eq!(decoded(Encoding::EucKr, b"\x80\xFF"), [FFFD, FFFD]);
}

#[test]
fn utf16_decoders() {
    assert_eq!(
        decoded(Encoding::Utf16Le, b"a\x00\x3D\xD8\x00\xDE"),
        [0x61, 0x1F600]
    );
    assert_eq!(
        decoded(Encoding::Utf16Be, b"\x00a\xD8\x3D\xDE\x00"),
        [0x61, 0x1F600]
    );
    assert_eq!(decoded(Encoding::Utf16Le, b"\x00\xDC"), [FFFD]);
    assert_eq!(decoded(Encoding::Utf16Le, b"\x00\xD8a\x00"), [FFFD, 0x61]);
    assert_eq!(
        decoded(Encoding::Utf16Le, b"\x00\xD8\x00\xD8\x00\xDC"),
        [FFFD, 0x10000]
    );
    assert_eq!(decoded(Encoding::Utf16Le, b"a"), [FFFD]);
    assert_eq!(decoded(Encoding::Utf16Le, b"\x00\xD8"), [FFFD]);
    assert_eq!(decoded(Encoding::Utf16Be, b"\xD8\x00\x00"), [FFFD]);
}

#[test]
fn replacement_decoder() {
    assert_eq!(decoded(Encoding::Replacement, b""), [] as [u32; 0]);
    assert_eq!(decoded(Encoding::Replacement, b"abc"), [FFFD]);
    assert_eq!(decoded_bytewise(Encoding::Replacement, b"abc"), "\u{FFFD}");
}

#[test]
fn fatal_mode_stops_and_resets() {
    let mut decoder = Encoding::Utf8.new_decoder(DecoderErrorMode::Fatal);
    let mut output = String::new();
    assert!(decoder.decode(b"ab\xFFcd", true, &mut output).is_err());
    assert_eq!(output, "ab");
    output.clear();
    decoder.decode(b"ok", true, &mut output).unwrap();
    assert_eq!(output, "ok");
    assert_eq!(utf8_decode_without_bom_or_fail(b"\xE2\x82"), None);
    assert_eq!(
        utf8_decode_without_bom_or_fail(b"\xE2\x82\xAC").as_deref(),
        Some("\u{20AC}")
    );
    assert_eq!(Encoding::ShiftJis.decode_without_bom_or_fail(b"\x82"), None);
}

#[test]
fn streaming_matches_one_shot() {
    // A deterministic mix of valid and invalid multi-byte input.
    let mut state = 0x2545_F491_u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let alphabet = [
        0x1B, 0x24, 0x28, 0x42, 0x4A, 0x30, 0x39, 0x81, 0x84, 0xA1, 0xFE, 0x8E, 0x8F, 0x5C, 0x41,
        0xD8, 0xDC, 0x00, 0xE3, 0xF0,
    ];
    for &encoding in Encoding::all() {
        for _ in 0..300 {
            let length = next() as usize % 20;
            let bytes: Vec<u8> = (0..length)
                .map(|_| alphabet[next() as usize % alphabet.len()])
                .collect();
            assert_eq!(
                decoded_bytewise(encoding, &bytes),
                encoding.decode_without_bom(&bytes),
                "{encoding} {bytes:02X?}"
            );
        }
    }
}

#[test]
fn hooks() {
    assert_eq!(bom_sniff(b"\xEF\xBB\xBFx"), Some((Encoding::Utf8, 3)));
    assert_eq!(bom_sniff(b"\xFE\xFF"), Some((Encoding::Utf16Be, 2)));
    assert_eq!(bom_sniff(b"\xFF\xFE"), Some((Encoding::Utf16Le, 2)));
    assert_eq!(bom_sniff(b"\xEF\xBB"), None);
    let result = decode(b"\xFF\xFEa\x00", Encoding::Windows1252);
    assert_eq!(
        (result.text.as_str(), result.encoding, result.had_errors),
        ("a", Encoding::Utf16Le, false)
    );
    let result = decode(b"caf\xE9", Encoding::Windows1252);
    assert_eq!(
        (result.text.as_str(), result.encoding),
        ("caf\u{E9}", Encoding::Windows1252)
    );
    let result = decode(b"\xEF\xBB\xBF\xFF", Encoding::ShiftJis);
    assert_eq!(
        (result.text.as_str(), result.encoding, result.had_errors),
        ("\u{FFFD}", Encoding::Utf8, true)
    );
    assert_eq!(utf8_decode(b"\xEF\xBB\xBF\xEF\xBB\xBFa"), "\u{FEFF}a");
    assert_eq!(utf8_decode_without_bom(b"\xEF\xBB\xBFa"), "\u{FEFF}a");
}

#[test]
fn encoders() {
    assert_eq!(
        encode("caf\u{E9} \u{2603}", Encoding::Windows1252),
        b"caf\xE9 &#9731;"
    );
    assert_eq!(encode("\u{20AC}", Encoding::Gbk), b"\x80");
    assert_eq!(encode("\u{20AC}", Encoding::Gb18030), b"\xA2\xE3");
    assert_eq!(encode("\u{E5E5}", Encoding::Gb18030), b"&#58853;");
    assert_eq!(
        encode("\u{E78D}\u{E864}", Encoding::Gb18030),
        b"\xA6\xD9\xFE\xA0"
    );
    assert_eq!(
        encode("\u{80}\u{10FFFF}\u{E7C7}", Encoding::Gb18030),
        b"\x81\x30\x81\x30\xE3\x32\x9A\x35\x81\x35\xF4\x37"
    );
    assert_eq!(encode("\u{10000}", Encoding::Gbk), b"&#65536;");
    // Big5 avoids HKSCS pointers and takes the last pointer for six duplicates.
    assert_eq!(
        encode("\u{2550}\u{5341}", Encoding::Big5),
        b"\xF9\xF9\xA4\x51"
    );
    assert_eq!(encode("\u{43F0}", Encoding::Big5), b"&#17392;");
    assert_eq!(encode("\u{CA}\u{304}", Encoding::Big5), b"&#202;&#772;");
    assert_eq!(
        encode("\u{A5}\u{203E}\u{FF71}\u{2212}", Encoding::EucJp),
        b"\x5C\x7E\x8E\xB1\xA1\xDD"
    );
    assert_eq!(encode("\u{4E02}", Encoding::EucJp), b"&#19970;");
    assert_eq!(
        encode("\u{80}\u{FF71}\u{2170}\u{2252}", Encoding::ShiftJis),
        b"\x80\xB1\xFA\x40\x81\xE0"
    );
    assert_eq!(encode("\u{E000}", Encoding::ShiftJis), b"&#57344;");
    assert_eq!(
        encode("\u{AC00}\u{AC02}", Encoding::EucKr),
        b"\xB0\xA1\x81\x41"
    );
    assert_eq!(
        encode("a\u{F780}\u{F7FF}\u{E9}", Encoding::XUserDefined),
        b"a\x80\xFF&#233;"
    );
    assert_eq!(
        encode("\u{1F600}", Encoding::Utf16Le),
        "\u{1F600}".as_bytes()
    );
    assert_eq!(encode("x", Encoding::Replacement), b"x");
}

#[test]
fn iso_2022_jp_encoder() {
    assert_eq!(
        encode("a\u{3042}b", Encoding::Iso2022Jp),
        b"a\x1B$B$\"\x1B(Bb"
    );
    assert_eq!(encode("\u{FF71}", Encoding::Iso2022Jp), b"\x1B$B%\"\x1B(B");
    assert_eq!(
        encode("\u{A5}a\\", Encoding::Iso2022Jp),
        b"\x1B(J\x5Ca\x1B(B\\"
    );
    assert_eq!(
        encode("\u{3042}\u{E9}", Encoding::Iso2022Jp),
        b"\x1B$B$\"\x1B(B&#233;"
    );
    // U+000E, U+000F and U+001B are reported as U+FFFD.
    assert_eq!(encode("\u{1B}", Encoding::Iso2022Jp), b"&#65533;");
    let mut encoder = Encoding::Iso2022Jp.new_encoder();
    let mut output = Vec::new();
    encoder.encode_char('\u{3042}', &mut output).unwrap();
    assert!(encoder.has_pending_state());
    assert_eq!(encoder.encode_char('\u{1B}', &mut output), Err('\u{FFFD}'));
    encoder.finish(&mut output);
    assert_eq!(output, b"\x1B$B$\"\x1B(B");
}

#[test]
fn every_encodable_scalar_round_trips() {
    for &encoding in Encoding::all()
        .iter()
        .filter(|encoding| encoding.output_encoding() == **encoding)
    {
        let mut encoder = encoding.new_encoder();
        let mut mapped = 0usize;
        for character in (0..=0x10FFFF).filter_map(char::from_u32) {
            let mut bytes = Vec::new();
            if encoder.encode_char(character, &mut bytes).is_err() {
                encoder.finish(&mut bytes);
                continue;
            }
            encoder.finish(&mut bytes);
            mapped += 1;
            let text = encoding.decode_without_bom(&bytes);
            // Encoders fold a few code points onto others (U+2212 to U+FF0D,
            // half-width katakana in ISO-2022-JP); everything else is exact.
            let folded = matches!(u32::from(character), 0xA5 | 0x203E | 0x2212 | 0xFF61..=0xFF9F | 0xE7C7 | 0xE78D..=0xE864);
            if !folded {
                assert_eq!(
                    text,
                    character.to_string(),
                    "{encoding} U+{:04X} -> {bytes:02X?}",
                    u32::from(character)
                );
            }
        }
        assert!(mapped >= 128, "{encoding}");
    }
}
