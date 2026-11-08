use crate::redact::{DiagnosticValue, EscapedText, RedactionPolicy};
use crate::{Budget, BudgetError, SourceLocation};
use std::fmt::{self, Write};

/// How serious a diagnostic is. Ordered from least to most severe.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Severity {
    /// Supplementary information.
    Note,
    /// Recovered or questionable input; processing continued.
    Warning,
    /// An operation failed or produced no result.
    Error,
}

impl Severity {
    /// Lower-case name used in rendered output.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// Longest permitted [`DiagnosticName`], in bytes.
pub const MAX_NAME_BYTES: usize = 64;

/// A stable machine-readable name for a diagnostic code or field key.
///
/// Names are 1–64 bytes of lower-case ASCII letters, digits, `.`, `-` and `_`,
/// starting with a letter, such as `html.eof-in-tag`. They are fixed engine
/// strings, so content can never be smuggled into a code or key.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DiagnosticName(&'static str);

impl DiagnosticName {
    /// Validate a name.
    pub const fn new(name: &'static str) -> Result<Self, InvalidName> {
        if is_valid_name(name.as_bytes()) {
            Ok(Self(name))
        } else {
            Err(InvalidName)
        }
    }

    /// Validate a name in a constant; an invalid literal fails compilation.
    ///
    /// ```
    /// use rustic_base::DiagnosticName;
    /// const EOF_IN_TAG: DiagnosticName = DiagnosticName::from_static("html.eof-in-tag");
    /// assert_eq!(EOF_IN_TAG.as_str(), "html.eof-in-tag");
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if `name` is invalid. Use [`Self::new`] outside constant contexts.
    pub const fn from_static(name: &'static str) -> Self {
        match Self::new(name) {
            Ok(name) => name,
            Err(_) => panic!("invalid diagnostic name"),
        }
    }

    /// The validated name.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

const fn is_valid_name(bytes: &[u8]) -> bool {
    if bytes.is_empty() || bytes.len() > MAX_NAME_BYTES || !bytes[0].is_ascii_lowercase() {
        return false;
    }
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if !(byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || byte == b'.'
            || byte == b'-'
            || byte == b'_')
        {
            return false;
        }
        index += 1;
    }
    true
}

/// A diagnostic name was empty, too long or used a disallowed character.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidName;

impl fmt::Display for InvalidName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid diagnostic name")
    }
}

impl std::error::Error for InvalidName {}

/// One structured diagnostic event.
///
/// The summary is a fixed engine string. Anything variable, and especially
/// anything content-derived, goes into a named field with an explicit
/// [`crate::Sensitivity`], so rendering can redact it. `Debug` output is
/// redacted too.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    severity: Severity,
    code: DiagnosticName,
    summary: &'static str,
    location: Option<SourceLocation>,
    fields: Vec<(DiagnosticName, DiagnosticValue)>,
}

impl Diagnostic {
    /// Create a diagnostic without a location or fields.
    pub const fn new(severity: Severity, code: DiagnosticName, summary: &'static str) -> Self {
        Self {
            severity,
            code,
            summary,
            location: None,
            fields: Vec::new(),
        }
    }

    /// Attach the source revision and span the diagnostic refers to.
    #[must_use]
    pub fn at(mut self, location: SourceLocation) -> Self {
        self.location = Some(location);
        self
    }

    /// Append a field. Fields keep insertion order; keys may repeat.
    #[must_use]
    pub fn with_field(mut self, key: DiagnosticName, value: DiagnosticValue) -> Self {
        self.fields.push((key, value));
        self
    }

    /// The severity.
    pub const fn severity(&self) -> Severity {
        self.severity
    }

    /// The stable diagnostic code.
    pub const fn code(&self) -> DiagnosticName {
        self.code
    }

