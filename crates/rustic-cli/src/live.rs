//! `drive`: a live page without a window (TD-182, ADR 0069).
//!
//! The page loads once in a long-lived renderer ([`crate::broker::live`])
//! and a script of host input runs against it: frames, clicks, keys,
//! scrolls, resizes and clock steps, the same requests the desktop window
//! sends. Each frame prints one line; `probe` prints a pixel of the last
//! frame; `--output` writes the last frame as a PNG. Tests and automation
//! use it on every platform, including ones without a window system.

use std::ffi::OsString;
use std::path::Path;

use rustic_base::EscapedText;
use rustic_ipc::protocol::{
    FrameAsk, MODIFIER_ALT, MODIFIER_CONTROL, MODIFIER_META, MODIFIER_SHIFT,
};

/// Most steps in one `--input` script.
pub const MAX_STEPS: usize = 1_000;

/// One step of an `--input` script.
#[derive(Clone, Debug, PartialEq)]
enum Step {
    /// Ask for a frame.
    Frame,
    /// Let `ms` milliseconds pass before the next frame.
    Wait(u64),
    /// A primary click at window CSS pixels.
    Click(f64, f64, u8),
    /// Press and release a key: DOM `key`, `code`, modifiers.
    Key(String, String, u8),
    /// The user scrolls the viewport (applied with the next frame).
    Scroll(f64, f64),
    /// The window's size in CSS pixels changes.
    Resize(u32, u32),
    /// The page zoom changes.
    Zoom(f64),
    /// Print the RGBA of the last frame's device pixel.
    Probe(u32, u32),
}

fn pair<T: std::str::FromStr>(text: &str, separator: char) -> Option<(T, T)> {
    let (a, b) = text.split_once(separator)?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// Modifier prefixes such as `Control+` and `Shift+` before a key.
fn modifiers(text: &str) -> (u8, &str) {
    let mut bits = 0;
    let mut rest = text;
    loop {
        let next = [
            ("Control+", MODIFIER_CONTROL),
            ("Alt+", MODIFIER_ALT),
            ("Shift+", MODIFIER_SHIFT),
            ("Meta+", MODIFIER_META),
        ]
        .iter()
        .find_map(|(prefix, bit)| rest.strip_prefix(prefix).map(|after| (after, *bit)));
        match next {
            Some((after, bit)) if !after.is_empty() => {
                bits |= bit;
                rest = after;
            }
            _ => return (bits, rest),
        }
    }
}

/// The DOM `code` for a `key` typed without one: letters `KeyX`, digits
/// `DigitN`, a space `Space`, named keys their own name.
fn code_for(key: &str) -> String {
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => format!("Key{}", c.to_ascii_uppercase()),
        (Some(c), None) if c.is_ascii_digit() => format!("Digit{c}"),
        (Some(' '), None) => "Space".to_owned(),
        _ => key.to_owned(),
    }
}

