//! The deterministic text form of an [`AxTree`] (`rustic dump-a11y`).

use std::fmt::Write as _;

use crate::{AxTree, Checked, DumpLimit, Live};

pub(crate) fn dump(tree: &AxTree, max_bytes: usize) -> Result<String, DumpLimit> {
    let index = tree.index();
    let mut out = String::new();
    let mut stack = vec![(0usize, 0usize)];
    while let Some((i, depth)) = stack.pop() {
        let node = &tree.nodes[i];
        let mut line = String::new();
        for _ in 0..depth {
            line.push_str("  ");
        }
        line.push_str(node.role.as_str());
        if !node.name.is_empty() {
            line.push(' ');
            quote(&mut line, &node.name);
        }
        let mut field = |key: &str, value: &Option<String>| {
            if let Some(value) = value {
                line.push(' ');
                line.push_str(key);
                line.push('=');
                quote(&mut line, value);
            }
        };
        field(
            "description",
            &(!node.description.is_empty()).then(|| node.description.clone()),
        );
        field("value", &node.value);
        field("placeholder", &node.placeholder);
        field("url", &node.url);
        if let Some(level) = node.level {
            let _ = write!(line, " level={level}");
        }
        let s = &node.states;
        for (on, word) in [
            (s.focusable, "focusable"),
            (s.focused, "focused"),
            (s.disabled, "disabled"),
            (s.required, "required"),
            (s.invalid, "invalid"),
            (s.readonly, "readonly"),
            (s.editable, "editable"),
            (s.multiline, "multiline"),
            (s.multiselectable, "multiselectable"),
            (s.modal, "modal"),
            (s.busy, "busy"),
            (s.protected, "protected"),
        ] {
            if on {
                line.push(' ');
                line.push_str(word);
            }
        }
        let tri = |value: Checked| match value {
            Checked::True => "true",
            Checked::False => "false",
            Checked::Mixed => "mixed",
        };
        if let Some(checked) = s.checked {
            let _ = write!(line, " checked={}", tri(checked));
        }
        if let Some(pressed) = s.pressed {
            let _ = write!(line, " pressed={}", tri(pressed));
        }
        if let Some(expanded) = s.expanded {
            let _ = write!(line, " expanded={expanded}");
        }
        if let Some(selected) = s.selected {
            let _ = write!(line, " selected={selected}");
        }
        if let Some(live) = node.live {
            line.push_str(match live {
                Live::Polite => " live=polite",
                Live::Assertive => " live=assertive",
            });
        }
        if let Some(range) = node.range {
            let show = |v: Option<f64>| v.map_or_else(|| "?".to_owned(), |v| format!("{v}"));
            let _ = write!(
                line,
                " range={}..{} now={}",
                show(range.min),
                show(range.max),
                show(range.now)
            );
        }
        if let Some((anchor, focus)) = node.selection {
            let _ = write!(line, " selection={anchor}..{focus}");
        }
        if let Some(b) = node.bounds {
            // Two decimals: layout's float noise stays out of dumps.
            let r = |v: f64| (v * 100.0).round() / 100.0 + 0.0;
            let _ = write!(
                line,
                " @{},{} {}x{}",
                r(b.x),
                r(b.y),
                r(b.width),
                r(b.height)
            );
        }
        line.push('\n');
        if out.len() + line.len() > max_bytes {
            return Err(DumpLimit);
        }
        out.push_str(&line);
        for child in node.children.iter().rev() {
            if let Some(&c) = index.get(child) {
                stack.push((c, depth + 1));
            }
        }
    }
    if tree.truncated {
        let note = "(truncated)\n";
        if out.len() + note.len() > max_bytes {
            return Err(DumpLimit);
        }
        out.push_str(note);
    }
    Ok(out)
}

/// Append `text` in double quotes, escaping quotes, backslashes and
/// control characters so page text cannot forge dump lines.
fn quote(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