    /// The fixed human-readable summary.
    pub const fn summary(&self) -> &'static str {
        self.summary
    }

    /// The source location, if any.
    pub const fn location(&self) -> Option<SourceLocation> {
        self.location
    }

    /// The fields, in insertion order.
    pub fn fields(&self) -> &[(DiagnosticName, DiagnosticValue)] {
        &self.fields
    }

    /// Accounted size: code, summary, field keys and raw (unredacted) values.
    ///
    /// Counting raw values means a policy change cannot alter accounting.
    pub fn accounted_bytes(&self) -> Result<usize, BudgetError> {
        let mut total = self
            .code
            .as_str()
            .len()
            .checked_add(self.summary.len())
            .ok_or(BudgetError::Exceeded)?;
        for (key, value) in &self.fields {
            total = total
                .checked_add(key.as_str().len())
                .and_then(|total| total.checked_add(value.byte_len()))
                .ok_or(BudgetError::Exceeded)?;
        }
        Ok(total)
    }

    /// Render one deterministic line under `policy`, at most `max_bytes` long.
    ///
    /// Format: `severity[code] source=S@rR bytes=A..B: summary key="value" ...`.
    /// The location part is omitted when absent. Field values are escaped with
    /// [`EscapedText`]; private values become `"<redacted>"` under
    /// [`RedactionPolicy::Redact`]. Exceeding `max_bytes` returns an error and
    /// no partial line, so a truncated line can never hide a trailing field.
    pub fn render(&self, policy: RedactionPolicy, max_bytes: usize) -> Result<String, RenderError> {
        let mut output = CappedString::new(max_bytes);
        self.write_line(policy, &mut output)
            .map_err(|_| RenderError::OutputLimit)?;
        Ok(output.text)
    }

    fn write_line(&self, policy: RedactionPolicy, output: &mut impl Write) -> fmt::Result {
        write!(output, "{}[{}]", self.severity.as_str(), self.code.as_str())?;
        if let Some(location) = self.location {
            let revision = location.revision();
            let range = location.span().range();
            write!(
                output,
                " source={}@r{} bytes={}..{}",
                revision.source().get(),
                revision.number(),
                range.start,
                range.end
            )?;
        }
        // The summary is engine-controlled, but escape it anyway so a careless
        // literal cannot break the one-line format.
        let summary = EscapedText(self.summary).to_string();
        write!(output, ": {}", &summary[1..summary.len() - 1])?;
        for (key, value) in &self.fields {
            write!(
                output,
                " {}={}",
                key.as_str(),
                EscapedText(value.display_text(policy))
            )?;
        }
        Ok(())
    }
}

/// A string writer that fails instead of growing past its byte cap.
struct CappedString {
    text: String,
    cap: usize,
}

impl CappedString {
    fn new(cap: usize) -> Self {
        Self {
            text: String::new(),
            cap,
        }
    }
}

impl Write for CappedString {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let len = self.text.len().checked_add(text.len()).ok_or(fmt::Error)?;
        if len > self.cap {
            return Err(fmt::Error);
        }
        self.text.push_str(text);
        Ok(())
    }
}

/// Rendering would exceed the caller's output byte limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderError {
    /// The rendered output is longer than the permitted number of bytes.
    OutputLimit,
}

impl fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("diagnostic output limit exceeded")
    }
}

impl std::error::Error for RenderError {}

/// A bounded, ordered diagnostic collection.
///
/// Both the entry count and [`Diagnostic::accounted_bytes`] are budgeted. A
/// rejected push leaves the log and its usage unchanged; the caller decides
/// whether a full log is fatal or the new entry is dropped.
#[derive(Debug)]
pub struct DiagnosticLog {
    entries: Vec<Diagnostic>,
    entry_budget: Budget,
    byte_budget: Budget,
}