fn parse_script(script: &str) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    for part in script
        .split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let (word, rest) = part.split_once(' ').unwrap_or((part, ""));
        let rest = rest.trim();
        let bad = || format!("--input: cannot read {}", EscapedText(part));
        let finite = |(x, y): (f64, f64)| (x.is_finite() && y.is_finite()).then_some((x, y));
        let step = match word {
            "frame" if rest.is_empty() => Step::Frame,
            "wait" => Step::Wait(
                rest.parse()
                    .ok()
                    .filter(|&ms| ms <= 3_600_000)
                    .ok_or_else(bad)?,
            ),
            "click" => {
                let (bits, at) = modifiers(rest);
                let (x, y) = pair(at, ',').and_then(finite).ok_or_else(bad)?;
                Step::Click(x, y, bits)
            }
            "key" if !rest.is_empty() => {
                let (bits, key) = modifiers(rest);
                // `key NAME` or `key NAME:CODE`; a lone `:` is the key.
                let (key, code) = match key.rsplit_once(':') {
                    Some((key, code)) if !key.is_empty() && !code.is_empty() => {
                        (key.to_owned(), code.to_owned())
                    }
                    _ => (key.to_owned(), code_for(key)),
                };
                let key = if key == "Space" { " ".to_owned() } else { key };
                Step::Key(key, code, bits)
            }
            "scroll" => {
                let (x, y) = pair(rest, ',').and_then(finite).ok_or_else(bad)?;
                Step::Scroll(x, y)
            }
            "resize" => {
                let (w, h) = pair::<u32>(rest, 'x')
                    .filter(|&(w, h)| (1..=16_384).contains(&w) && (1..=16_384).contains(&h))
                    .ok_or_else(bad)?;
                Step::Resize(w, h)
            }
            "zoom" => Step::Zoom(
                rest.parse()
                    .ok()
                    .filter(|zoom| (0.25..=5.0).contains(zoom))
                    .ok_or_else(bad)?,
            ),
            "probe" => {
                let (x, y) = pair::<u32>(rest, ',').ok_or_else(bad)?;
                Step::Probe(x, y)
            }
            _ => return Err(bad()),
        };
        steps.push(step);
        if steps.len() > MAX_STEPS {
            return Err(format!("--input: at most {MAX_STEPS} steps"));
        }
    }
    Ok(steps)
}

