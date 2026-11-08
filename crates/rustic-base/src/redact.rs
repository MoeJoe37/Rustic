use std::fmt;

/// Whether a diagnostic value may appear in ordinary logs.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Sensitivity {
    /// Engine-chosen text such as a tag name category, limit or state name.
    Public,
    /// Page content, URLs, credentials, file paths, form data or anything a
    /// site, user or embedder supplied. Redacted unless explicitly revealed.
    Private,
}

/// How rendering treats [`Sensitivity::Private`] values.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RedactionPolicy {
    /// Replace every private value with `<redacted>`. Use for logs, crash
    /// reports, telemetry and anything that may leave the local session.
    #[default]
    Redact,
    /// Show private values, escaped. Only for an explicit local developer
    /// opt-in where the viewer already has access to the content.
    Reveal,
}

/// The fixed text that replaces a redacted value.
///
/// It intentionally omits the value's length, which can itself be sensitive.
pub const REDACTED: &str = "<redacted>";

/// A diagnostic value that carries its own sensitivity.
///
/// `Debug` never prints a private value, so deriving `Debug` on containing types
/// cannot leak it. Read it with [`Self::reveal`] or render it under a policy.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct DiagnosticValue {
    sensitivity: Sensitivity,
    text: String,
}

impl DiagnosticValue {
    /// A value safe for ordinary logs.
    pub fn public(text: impl Into<String>) -> Self {
        Self {
            sensitivity: Sensitivity::Public,
            text: text.into(),
        }
    }

    /// A value that must be redacted by default.
    pub fn private(text: impl Into<String>) -> Self {
        Self {
            sensitivity: Sensitivity::Private,
            text: text.into(),
        }
    }

    /// This value's sensitivity.
    pub const fn sensitivity(&self) -> Sensitivity {
        self.sensitivity
    }

    /// The raw, unescaped text. Callers take responsibility for its handling.
    pub fn reveal(&self) -> &str {
        &self.text
    }

    /// The text as shown under `policy`, before escaping.
    pub fn display_text(&self, policy: RedactionPolicy) -> &str {
        match (self.sensitivity, policy) {
            (Sensitivity::Private, RedactionPolicy::Redact) => REDACTED,
            _ => &self.text,
        }
    }

    /// Raw text length in bytes, for resource accounting only.
    pub(crate) fn byte_len(&self) -> usize {
        self.text.len()
    }
}

impl fmt::Debug for DiagnosticValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.sensitivity {
            Sensitivity::Public => formatter
                .debug_tuple("Public")
                .field(&EscapedText(&self.text))
                .finish(),
            Sensitivity::Private => formatter.debug_tuple("Private").field(&REDACTED).finish(),
        }
    }
}

/// Displays text as one inert, quoted line safe for terminals and log files.
///
/// Backslash, quote, C0/C1 controls, DEL, line/paragraph separators and bidi
/// override/isolate controls are written as escapes, so content cannot forge
/// extra log lines, move the cursor or visually reorder surrounding text.
/// Other characters, including non-Latin scripts, are kept as they are.
#[derive(Clone, Copy)]
pub struct EscapedText<'a>(pub &'a str);

impl fmt::Display for EscapedText<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        use fmt::Write;
        formatter.write_char('"')?;
        write_escaped(formatter, self.0)?;
        formatter.write_char('"')
    }
}

/// Write `text` with [`EscapedText`]'s escapes but without surrounding quotes.
pub(crate) fn write_escaped(output: &mut impl fmt::Write, text: &str) -> fmt::Result {
    let mut clean_start = 0;
    for (index, character) in text.char_indices() {
        let escape = match character {
            '"' => Some(Escape::Literal("\\\"")),
            '\\' => Some(Escape::Literal("\\\\")),
            '\n' => Some(Escape::Literal("\\n")),
            '\r' => Some(Escape::Literal("\\r")),
            '\t' => Some(Escape::Literal("\\t")),
            _ if needs_code_escape(character) => Some(Escape::Code(character)),
            _ => None,
        };
        if let Some(escape) = escape {
            // `index` and `clean_start` are both char boundaries of `text`.
            output.write_str(&text[clean_start..index])?;
            match escape {
                Escape::Literal(literal) => output.write_str(literal)?,
                Escape::Code(character) => write!(output, "\\u{{{:x}}}", u32::from(character))?,
            }
            clean_start = index + character.len_utf8();
        }
    }
    output.write_str(&text[clean_start..])
}

impl fmt::Debug for EscapedText<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

enum Escape {
    Literal(&'static str),
    Code(char),
}

fn needs_code_escape(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            // Line and paragraph separators.
            '\u{2028}' | '\u{2029}'
            // Bidi embeddings, overrides, isolates and marks.
            | '\u{200e}' | '\u{200f}' | '\u{061c}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
            // Zero-width characters that can hide text, and the BOM.
            | '\u{200b}'..='\u{200d}' | '\u{2060}' | '\u{feff}'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_values_never_appear_in_debug_output() {
        let secret = DiagnosticValue::private("https://user:hunter2@example.test/");
        let debug = format!("{secret:?}");
        assert_eq!(debug, "Private(\"<redacted>\")");
        assert!(!debug.contains("hunter2"));
        assert_eq!(secret.display_text(RedactionPolicy::Redact), REDACTED);
        assert_eq!(
            secret.display_text(RedactionPolicy::Reveal),
            "https://user:hunter2@example.test/"
        );
        assert_eq!(RedactionPolicy::default(), RedactionPolicy::Redact);
        let public = DiagnosticValue::public("div");
        assert_eq!(format!("{public:?}"), "Public(\"div\")");
        assert_eq!(public.display_text(RedactionPolicy::Redact), "div");
    }

    #[test]
    fn escaping_neutralizes_log_and_terminal_injection() {
        let hostile = "a\"b\\c\nFAKE: ok\r\t\u{1b}[2J\u{7f}\u{85}\u{2028}\u{202e}evil\u{2066}\u{200b}\u{feff}";
        assert_eq!(
            EscapedText(hostile).to_string(),
            "\"a\\\"b\\\\c\\nFAKE: ok\\r\\t\\u{1b}[2J\\u{7f}\\u{85}\\u{2028}\\u{202e}evil\\u{2066}\\u{200b}\\u{feff}\""
        );
        let rendered = EscapedText(hostile).to_string();
        assert!(!rendered.chars().any(needs_code_escape));
        assert!(!rendered.contains('\n'));
    }

    #[test]
    fn escaping_keeps_ordinary_unicode() {
        assert_eq!(
            EscapedText("مرحبا world é 😀").to_string(),
            "\"مرحبا world é 😀\""
        );
        assert_eq!(EscapedText("").to_string(), "\"\"");
    }
}