impl DiagnosticLog {
    /// Create an empty log; zero limits reject every entry.
    pub const fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: Vec::new(),
            entry_budget: Budget::new(max_entries),
            byte_budget: Budget::new(max_bytes),
        }
    }

    /// Append `diagnostic` if both budgets allow it.
    pub fn push(&mut self, diagnostic: Diagnostic) -> Result<(), DiagnosticLogError> {
        let bytes = diagnostic
            .accounted_bytes()
            .map_err(|_| DiagnosticLogError::ByteLimit)?;
        if self.entry_budget.remaining() == 0 {
            return Err(DiagnosticLogError::EntryLimit);
        }
        if bytes > self.byte_budget.remaining() {
            return Err(DiagnosticLogError::ByteLimit);
        }
        self.entries
            .try_reserve(1)
            .map_err(|_| DiagnosticLogError::ByteLimit)?;
        // Both checks passed above, so neither charge can fail.
        self.entry_budget
            .charge(1)
            .map_err(|_| DiagnosticLogError::EntryLimit)?;
        self.byte_budget
            .charge(bytes)
            .map_err(|_| DiagnosticLogError::ByteLimit)?;
        self.entries.push(diagnostic);
        Ok(())
    }

    /// Entries in push order.
    pub fn entries(&self) -> &[Diagnostic] {
        &self.entries
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the log has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Accounted bytes currently used.
    pub const fn used_bytes(&self) -> usize {
        self.byte_budget.used()
    }

    /// The most severe entry's severity, if any.
    pub fn max_severity(&self) -> Option<Severity> {
        self.entries.iter().map(Diagnostic::severity).max()
    }

    /// Render all entries, one per line with a trailing newline, within
    /// `max_bytes`. Fails with no partial output, like [`Diagnostic::render`].
    pub fn render(&self, policy: RedactionPolicy, max_bytes: usize) -> Result<String, RenderError> {
        let mut output = CappedString::new(max_bytes);
        for diagnostic in &self.entries {
            diagnostic
                .write_line(policy, &mut output)
                .and_then(|()| output.write_char('\n'))
                .map_err(|_| RenderError::OutputLimit)?;
        }
        Ok(output.text)
    }

    /// Consume the log and return its entries.
    pub fn into_entries(self) -> Vec<Diagnostic> {
        self.entries
    }
}

/// A diagnostic did not fit in a [`DiagnosticLog`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticLogError {
    /// The entry count limit is reached.
    EntryLimit,
    /// The entry's accounted bytes exceed the remaining byte limit.
    ByteLimit,
}

impl fmt::Display for DiagnosticLogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EntryLimit => "diagnostic entry limit exceeded",
            Self::ByteLimit => "diagnostic byte limit exceeded",
        })
    }
}

