//! Read CLDR root collation elements and construct level-wise sort keys.

use rustic_base::unicode::{NormalizationForm, normalize_utf16};

use crate::runtime::{Agent, JsResult, OutOfMemory, StringId};

const KEYS: &[u8] = include_bytes!("../../../../../data/collation-keys.bin");
const ELEMENTS: &[u8] = include_bytes!("../../../../../data/collation-elements.bin");
const DIGITS: &[u8] = include_bytes!("../../../../../data/collation-digits.bin");
const KEY_SIZE: usize = 16;
const ELEMENT_SIZE: usize = 6;

#[derive(Clone, Copy, Debug)]
pub(super) struct Options {
    pub(super) numeric: bool,
    pub(super) ignore_punctuation: bool,
    pub(super) case_first: usize,
    pub(super) sensitivity: usize,
    pub(super) phonebook: bool,
}

#[derive(Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Key {
    primary: Vec<u32>,
    secondary: Vec<u32>,
    tertiary: Vec<u32>,
}

fn read_24(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0])
}

fn read_key(row: &[u8]) -> [u32; 4] {
    std::array::from_fn(|index| read_24(&row[index * 3..]))
}

fn lookup(points: &[u32]) -> Option<&'static [u8]> {
    let mut key = [0; 4];
    for (out, point) in key.iter_mut().zip(points) {
        *out = point + 1;
    }
    let mut low = 0;
    let mut high = KEYS.len() / KEY_SIZE;
    while low < high {
        let mid = low + (high - low) / 2;
        let entry = &KEYS[mid * KEY_SIZE..][..KEY_SIZE];
        match read_key(entry).cmp(&key) {
            std::cmp::Ordering::Less => low = mid + 1,
            std::cmp::Ordering::Greater => high = mid,
            std::cmp::Ordering::Equal => {
                let start = read_24(&entry[12..]) as usize * ELEMENT_SIZE;
                return Some(&ELEMENTS[start..][..usize::from(entry[15]) * ELEMENT_SIZE]);
            }
        }
    }
    None
}

fn digit(point: u32) -> Option<u32> {
    let mut low = 0;
    let mut high = DIGITS.len() / 4;
    while low < high {
        let mid = low + (high - low) / 2;
        let start = u32::from_le_bytes(DIGITS[mid * 4..][..4].try_into().expect("digit record"));
        if point < start {
            high = mid;
        } else if point - start < 10 {
            return Some(point - start);
        } else {
            low = mid + 1;
        }
    }
    None
}

impl Key {
    fn push(
        &mut self,
        agent: &mut Agent,
        primary: u32,
        secondary: u32,
        tertiary: u32,
        options: Options,
    ) -> JsResult<()> {
        if primary != 0 {
            agent.heap.charge(4)?;
            self.primary.push(primary);
        }
        if secondary != 0 && matches!(options.sensitivity, 0 | 2) {
            agent.heap.charge(4)?;
            self.secondary.push(secondary);
        }
        if tertiary != 0 && (options.sensitivity == 0 || (options.sensitivity == 3 && primary != 0))
        {
            // UCA reserves 0x08–0x0D for uppercase forms. Kana and other
            // compatibility variants use different tertiary weights.
            let upper = (8..=13).contains(&tertiary);
            let weight = match options.sensitivity {
                3 => u32::from(upper) + 1,
                _ => tertiary,
            };
            let first = match options.case_first {
                1 => !upper,
                2 => upper,
                _ => false,
            };
            agent.heap.charge(4)?;
            self.tertiary.push(weight + if first { 32 } else { 0 });
        }
        Ok(())
    }

    fn append_mapping(
        &mut self,
        agent: &mut Agent,
        mapping: &[u8],
        options: Options,
    ) -> JsResult<()> {
        for element in mapping.chunks_exact(ELEMENT_SIZE) {
            if options.ignore_punctuation && element[5] != 0 {
                continue;
            }
            let primary = u32::from(u16::from_le_bytes([element[0], element[1]]));
            let secondary = u32::from(u16::from_le_bytes([element[2], element[3]]));
            self.push(agent, primary, secondary, u32::from(element[4]), options)?;
        }
        Ok(())
    }
}

