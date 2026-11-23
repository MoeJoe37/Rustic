//! Scripted window steps (tests): resize, minimize and restore a real
//! window and feed it keys, text, IME composition, clicks and wheel input
//! through the same handlers as the platform's own events.

use crate::{Ime, Input, Key, Modifiers, ScrollInput};

/// Most steps one script holds.
pub const MAX_STEPS: usize = 256;

/// One scripted step.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Ask the window system for this inner size in logical pixels.
    Resize(u32, u32),
    /// Minimize (iconify) the window.
    Minimize,
    /// Restore it and ask for focus.
    Restore,
    /// Present one more settled frame.
    Frame,
    /// A click at (x, y) logical pixels from the window's top-left corner.
    Click(f64, f64),
    /// An input as the platform would report it.
    Input(Input),
    /// Drop the window's drawing surface as a lost one is dropped (driver
    /// reset, display change, an Android/iOS suspend) and present again.
    LoseSurface,
}

/// A parsed script: `;`-separated steps, run one after another, each after
/// the previous one's frame is presented.
///
/// Steps: `resize WxH`, `minimize`, `restore`, `frame`, `click X,Y`,
/// `wheel DX,DY` (lines), `key [ctrl+][shift+][alt+][meta+]NAME` (NAME is
/// a single character or Enter, Escape, Backspace, Delete, Tab, Left,
/// Right, Up, Down, Home, End, PageUp, PageDown, Space or F1-F24),
/// `text TEXT`, `ime-on`, `preedit TEXT`, `commit TEXT`, `ime-off` and
/// `lose-surface`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Script {
    steps: Vec<Step>,
}

impl Script {
    /// Parse `text`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut steps = Vec::new();
        for raw in text.split(';') {
            let raw = raw.trim_start();
            if raw.trim().is_empty() {
                continue;
            }
            if steps.len() >= MAX_STEPS {
                return Err(format!("a window script holds at most {MAX_STEPS} steps"));
            }
            let (word, rest) = raw.split_once(' ').unwrap_or((raw.trim_end(), ""));
            let bad = || format!("bad window script step: {}", raw.trim());
            let pair = |sep: char| -> Result<(f64, f64), String> {
                let (a, b) = rest.trim().split_once(sep).ok_or_else(bad)?;
                let a: f64 = a.parse().map_err(|_| bad())?;
                let b: f64 = b.parse().map_err(|_| bad())?;
                if a.is_finite() && b.is_finite() {
                    Ok((a, b))
                } else {
                    Err(bad())
                }
            };
            steps.push(match word {
                "resize" => {
                    let (w, h) = pair('x')?;
                    if !(64.0..=8192.0).contains(&w) || !(64.0..=8192.0).contains(&h) {
                        return Err(bad());
                    }
                    Step::Resize(w as u32, h as u32)
                }
                "minimize" => Step::Minimize,
                "restore" => Step::Restore,
                "frame" => Step::Frame,
                "click" => {
                    let (x, y) = pair(',')?;
                    Step::Click(x, y)
                }
                "wheel" => {
                    let (x, y) = pair(',')?;
                    Step::Input(Input::Scroll(ScrollInput::Lines(x, y)))
                }
                "key" => {
                    let (key, modifiers) = parse_key(rest.trim()).ok_or_else(bad)?;
                    Step::Input(Input::Key(key, modifiers))
                }
                "text" if !rest.is_empty() => Step::Input(Input::Text(rest.to_owned())),
                "ime-on" => Step::Input(Input::Ime(Ime::Enabled)),
                "preedit" => {
                    let cursor = Some((rest.len(), rest.len()));
                    Step::Input(Input::Ime(Ime::Preedit(rest.to_owned(), cursor)))
                }
                "commit" => Step::Input(Input::Ime(Ime::Commit(rest.to_owned()))),
                "ime-off" => Step::Input(Input::Ime(Ime::Disabled)),
                "lose-surface" => Step::LoseSurface,
                _ => return Err(bad()),
            });
        }
        Ok(Self { steps })
    }

    /// The steps in order.
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }
}

/// Parse `ctrl+shift+t`-style key names.
pub fn parse_key(text: &str) -> Option<(Key, Modifiers)> {
    let mut modifiers = Modifiers::default();
    let mut parts: Vec<&str> = text.split('+').collect();
    // "ctrl++" names the plus key.
    if text.ends_with("++") {
        parts.truncate(parts.len().saturating_sub(2));
        parts.push("+");
    }
    let name = parts.pop()?;
    for part in parts {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers.ctrl = true,
            "shift" => modifiers.shift = true,
            "alt" | "option" => modifiers.alt = true,
            "meta" | "cmd" | "command" | "super" => modifiers.meta = true,
            // The platform's shortcut modifier.
            "mod" => {
                if cfg!(target_os = "macos") {
                    modifiers.meta = true;
                } else {
                    modifiers.ctrl = true;
                }
            }
            _ => return None,
        }
    }
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Some((Key::Character(c.to_ascii_lowercase()), modifiers));
    }
    let key = match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => Key::Enter,
        "escape" | "esc" => Key::Escape,
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "tab" => Key::Tab,
        "left" => Key::Left,
        "right" => Key::Right,
        "up" => Key::Up,
        "down" => Key::Down,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "space" => Key::Space,
        other => {
            let n: u8 = other.strip_prefix('f')?.parse().ok()?;
            if !(1..=24).contains(&n) {
                return None;
            }
            Key::Function(n)
        }
    };
    Some((key, modifiers))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_parse_every_step_kind() {
        let script = Script::parse(
            "resize 640x480; minimize;restore; frame; click 10,20.5; wheel 0,-3; key ctrl+shift+t; \
             key F5; key ctrl++; text hi there; ime-on; preedit にほ; commit 日本; ime-off",
        )
        .unwrap();
        let ctrl_shift = Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(
            script.steps(),
            &[
                Step::Resize(640, 480),
                Step::Minimize,
                Step::Restore,
                Step::Frame,
                Step::Click(10.0, 20.5),
                Step::Input(Input::Scroll(ScrollInput::Lines(0.0, -3.0))),
                Step::Input(Input::Key(Key::Character('t'), ctrl_shift)),
                Step::Input(Input::Key(Key::Function(5), Modifiers::default())),
                Step::Input(Input::Key(
                    Key::Character('+'),
                    Modifiers {
                        ctrl: true,
                        ..Modifiers::default()
                    }
                )),
                Step::Input(Input::Text("hi there".into())),
                Step::Input(Input::Ime(Ime::Enabled)),
                Step::Input(Input::Ime(Ime::Preedit("にほ".into(), Some((6, 6))))),
                Step::Input(Input::Ime(Ime::Commit("日本".into()))),
                Step::Input(Input::Ime(Ime::Disabled)),
            ]
        );
    }

    #[test]
    fn bad_steps_are_rejected() {
        for bad in [
            "resize 10x10",
            "resize axb",
            "click 1",
            "click NaN,1",
            "key hyper+a",
            "key F25",
            "key nothing",
            "fly",
            "text",
        ] {
            assert!(Script::parse(bad).is_err(), "{bad}");
        }
        assert!(Script::parse(&"frame;".repeat(MAX_STEPS + 1)).is_err());
        assert_eq!(Script::parse(" ; ").unwrap().steps(), &[]);
    }
}