impl std::error::Error for DiagnosticLogError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SourceId, SourceRevision, SourceSpan};

    const EOF_IN_TAG: DiagnosticName = DiagnosticName::from_static("html.eof-in-tag");
    const URL: DiagnosticName = DiagnosticName::from_static("url");
    const TAG: DiagnosticName = DiagnosticName::from_static("tag");

    fn sample() -> (Diagnostic, u64) {
        let source = SourceId::allocate().unwrap();
        let revision = SourceRevision::initial(source).next().unwrap();
        let location = SourceLocation::new(revision, SourceSpan::new(10, 14).unwrap());
        let diagnostic =
            Diagnostic::new(Severity::Warning, EOF_IN_TAG, "end of input inside a tag")
                .at(location)
                .with_field(TAG, DiagnosticValue::public("div"))
                .with_field(
                    URL,
                    DiagnosticValue::private("https://user:hunter2@example.test/\nerror[fake]"),
                );
        (diagnostic, source.get())
    }

    #[test]
    fn names_are_validated() {
        assert!(DiagnosticName::new("html.eof-in-tag_2").is_ok());
        for bad in ["", "Html", "1abc", "a b", "a\n", "a=b", "é"] {
            assert_eq!(DiagnosticName::new(bad), Err(InvalidName), "{bad:?}");
        }
        let longest: &'static str = "a".repeat(MAX_NAME_BYTES).leak();
        assert!(DiagnosticName::new(longest).is_ok());
        let too_long: &'static str = "a".repeat(MAX_NAME_BYTES + 1).leak();
        assert_eq!(DiagnosticName::new(too_long), Err(InvalidName));
    }

    #[test]
    fn rendering_is_redacted_by_default_and_single_line() {
        let (diagnostic, source) = sample();
        let redacted = diagnostic.render(RedactionPolicy::default(), 1024).unwrap();
        assert_eq!(
            redacted,
            format!(
                "warning[html.eof-in-tag] source={source}@r1 bytes=10..14: end of input inside a tag tag=\"div\" url=\"<redacted>\""
            )
        );
        assert!(!format!("{diagnostic:?}").contains("hunter2"));
        let revealed = diagnostic.render(RedactionPolicy::Reveal, 1024).unwrap();
        assert!(revealed.ends_with("url=\"https://user:hunter2@example.test/\\nerror[fake]\""));
        assert!(!revealed.contains('\n'));
    }

    #[test]
    fn rendering_without_location_or_fields() {
        let diagnostic = Diagnostic::new(Severity::Error, EOF_IN_TAG, "say \"hi\"\n");
        assert_eq!(
            diagnostic.render(RedactionPolicy::Redact, 100).unwrap(),
            "error[html.eof-in-tag]: say \\\"hi\\\"\\n"
        );
    }

    #[test]
    fn render_limit_is_exact_and_atomic() {
        let (diagnostic, _) = sample();
        let full = diagnostic
            .render(RedactionPolicy::Redact, usize::MAX)
            .unwrap();
        assert_eq!(
            diagnostic.render(RedactionPolicy::Redact, full.len()),
            Ok(full.clone())
        );
        assert_eq!(
            diagnostic.render(RedactionPolicy::Redact, full.len() - 1),
            Err(RenderError::OutputLimit)
        );
        assert_eq!(
            diagnostic.render(RedactionPolicy::Redact, 0),
            Err(RenderError::OutputLimit)
        );
    }

    #[test]
    fn log_budgets_are_exact_and_rejections_are_atomic() {
        let (diagnostic, _) = sample();
        let bytes = diagnostic.accounted_bytes().unwrap();
        assert_eq!(
            bytes,
            "html.eof-in-tag".len()
                + "end of input inside a tag".len()
                + "tag".len()
                + "div".len()
                + "url".len()
                + "https://user:hunter2@example.test/\nerror[fake]".len()
        );

        let mut log = DiagnosticLog::new(2, bytes * 2);
        log.push(diagnostic.clone()).unwrap();
        log.push(diagnostic.clone()).unwrap();
        assert_eq!(
            log.push(diagnostic.clone()),
            Err(DiagnosticLogError::EntryLimit)
        );
        assert_eq!(log.len(), 2);
        assert_eq!(log.used_bytes(), bytes * 2);

        let mut log = DiagnosticLog::new(5, bytes * 2 - 1);
        log.push(diagnostic.clone()).unwrap();
        assert_eq!(
            log.push(diagnostic.clone()),
            Err(DiagnosticLogError::ByteLimit)
        );
        assert_eq!(log.len(), 1);
        assert_eq!(log.used_bytes(), bytes);

        let note = Diagnostic::new(Severity::Note, TAG, "n");
        log.push(note).unwrap();
        assert_eq!(log.max_severity(), Some(Severity::Warning));

        let mut zero = DiagnosticLog::new(0, 100);
        assert_eq!(zero.push(diagnostic), Err(DiagnosticLogError::EntryLimit));
        assert!(zero.is_empty());
        assert_eq!(zero.max_severity(), None);
    }

    #[test]
    fn log_rendering_is_ordered_capped_and_redacted() {
        let (diagnostic, _) = sample();
        let mut log = DiagnosticLog::new(4, 4096);
        log.push(diagnostic.clone()).unwrap();
        log.push(Diagnostic::new(Severity::Note, TAG, "second"))
            .unwrap();
        let line = diagnostic.render(RedactionPolicy::Redact, 1024).unwrap();
        let expected = format!("{line}\nnote[tag]: second\n");
        assert_eq!(
            log.render(RedactionPolicy::Redact, expected.len()),
            Ok(expected.clone())
        );
        assert_eq!(
            log.render(RedactionPolicy::Redact, expected.len() - 1),
            Err(RenderError::OutputLimit)
        );
        assert!(!expected.contains("hunter2"));
        assert_eq!(log.into_entries().len(), 2);
    }
}