pub(super) fn key(agent: &mut Agent, input: StringId, options: Options) -> JsResult<Key> {
    let room = agent
        .heap
        .byte_limit()
        .saturating_sub(agent.heap.bytes_used());
    // Reserve space for the decomposition buffer, its sort scratch, and the
    // UTF-16 result. Poll before entering the Unicode routine as well.
    let max_decomposed = room / 16;
    let input_len = agent.heap.string_len(input);
    if input_len > max_decomposed.saturating_mul(2) {
        return Err(OutOfMemory.into());
    }
    for _ in 0..input_len {
        agent.check_native_work()?;
    }
    let units = normalize_utf16(
        agent.heap.string(input).units(),
        NormalizationForm::Nfd,
        max_decomposed,
        room / 6,
    )
    .ok_or(OutOfMemory)?;
    agent
        .heap
        .charge(units.len().checked_mul(6).ok_or(OutOfMemory)?)?;
    let points = char::decode_utf16(units)
        .map(|point| match point {
            Ok(point) => u32::from(point),
            Err(error) => u32::from(error.unpaired_surrogate()),
        })
        .collect::<Vec<_>>();
    let mut remaining = points.as_slice();
    let mut result = Key::default();
    while let Some((&first, rest)) = remaining.split_first() {
        agent.check_native_work()?;
        if options.numeric && digit(first).is_some() {
            let mut end = 0;
            let mut leading = 0;
            for &point in remaining {
                let Some(value) = digit(point) else { break };
                agent.check_native_work()?;
                if end == leading && value == 0 {
                    leading += 1;
                }
                end += 1;
            }
            let numbers = &remaining[leading..end];
            let length = u32::try_from(numbers.len()).map_err(|_| OutOfMemory)?;
            result.push(agent, 0x21e6, 0x20, 2, options)?;
            result.push(
                agent,
                length.checked_add(1).ok_or(OutOfMemory)?,
                0,
                0,
                options,
            )?;
            for &point in numbers {
                result.push(
                    agent,
                    digit(point).expect("decimal input") + 1,
                    0,
                    0,
                    options,
                )?;
            }
            remaining = &remaining[end..];
            continue;
        }
        if options.phonebook
            && rest.first() == Some(&0x308)
            && matches!(first, 0x41 | 0x4f | 0x55 | 0x61 | 0x6f | 0x75)
        {
            result.append_mapping(agent, lookup(&[first]).expect("ASCII mapping"), options)?;
            let e = if first < 0x61 { 0x45 } else { 0x65 };
            // Phonebook vowels expand to the base letter and e, with a
            // secondary distinction from a literal two-letter sequence.
            let mapping = lookup(&[e]).expect("ASCII mapping");
            let primary = u32::from(u16::from_le_bytes([mapping[0], mapping[1]]));
            result.push(agent, primary, 0x21, u32::from(mapping[4]), options)?;
            remaining = &rest[1..];
            continue;
        }
        if let Some((length, mapping)) = (1..=remaining.len().min(4))
            .rev()
            .find_map(|length| lookup(&remaining[..length]).map(|mapping| (length, mapping)))
        {
            result.append_mapping(agent, mapping, options)?;
            remaining = &remaining[length..];
        } else {
            let base = match first {
                0x4e00..=0x9fff => 0xfb40,
                0x3400..=0x4dbf | 0x20000..=0x2ee5f | 0x30000..=0x3347f => 0xfb80,
                _ => 0xfbc0,
            };
            result.push(agent, base + (first >> 15), 0x20, 2, options)?;
            result.push(agent, (first & 0x7fff) | 0x8000, 0, 0, options)?;
            remaining = rest;
        }
    }
    Ok(result)
}
