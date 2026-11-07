use rustic_base::{Budget, CancellationToken};

use crate::HtmlError;

/// Bounded incremental UTF-8 decoding with HTML input preprocessing.
///
/// Invalid sequences become U+FFFD, incomplete sequences wait for more bytes,
/// one initial BOM is removed, and CR/CRLF become LF even across feed boundaries.
/// This does not select encodings, decode legacy charsets, or restart a parser.
#[derive(Debug)]
pub struct Utf8Input {
    bytes: Budget,
    pending: Vec<u8>,
    at_start: bool,
    previous_cr: bool,
    finished: bool,
    poisoned: bool,
    cancellation: CancellationToken,
}

impl Utf8Input {
    /// Create an explicitly UTF-8 stream and charge original bytes cumulatively.
    pub fn new(max_input_bytes: usize, cancellation: CancellationToken) -> Self {
        Self {
            bytes: Budget::new(max_input_bytes),
            pending: Vec::new(),
            at_start: true,
            previous_cr: false,
            finished: false,
            poisoned: false,
            cancellation,
        }
    }

    /// Original bytes accepted by successful budget charges, including BOM bytes.
    pub fn bytes_consumed(&self) -> usize {
        self.bytes.used()
    }

    /// Decode another chunk without treating its boundary as EOF.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<String, HtmlError> {
        self.ensure_active()?;
        let result = self.decode(chunk);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn decode(&mut self, chunk: &[u8]) -> Result<String, HtmlError> {
        self.cancellation
            .check()
            .map_err(|_| HtmlError::Cancelled)?;
        self.bytes
            .charge(chunk.len())
            .map_err(|_| HtmlError::InputLimit)?;
        let mut bytes = std::mem::take(&mut self.pending);
        bytes.extend_from_slice(chunk);
        let mut rest = bytes.as_slice();
        let mut output = String::new();
        while !rest.is_empty() {
            self.cancellation
                .check()
                .map_err(|_| HtmlError::Cancelled)?;
            match std::str::from_utf8(rest) {
                Ok(valid) => {
                    self.preprocess(valid, &mut output)?;
                    break;
                }
                Err(error) => {
                    let valid = std::str::from_utf8(&rest[..error.valid_up_to()])
                        .expect("validated UTF-8 prefix");
                    self.preprocess(valid, &mut output)?;
                    rest = &rest[error.valid_up_to()..];
                    if let Some(length) = error.error_len() {
                        self.scalar('\u{fffd}', &mut output);
                        rest = &rest[length..];
                    } else {
                        // A UTF-8 scalar has at most four bytes, so at most three
                        // incomplete bytes persist between chunks.
                        self.pending.extend_from_slice(rest);
                        break;
                    }
                }
            }
        }
        Ok(output)
    }

    /// Emit a replacement for a truncated final sequence and close the stream.
    pub fn finish(&mut self) -> Result<String, HtmlError> {
        self.ensure_active()?;
        if self.cancellation.check().is_err() {
            self.poisoned = true;
            return Err(HtmlError::Cancelled);
        }
        let mut output = String::new();
        if !self.pending.is_empty() {
            self.pending.clear();
            self.scalar('\u{fffd}', &mut output);
        }
        self.finished = true;
        Ok(output)
    }

    fn ensure_active(&self) -> Result<(), HtmlError> {
        if self.poisoned {
            Err(HtmlError::Poisoned)
        } else if self.finished {
            Err(HtmlError::AlreadyFinished)
        } else {
            Ok(())
        }
    }

    fn preprocess(&mut self, input: &str, output: &mut String) -> Result<(), HtmlError> {
        for value in input.chars() {
            self.cancellation
                .check()
                .map_err(|_| HtmlError::Cancelled)?;
            self.scalar(value, output);
        }
        Ok(())
    }

    fn scalar(&mut self, value: char, output: &mut String) {
        if self.at_start {
            self.at_start = false;
            if value == '\u{feff}' {
                return;
            }
        }
        if self.previous_cr && value == '\n' {
            self.previous_cr = false;
            return;
        }
        self.previous_cr = value == '\r';
        output.push(if self.previous_cr { '\n' } else { value });
    }
}
