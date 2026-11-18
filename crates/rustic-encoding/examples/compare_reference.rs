//! Compare rustic-encoding with a reference recorded by
//! `tools/encoding_reference.js` (docs/ENCODING.md, "Reference comparison").
//!
//! Prints every difference and a count per encoding. Known, documented
//! departures of the reference from the standard are listed in the doc.

use std::collections::BTreeMap;
use std::io::BufRead;

use rustic_encoding::Encoding;

fn hex_bytes(text: &str) -> Option<Vec<u8>> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect()
}

fn code_points(text: &str) -> String {
    text.chars()
        .map(|c| format!("{:x}", u32::from(c)))
        .collect::<Vec<_>>()
        .join(",")
}

fn parse_code_points(text: &str) -> Option<String> {
    if text.is_empty() {
        return Some(String::new());
    }
    text.split(',')
        .map(|hex| char::from_u32(u32::from_str_radix(hex, 16).ok()?))
        .collect()
}

/// URL query percent-encoding (special schemes) through an encoder, with
/// unmappable scalar values written as `%26%23NNN%3B`.
fn query(text: &str, encoding: Encoding) -> String {
    let mut encoder = encoding.new_encoder();
    let mut bytes = Vec::new();
    let mut output = String::new();
    let flush = |bytes: &mut Vec<u8>, output: &mut String| {
        for &byte in bytes.iter() {
            if !(0x21..=0x7E).contains(&byte) || matches!(byte, b'"' | b'#' | b'<' | b'>' | b'\'') {
                output.push_str(&format!("%{byte:02X}"));
            } else {
                output.push(char::from(byte));
            }
        }
        bytes.clear();
    };
    for character in text.chars() {
        if let Err(unmappable) = encoder.encode_char(character, &mut bytes) {
            flush(&mut bytes, &mut output);
            output.push_str(&format!("%26%23{}%3B", u32::from(unmappable)));
        }
    }
    encoder.finish(&mut bytes);
    flush(&mut bytes, &mut output);
    output
}

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: compare_reference REFERENCE.txt");
        std::process::exit(2);
    };
    let file = match std::fs::File::open(&path) {
        Ok(file) => std::io::BufReader::new(file),
        Err(error) => {
            eprintln!("{path}: {error}");
            std::process::exit(2);
        }
    };
    let (mut agree, mut differ) = (0u64, 0u64);
    let mut per_kind = BTreeMap::<String, u64>::new();
    for line in file.lines().map_while(Result::ok) {
        let mut fields = line.splitn(4, ' ');
        let (Some(kind), Some(name)) = (fields.next(), fields.next()) else {
            continue;
        };
        let (input, expected) = (fields.next().unwrap_or(""), fields.next().unwrap_or(""));
        let Some(encoding) = Encoding::for_label(name) else {
            println!("unknown encoding {name}");
            differ += 1;
            continue;
        };
        let actual = match kind {
            "D" => hex_bytes(input).map(|bytes| code_points(&encoding.decode_without_bom(&bytes))),
            "E" => u32::from_str_radix(input, 16)
                .ok()
                .and_then(char::from_u32)
                .map(|c| query(&c.to_string(), encoding)),
            "S" => parse_code_points(input).map(|text| query(&text, encoding)),
            _ => None,
        };
        if actual.as_deref() == Some(expected) {
            agree += 1;
        } else {
            differ += 1;
            *per_kind.entry(format!("{kind} {name}")).or_default() += 1;
            println!(
                "{kind} {name} {input}: reference {expected}, rustic {}",
                actual.unwrap_or_default()
            );
        }
    }
    for (kind, count) in per_kind {
        println!("{kind}: {count} differences");
    }
    println!("{agree} agree, {differ} differ");
}