/// `rustic drive [--window WxH] [--scale S] [--zoom Z] --input SCRIPT
/// [--output PNG] [OPTIONS] <file>`.
pub(crate) fn drive_command(args: &[OsString]) -> u8 {
    let usage = |message: &str| {
        eprintln!("rustic drive: {message}");
        2
    };
    let mut size = (800u32, 600u32);
    let mut scale = 1.0f64;
    let mut zoom = 1.0f64;
    let mut script = None;
    let mut output = None;
    let mut rest = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let value = |iter: &mut std::slice::Iter<'_, OsString>| {
            iter.next()
                .and_then(|value| value.to_str())
                .map(str::to_owned)
        };
        match arg.to_str() {
            Some("--window") => match value(&mut iter)
                .as_deref()
                .and_then(|v| pair::<u32>(v, 'x'))
            {
                Some((w, h)) if (1..=16_384).contains(&w) && (1..=16_384).contains(&h) => {
                    size = (w, h);
                }
                _ => return usage("--window requires WxH with each side from 1 to 16384"),
            },
            Some("--scale") => match value(&mut iter).and_then(|v| v.parse::<f64>().ok()) {
                Some(s) if (0.25..=8.0).contains(&s) => scale = s,
                _ => return usage("--scale requires a number from 0.25 to 8"),
            },
            Some("--zoom") => match value(&mut iter).and_then(|v| v.parse::<f64>().ok()) {
                Some(z) if (0.25..=5.0).contains(&z) => zoom = z,
                _ => return usage("--zoom requires a number from 0.25 to 5"),
            },
            Some("--input") => match value(&mut iter).map(|v| parse_script(&v)) {
                Some(Ok(steps)) => script = Some(steps),
                Some(Err(error)) => return usage(&error),
                None => return usage("--input requires a script"),
            },
            Some("--output") => match iter.next() {
                Some(path) => output = Some(path.clone()),
                None => return usage("--output requires a file"),
            },
            Some("--viewport" | "--frames" | "--click" | "--scroll" | "--report-scroll") => {
                return usage("the --input script drives the page; see rustic --help");
            }
            _ => rest.push(arg.clone()),
        }
    }
    let Some(steps) = script else {
        return usage("missing --input SCRIPT");
    };
    let ran = crate::broker::live(&rest, |page| -> Result<(), String> {
        let mut report = String::new();
        let mut elapsed_us = 0u64;
        let mut scroll = None;
        let mut last: Option<rustic_raster::Surface> = None;
        let mut count = 0usize;
        let notes = |page: &mut crate::broker::LivePage<'_>| {
            for note in page.take_notes() {
                eprintln!("rustic drive: {note}");
            }
        };
        for step in &steps {
            let result = match *step {
                Step::Frame => {
                    let ask = FrameAsk {
                        width: size.0,
                        height: size.1,
                        scale,
                        zoom,
                        scroll: scroll.take(),
                        elapsed_us: std::mem::take(&mut elapsed_us),
                    };
                    page.frame(ask).map(|frame| {
                        let next = frame.next_frame.map_or_else(
                            || "none".to_owned(),
                            |delay| format!("{}ms", delay.as_millis()),
                        );
                        report.push_str(&format!(
                            "frame {count}: {}x{} scroll {},{} next {next}\n",
                            frame.surface.width(),
                            frame.surface.height(),
                            frame.scroll.0,
                            frame.scroll.1,
                        ));
                        count += 1;
                        last = Some(frame.surface);
                    })
                }
                Step::Wait(ms) => {
                    elapsed_us = elapsed_us.saturating_add(ms.saturating_mul(1_000));
                    Ok(())
                }
                Step::Click(x, y, bits) => page.click(x, y, bits),
                Step::Key(ref key, ref code, bits) => page
                    .key(true, key, code, false, bits)
                    .and_then(|()| page.key(false, key, code, false, bits)),
                Step::Scroll(x, y) => {
                    scroll = Some((x, y));
                    Ok(())
                }
                Step::Resize(w, h) => {
                    size = (w, h);
                    Ok(())
                }
                Step::Zoom(z) => {
                    zoom = z;
                    Ok(())
                }
                Step::Probe(x, y) => {
                    let pixel = last.as_ref().and_then(|surface| {
                        (x < surface.width() && y < surface.height()).then(|| {
                            let i = ((y * surface.width() + x) * 4) as usize;
                            let p = &surface.premultiplied()[i..i + 4];
                            (p[0], p[1], p[2], p[3])
                        })
                    });
                    report.push_str(&match pixel {
                        Some((r, g, b, a)) => format!("probe {x},{y}: {r},{g},{b},{a}\n"),
                        None => format!("probe {x},{y}: none\n"),
                    });
                    Ok(())
                }
            };
            notes(page);
            result?;
        }
        if page.restarts() > 0 {
            report.push_str(&format!("renderer restarts: {}\n", page.restarts()));
        }
        print!("{report}");
        if let (Some(path), Some(surface)) = (&output, &last) {
            let png = rustic_raster::encode_png(surface, usize::MAX).map_err(|e| e.to_string())?;
            std::fs::write(path, png).map_err(|error| {
                format!(
                    "{}: {error}",
                    EscapedText(&Path::new(path).to_string_lossy())
                )
            })?;
        }
        Ok(())
    });
    match ran {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => {
            eprintln!("rustic drive: {}", EscapedText(&error));
            1
        }
        Err(status) => status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_parse_every_step_and_refuse_bad_ones() {
        let steps = parse_script(
            "frame; wait 500; click 10,20.5; key Enter; key a; key Control+Shift+z; \
                          key Space; key ::Semicolon; scroll 0,300; resize 640x480; zoom 1.5; \
                          probe 3,4",
        )
        .unwrap();
        assert_eq!(
            steps,
            [
                Step::Frame,
                Step::Wait(500),
                Step::Click(10.0, 20.5, 0),
                Step::Key("Enter".into(), "Enter".into(), 0),
                Step::Key("a".into(), "KeyA".into(), 0),
                Step::Key("z".into(), "KeyZ".into(), MODIFIER_CONTROL | MODIFIER_SHIFT),
                Step::Key(" ".into(), "Space".into(), 0),
                Step::Key(":".into(), "Semicolon".into(), 0),
                Step::Scroll(0.0, 300.0),
                Step::Resize(640, 480),
                Step::Zoom(1.5),
                Step::Probe(3, 4),
            ]
        );
        for bad in [
            "frame 2",
            "click 1",
            "click NaN,1",
            "resize 0x10",
            "zoom 9",
            "wait -1",
            "key",
            "jump",
        ] {
            assert!(parse_script(bad).is_err(), "{bad}");
        }
        assert!(parse_script(&"frame;".repeat(MAX_STEPS + 1)).is_err());
    }
}
