use rustic_base::{Budget, CancellationToken};
use rustic_encoding::{Decoder, DecoderErrorMode, Encoding};

use crate::HtmlError;

/// Bounded incremental decoding with HTML input preprocessing.
///
/// Invalid sequences become U+FFFD, incomplete sequences wait for more bytes,
/// one initial BOM is removed, and CR/CRLF become LF even across feed boundaries.
#[derive(Debug)]
pub struct HtmlInput {
    bytes: Budget,
    decoder: Decoder,
    at_start: bool,
    previous_cr: bool,
    finished: bool,
    poisoned: bool,
    cancellation: CancellationToken,
}

impl HtmlInput {
    /// Create an explicitly UTF-8 stream and charge original bytes cumulatively.
    pub fn new(max_input_bytes: usize, cancellation: CancellationToken) -> Self {
        Self::with_encoding(max_input_bytes, cancellation, Encoding::Utf8)
    }

    pub(crate) fn with_encoding(
        max_input_bytes: usize,
        cancellation: CancellationToken,
        encoding: Encoding,
    ) -> Self {
        Self {
            bytes: Budget::new(max_input_bytes),
            decoder: encoding.new_decoder(DecoderErrorMode::Replacement),
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
        let mut output = String::new();
        let mut decoded = String::new();
        for bytes in chunk.chunks(4096) {
            self.cancellation
                .check()
                .map_err(|_| HtmlError::Cancelled)?;
            self.decoder
                .decode(bytes, false, &mut decoded)
                .expect("replacement decoder");
            self.preprocess(&decoded, &mut output)?;
            decoded.clear();
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
        let mut decoded = String::new();
        self.decoder
            .decode(&[], true, &mut decoded)
            .expect("replacement decoder");
        let mut output = String::new();
        let result = self.preprocess(&decoded, &mut output);
        self.finished = result.is_ok();
        self.poisoned = result.is_err();
        result.map(|()| output)
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
            if value == '\u{feff}'
                && (self.decoder.encoding() == Encoding::Utf8 || self.decoder.encoding().is_utf16())
            {
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
