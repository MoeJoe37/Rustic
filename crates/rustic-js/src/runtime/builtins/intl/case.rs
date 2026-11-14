//! Context-sensitive Unicode SpecialCasing rules for az, lt and tr.

use rustic_base::unicode::combining_class;

use super::data::SOFT_DOTTED;

fn soft_dotted(code: u32) -> bool {
    let index = SOFT_DOTTED.partition_point(|&(_, last)| last < code);
    SOFT_DOTTED
        .get(index)
        .is_some_and(|&(first, _)| code >= first)
}

pub(super) fn tailor(units: &[u16], language: &str, upper: bool) -> Vec<u16> {
    let turkic = matches!(language, "az" | "tr");
    let code_points = char::decode_utf16(units.iter().copied())
        .map(|code| {
            code.map(u32::from)
                .unwrap_or_else(|e| u32::from(e.unpaired_surrogate()))
        })
        .collect::<Vec<_>>();
    // Scan both ways once. Class 0 and 230 delimit the context that matters
    // for Turkish, and class 0 delimits Lithuanian's More_Above condition.
    let mut following = vec![false; code_points.len()];
    let mut next_boundary = None;
    let mut above = false;
    for (index, &code) in code_points.iter().enumerate().rev() {
        following[index] = if turkic {
            next_boundary == Some(0x307)
        } else {
            above
        };
        let class = combining_class(code);
        if class == 0 || class == 230 {
            next_boundary = Some(code);
        }
        above = match class {
            0 => false,
            230 => true,
            _ => above,
        };
    }
    let mut previous_boundary = None;
    let mut result = Vec::with_capacity(units.len());
    for (code, following) in code_points.into_iter().zip(following) {
        let mut emit = |code| {
            if let Some(c) = char::from_u32(code) {
                result.extend_from_slice(c.encode_utf16(&mut [0; 2]));
            } else {
                result.push(u16::try_from(code).expect("unpaired surrogate"));
            }
        };
        match code {
            0x49 if turkic && !upper => emit(if following { 0x69 } else { 0x131 }),
            0x49 | 0x4A | 0x12E if !turkic && !upper && following => {
                emit(code);
                emit(0x307);
            }
            0x307
                if (turkic && !upper && previous_boundary == Some(0x49))
                    || (!turkic && upper && previous_boundary.is_some_and(soft_dotted)) => {}
            0x130 if turkic && !upper => emit(0x69),
            0x69 if turkic && upper => emit(0x130),
            0xCC | 0xCD | 0x128 if !turkic && !upper => {
                emit(0x69);
                emit(0x307);
                emit(match code {
                    0xCC => 0x300,
                    0xCD => 0x301,
                    _ => 0x303,
                });
            }
            _ => emit(code),
        }
        if matches!(combining_class(code), 0 | 230) {
            previous_boundary = Some(code);
        }
    }
    result
}
