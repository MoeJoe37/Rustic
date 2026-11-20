//! HTTP/1.1 response parsing (RFC 9112): the head and the body framing.
//!
//! Both parsers are incremental and take input in arbitrary pieces; the
//! result never depends on where the input was split. They keep no more than
//! the configured limits and never panic on content.

use std::fmt;

use crate::TransportLimits;
use crate::headers::{Headers, is_token, is_valid_value, trim_whitespace};

/// HTTP version of a response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HttpVersion {
    /// HTTP/1.0.
    Http10,
    /// HTTP/1.1, or a later 1.x minor version treated as 1.1 (RFC 9110 §6.2).
    Http11,
}

/// A final response's status line and header section.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResponseHead {
    version: HttpVersion,
    status: u16,
    reason: Vec<u8>,
    headers: Headers,
}

impl ResponseHead {
    /// The response version.
    pub fn version(&self) -> HttpVersion {
        self.version
    }

    /// The three-digit status code.
    pub fn status(&self) -> u16 {
        self.status
    }

    /// The reason phrase bytes, possibly empty.
    pub fn reason(&self) -> &[u8] {
        &self.reason
    }

    /// The header fields.
    pub fn headers(&self) -> &Headers {
        &self.headers
    }

    /// The same status line with other header fields (cache freshening).
    pub(crate) fn with_headers(&self, headers: Headers) -> Self {
        Self {
            headers,
            ..self.clone()
        }
    }
}

/// A response violated HTTP/1.1 syntax or framing, or a limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    /// The status line is not `HTTP/1.x NNN reason`.
    InvalidStatusLine,
    /// A header line is malformed (no colon, whitespace before the colon,
    /// a non-token name, a bare CR, NUL, or a fold before any field).
    InvalidHeader,
    /// The head exceeded `max_head_bytes` or `max_header_count`.
    HeadTooLarge,
    /// More than `max_interim_responses` 1xx responses.
    TooManyInterimResponses,
    /// `101 Switching Protocols` without a requested upgrade.
    UnexpectedUpgrade,
    /// `Content-Length` values disagree or are not decimal digits.
    InvalidContentLength,
    /// A transfer coding other than a single final `chunked`.
    UnsupportedTransferCoding,
    /// A chunk-size line is malformed or exceeds `max_chunk_line_bytes`.
    InvalidChunk,
    /// The trailer section is malformed or exceeds `max_trailer_bytes`.
    InvalidTrailer,
    /// The decoded body exceeds `max_body_bytes`.
    BodyTooLarge,
    /// A `Content-Encoding` body is corrupt or truncated.
    InvalidContentCoding,
    /// The connection closed before the framing said the message ended.
    Truncated,
    /// The connection closed before any response byte arrived.
    Empty,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidStatusLine => "invalid HTTP status line",
            Self::InvalidHeader => "invalid HTTP header line",
            Self::HeadTooLarge => "HTTP response head too large",
            Self::TooManyInterimResponses => "too many interim HTTP responses",
            Self::UnexpectedUpgrade => "unexpected protocol upgrade",
            Self::InvalidContentLength => "invalid or conflicting Content-Length",
            Self::UnsupportedTransferCoding => "unsupported Transfer-Encoding",
            Self::InvalidChunk => "invalid chunked encoding",
            Self::InvalidTrailer => "invalid chunked trailer section",
            Self::BodyTooLarge => "HTTP response body too large",
            Self::InvalidContentCoding => "invalid content-coded response body",
            Self::Truncated => "connection closed before the response ended",
            Self::Empty => "connection closed without a response",
        })
    }
}

impl std::error::Error for ProtocolError {}

/// How the body of a response is delimited (RFC 9112 §6.3).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Framing {
    /// No body: `HEAD`, 1xx, 204 and 304.
    Empty,
    /// Exactly this many bytes.
    Length(u64),
    /// `Transfer-Encoding: chunked`.
    Chunked,
    /// Until the connection closes.
    UntilClose,
}

/// Parse a complete response that arrives as `chunks` and then the end of
/// the stream, exactly as the transport would: interim responses skipped,
/// body framing, content codings, every [`TransportLimits`] bound. The
/// result must not depend on how the bytes are split; fuzzing and replay
/// tools use this, the transport runs the same steps over a socket.
pub fn parse_response(
    chunks: &[&[u8]],
    limits: &TransportLimits,
    head_request: bool,
) -> Result<(ResponseHead, Vec<u8>), ProtocolError> {
    let mut head_parser = HeadParser::new(limits);
    let mut state: Option<(
        ResponseHead,
        BodyDecoder,
        Option<crate::coding::ContentDecoder>,
    )> = None;
    let mut body = Vec::new();
    let mut received_any = false;
    for &input in chunks {
        if input.is_empty() {
            continue;
        }
        received_any = true;
        let mut wire = Vec::new();
        match &mut state {
            Some((_, decoder, _)) => decoder.feed(input, &mut wire)?,
            None => {
                if let Some((head, rest)) = head_parser.feed(input)? {
                    let body_framing = framing(&head, head_request)?;
                    let content = if body_framing == Framing::Empty {
                        None
                    } else {
                        crate::coding::ContentDecoder::for_response(
                            head.headers(),
                            limits.max_body_bytes,
                        )
                    };
                    let mut decoder = BodyDecoder::new(body_framing, limits);
                    decoder.feed(&rest, &mut wire)?;
                    state = Some((head, decoder, content));
                }
            }
        }
        if let Some((_, decoder, content)) = &mut state {
            match content {
                Some(content) => content.feed(&wire, &mut body)?,
                None => body.extend_from_slice(&wire),
            }
            if decoder.is_done() {
                break;
            }
        }
    }
    match state {
        Some((head, mut decoder, mut content)) => {
            decoder.finish()?;
            if let Some(content) = &mut content {
                content.finish(&mut body)?;
            }
            Ok((head, body))
        }
        None if received_any => Err(ProtocolError::Truncated),
        None => Err(ProtocolError::Empty),
    }
}

const STATUS_PREFIX: &[u8] = b"HTTP/1.";

/// Incremental parser for the response head, skipping interim responses.
pub(crate) struct HeadParser {
    buffer: Vec<u8>,
    /// Bytes of `buffer` already scanned for the end of the head.
    scanned: usize,
    /// Head bytes consumed by earlier interim responses and blank lines.
    consumed_bytes: usize,
    interim_count: usize,
    max_head_bytes: usize,
    max_header_count: usize,
    max_interim: usize,
}

impl HeadParser {
    pub(crate) fn new(limits: &TransportLimits) -> Self {
        Self {
            buffer: Vec::new(),
            scanned: 0,
            consumed_bytes: 0,
            interim_count: 0,
            max_head_bytes: limits.max_head_bytes,
            max_header_count: limits.max_header_count,
            max_interim: limits.max_interim_responses,
        }
    }

    /// Feed bytes. On the final head, returns it with the bytes that follow
    /// it (the start of the body).
    pub(crate) fn feed(
        &mut self,
        input: &[u8],
    ) -> Result<Option<(ResponseHead, Vec<u8>)>, ProtocolError> {
        self.buffer.extend_from_slice(input);
        loop {
            self.drop_leading_blank_lines();
            // Fail as soon as the input cannot be a status line, so an
            // HTTP/0.9-style body is refused without waiting for EOF.
            let prefix = self.buffer.len().min(STATUS_PREFIX.len());
            // Bytes that are all CR or LF may still become blank lines
            // (RFC 9112 §2.2), however they are split.
            if self.buffer[..prefix] != STATUS_PREFIX[..prefix]
                && !self.buffer.iter().all(|byte| matches!(byte, b'\r' | b'\n'))
            {
                return Err(ProtocolError::InvalidStatusLine);
            }
            let Some(end) = self.find_end() else {
                if self.consumed_bytes + self.buffer.len() > self.max_head_bytes {
                    return Err(ProtocolError::HeadTooLarge);
                }
                return Ok(None);
            };
            if self.consumed_bytes + end > self.max_head_bytes {
                return Err(ProtocolError::HeadTooLarge);
            }
            let head = parse_head(&self.buffer[..end], self.max_header_count)?;
            let rest = self.buffer.split_off(end);
            self.buffer = rest;
            self.scanned = 0;
            if (100..200).contains(&head.status) {
                if head.status == 101 {
                    return Err(ProtocolError::UnexpectedUpgrade);
                }
                self.interim_count += 1;
                if self.interim_count > self.max_interim {
                    return Err(ProtocolError::TooManyInterimResponses);
                }
                self.consumed_bytes += end;
                continue;
            }
            return Ok(Some((head, std::mem::take(&mut self.buffer))));
        }
    }

    /// The index just past the blank line ending the head. Bare LF line
    /// endings are accepted (RFC 9112 §2.2). The buffer never starts with a
    /// blank line, so the first blank line found ends the head.
    fn find_end(&mut self) -> Option<usize> {
        let buffer = &self.buffer;
        let mut index = self.scanned;
        while let Some(offset) = buffer[index..].iter().position(|&byte| byte == b'\n') {
            let newline = index + offset;
            match &buffer[newline + 1..] {
                [b'\n', ..] => return Some(newline + 2),
                [b'\r', b'\n', ..] => return Some(newline + 3),
                [] | [b'\r'] => {
                    self.scanned = newline;
                    return None;
                }
                _ => index = newline + 1,
            }
        }
        self.scanned = buffer.len();
        None
    }

    /// RFC 9112 §2.2: drop empty lines received before a status line. A
    /// trailing CR is kept until the next byte shows whether it ends a line.
    fn drop_leading_blank_lines(&mut self) {
        let blank = self
            .buffer
            .iter()
            .take_while(|byte| matches!(byte, b'\r' | b'\n'))
            .count();
        let drop = self.buffer[..blank]
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(0, |last| last + 1);
        if drop > 0 {
            self.buffer.drain(..drop);
            self.consumed_bytes += drop;
            self.scanned = self.scanned.saturating_sub(drop);
        }
    }
}

fn split_lines(head: &[u8]) -> impl Iterator<Item = &[u8]> {
    head.split(|&byte| byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
}

fn parse_head(head: &[u8], max_header_count: usize) -> Result<ResponseHead, ProtocolError> {
    let mut lines = split_lines(head);
    let status_line = lines.next().ok_or(ProtocolError::InvalidStatusLine)?;
    let (version, status, reason) = parse_status_line(status_line)?;
    let mut headers = Headers::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if line.contains(&b'\r') || line.contains(&0) {
            return Err(ProtocolError::InvalidHeader);
        }
        if let [b' ' | b'\t', ..] = line {
            // RFC 9112 §5.2: replace obsolete line folding with a space.
            let value = headers
                .last_value_mut()
                .ok_or(ProtocolError::InvalidHeader)?;
            let continuation = trim_whitespace(line);
            if !continuation.is_empty() {
                if !value.is_empty() {
                    value.push(b' ');
                }
                value.extend_from_slice(continuation);
            }
            continue;
        }
        let colon = line
            .iter()
            .position(|&byte| byte == b':')
            .ok_or(ProtocolError::InvalidHeader)?;
        let (name, value) = (&line[..colon], &line[colon + 1..]);
        if !is_token(name) || !is_valid_value(value) {
            return Err(ProtocolError::InvalidHeader);
        }
        if headers.len() == max_header_count {
            return Err(ProtocolError::HeadTooLarge);
        }
        let name = std::str::from_utf8(name).map_err(|_| ProtocolError::InvalidHeader)?;
        headers
            .append(name, value)
            .map_err(|_| ProtocolError::InvalidHeader)?;
    }
    Ok(ResponseHead {
        version,
        status,
        reason,
        headers,
    })
}

fn parse_status_line(line: &[u8]) -> Result<(HttpVersion, u16, Vec<u8>), ProtocolError> {
    let rest = line
        .strip_prefix(STATUS_PREFIX)
        .ok_or(ProtocolError::InvalidStatusLine)?;
    let version = match rest.first() {
        Some(b'0') => HttpVersion::Http10,
        Some(byte) if byte.is_ascii_digit() => HttpVersion::Http11,
        _ => return Err(ProtocolError::InvalidStatusLine),
    };
    let rest = rest[1..]
        .strip_prefix(b" ")
        .ok_or(ProtocolError::InvalidStatusLine)?;
    let (code, reason) = match rest {
        [a, b, c] => ([*a, *b, *c], &[][..]),
        [a, b, c, b' ', reason @ ..] => ([*a, *b, *c], reason),
        _ => return Err(ProtocolError::InvalidStatusLine),
    };
    if !code.iter().all(u8::is_ascii_digit) || code[0] == b'0' {
        return Err(ProtocolError::InvalidStatusLine);
    }
    let status = code
        .iter()
        .fold(0u16, |status, digit| status * 10 + u16::from(digit - b'0'));
    if reason.iter().any(|&byte| byte == 0 || byte == b'\r') {
        return Err(ProtocolError::InvalidStatusLine);
    }
    Ok((version, status, reason.to_vec()))
}

/// Choose the body framing for a final response (RFC 9112 §6.3).
///
/// Like Chromium, a `Transfer-Encoding: chunked` overrides `Content-Length`,
/// and repeated identical `Content-Length` values are one length (Fetch
/// "extract a length"). In HTTP/1.0 a `Transfer-Encoding` is faulty framing
/// and the body runs until close (RFC 9112 §6.1).
pub(crate) fn framing(
    head: &ResponseHead,
    request_is_head: bool,
) -> Result<Framing, ProtocolError> {
    if request_is_head || matches!(head.status, 100..=199 | 204 | 304) {
        return Ok(Framing::Empty);
    }
    if let Some(coding) = head.headers.combined("transfer-encoding") {
        if head.version == HttpVersion::Http10 {
            return Ok(Framing::UntilClose);
        }
        let codings: Vec<&[u8]> = coding
            .split(|&byte| byte == b',')
            .map(trim_whitespace)
            .filter(|item| !item.is_empty())
            .collect();
        return match codings.as_slice() {
            [only] if only.eq_ignore_ascii_case(b"chunked") => Ok(Framing::Chunked),
            _ => Err(ProtocolError::UnsupportedTransferCoding),
        };
    }
    let mut length = None;
    for value in head.headers.get_all("content-length") {
        for item in value.split(|&byte| byte == b',').map(trim_whitespace) {
            let parsed = parse_decimal(item).ok_or(ProtocolError::InvalidContentLength)?;
            if length.is_some_and(|known| known != parsed) {
                return Err(ProtocolError::InvalidContentLength);
            }
            length = Some(parsed);
        }
    }
    Ok(match length {
        Some(0) => Framing::Empty,
        Some(length) => Framing::Length(length),
        None => Framing::UntilClose,
    })
}

fn parse_decimal(digits: &[u8]) -> Option<u64> {
    if digits.is_empty() {
        return None;
    }
    digits.iter().try_fold(0u64, |value, &digit| {
        digit
            .is_ascii_digit()
            .then_some(())
            .and_then(|()| value.checked_mul(10))
            .and_then(|value| value.checked_add(u64::from(digit - b'0')))
    })
}

#[derive(Debug)]
enum Chunk {
    /// Reading a chunk-size line; holds the line so far.
    Size(Vec<u8>),
    /// Inside chunk data with this many bytes left.
    Data(u64),
    /// After chunk data: expecting CRLF (or LF). `true` once CR was seen.
    DataEnd(bool),
    /// In the trailer section; holds the current line and total bytes.
    Trailer(Vec<u8>, usize),
}

#[derive(Debug)]
enum BodyState {
    Length(u64),
    Chunked(Chunk),
    UntilClose,
    Done,
}

/// Incremental body decoder for one response.
#[derive(Debug)]
pub(crate) struct BodyDecoder {
    state: BodyState,
    delivered: u64,
    max_body: u64,
    max_chunk_line: usize,
    max_trailer: usize,
}

impl BodyDecoder {
    pub(crate) fn new(framing: Framing, limits: &TransportLimits) -> Self {
        let state = match framing {
            Framing::Empty => BodyState::Done,
            Framing::Length(length) => BodyState::Length(length),
            Framing::Chunked => BodyState::Chunked(Chunk::Size(Vec::new())),
            Framing::UntilClose => BodyState::UntilClose,
        };
        Self {
            state,
            delivered: 0,
            max_body: limits.max_body_bytes,
            max_chunk_line: limits.max_chunk_line_bytes,
            max_trailer: limits.max_trailer_bytes,
        }
    }

    /// Whether the message is complete.
    pub(crate) fn is_done(&self) -> bool {
        matches!(self.state, BodyState::Done)
    }

    /// Decode `input`, appending body bytes to `output`. Bytes after the end
    /// of the message are ignored (the connection is not reused).
    pub(crate) fn feed(
        &mut self,
        mut input: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<(), ProtocolError> {
        while !input.is_empty() {
            match &mut self.state {
                BodyState::Done => return Ok(()),
                BodyState::UntilClose => {
                    self.deliver(input, output)?;
                    input = &[];
                }
                BodyState::Length(left) => {
                    let take = clamp(*left, input.len());
                    let left_after = *left - take as u64;
                    self.state = if left_after == 0 {
                        BodyState::Done
                    } else {
                        BodyState::Length(left_after)
                    };
                    let (data, rest) = input.split_at(take);
                    self.deliver(data, output)?;
                    input = rest;
                }
                BodyState::Chunked(chunk) => {
                    let step = step_chunk(chunk, input, self.max_chunk_line, self.max_trailer)?;
                    if step.finished {
                        self.state = BodyState::Done;
                    }
                    self.deliver(&input[..step.data], output)?;
                    input = &input[step.consumed..];
                }
            }
        }
        Ok(())
    }

    /// The connection closed. Only close-delimited bodies may end here.
    pub(crate) fn finish(&mut self) -> Result<(), ProtocolError> {
        match self.state {
            BodyState::Done => Ok(()),
            BodyState::UntilClose => {
                self.state = BodyState::Done;
                Ok(())
            }
            _ => Err(ProtocolError::Truncated),
        }
    }

    fn deliver(&mut self, data: &[u8], output: &mut Vec<u8>) -> Result<(), ProtocolError> {
        let delivered = self
            .delivered
            .checked_add(data.len() as u64)
            .filter(|&total| total <= self.max_body)
            .ok_or(ProtocolError::BodyTooLarge)?;
        self.delivered = delivered;
        output.extend_from_slice(data);
        Ok(())
    }
}

fn clamp(left: u64, available: usize) -> usize {
    usize::try_from(left).map_or(available, |left| left.min(available))
}

/// One step of the chunked decoder over a prefix of its input.
struct ChunkStep {
    /// Input bytes consumed.
    consumed: usize,
    /// The first `data` input bytes are chunk data (`data <= consumed`).
    data: usize,
    /// The last-chunk and trailer section are complete.
    finished: bool,
}

impl ChunkStep {
    fn framing(consumed: usize) -> Self {
        Self {
            consumed,
            data: 0,
            finished: false,
        }
    }
}

/// Advance the chunked decoder over a prefix of a non-empty `input`.
fn step_chunk(
    chunk: &mut Chunk,
    input: &[u8],
    max_line: usize,
    max_trailer: usize,
) -> Result<ChunkStep, ProtocolError> {
    match chunk {
        Chunk::Size(line) => {
            let Some(newline) = input.iter().position(|&byte| byte == b'\n') else {
                if line.len() + input.len() > max_line {
                    return Err(ProtocolError::InvalidChunk);
                }
                line.extend_from_slice(input);
                return Ok(ChunkStep::framing(input.len()));
            };
            if line.len() + newline > max_line {
                return Err(ProtocolError::InvalidChunk);
            }
            line.extend_from_slice(&input[..newline]);
            let size = parse_chunk_size(line)?;
            *chunk = if size == 0 {
                Chunk::Trailer(Vec::new(), 0)
            } else {
                Chunk::Data(size)
            };
            Ok(ChunkStep::framing(newline + 1))
        }
        Chunk::Data(left) => {
            let take = clamp(*left, input.len());
            let left_after = *left - take as u64;
            *chunk = if left_after == 0 {
                Chunk::DataEnd(false)
            } else {
                Chunk::Data(left_after)
            };
            Ok(ChunkStep {
                consumed: take,
                data: take,
                finished: false,
            })
        }
        Chunk::DataEnd(seen_cr) => match (input[0], *seen_cr) {
            (b'\r', false) => {
                *chunk = Chunk::DataEnd(true);
                Ok(ChunkStep::framing(1))
            }
            (b'\n', _) => {
                *chunk = Chunk::Size(Vec::new());
                Ok(ChunkStep::framing(1))
            }
            _ => Err(ProtocolError::InvalidChunk),
        },
        Chunk::Trailer(line, total) => {
            let newline = input.iter().position(|&byte| byte == b'\n');
            let piece = &input[..newline.unwrap_or(input.len())];
            *total = total
                .checked_add(piece.len() + 1)
                .filter(|&total| total <= max_trailer)
                .ok_or(ProtocolError::InvalidTrailer)?;
            line.extend_from_slice(piece);
            let Some(newline) = newline else {
                return Ok(ChunkStep::framing(input.len()));
            };
            let content = line.strip_suffix(b"\r").unwrap_or(line);
            let finished = content.is_empty();
            if !finished {
                validate_trailer_line(content)?;
                line.clear();
            }
            Ok(ChunkStep {
                consumed: newline + 1,
                data: 0,
                finished,
            })
        }
    }
}

/// `chunk-size [ chunk-ext ]`: hex digits, then optional whitespace and
/// `;`-introduced extensions, which are ignored (RFC 9112 §7.1.1).
fn parse_chunk_size(line: &[u8]) -> Result<u64, ProtocolError> {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let digits = line
        .iter()
        .position(|byte| !byte.is_ascii_hexdigit())
        .unwrap_or(line.len());
    if digits == 0 {
        return Err(ProtocolError::InvalidChunk);
    }
    let rest = trim_whitespace(&line[digits..]);
    if !rest.is_empty() && (rest[0] != b';' || !is_valid_value(rest)) {
        return Err(ProtocolError::InvalidChunk);
    }
    line[..digits].iter().try_fold(0u64, |size, &digit| {
        let value = (digit as char)
            .to_digit(16)
            .ok_or(ProtocolError::InvalidChunk)?;
        size.checked_mul(16)
            .and_then(|size| size.checked_add(u64::from(value)))
            .ok_or(ProtocolError::InvalidChunk)
    })
}

/// Trailer fields are validated and then dropped: Fetch exposes none.
fn validate_trailer_line(line: &[u8]) -> Result<(), ProtocolError> {
    let colon = line
        .iter()
        .position(|&byte| byte == b':')
        .ok_or(ProtocolError::InvalidTrailer)?;
    if !is_token(&line[..colon]) || !is_valid_value(&line[colon + 1..]) || line.contains(&b'\r') {
        return Err(ProtocolError::InvalidTrailer);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> TransportLimits {
        TransportLimits::default()
    }

    fn head(input: &[u8]) -> Result<Option<(ResponseHead, Vec<u8>)>, ProtocolError> {
        HeadParser::new(&limits()).feed(input)
    }

    /// Parse a whole response (head and body to EOF) from pieces.
    fn parse_pieces(
        pieces: &[&[u8]],
        request_is_head: bool,
    ) -> Result<(ResponseHead, Vec<u8>), ProtocolError> {
        let limits = limits();
        let mut parser = HeadParser::new(&limits);
        let mut decoder: Option<(ResponseHead, BodyDecoder)> = None;
        let mut body = Vec::new();
        for piece in pieces {
            match &mut decoder {
                None => {
                    if let Some((head, rest)) = parser.feed(piece)? {
                        let mut body_decoder =
                            BodyDecoder::new(framing(&head, request_is_head)?, &limits);
                        body_decoder.feed(&rest, &mut body)?;
                        decoder = Some((head, body_decoder));
                    }
                }
                Some((_, body_decoder)) => body_decoder.feed(piece, &mut body)?,
            }
        }
        let (head, mut body_decoder) = decoder.ok_or(ProtocolError::Truncated)?;
        body_decoder.finish()?;
        Ok((head, body))
    }

    /// Every way of splitting `input` in two, plus byte-at-a-time, gives
    /// the same result as one piece.
    fn parse_all_splits(
        input: &[u8],
        request_is_head: bool,
    ) -> Result<(ResponseHead, Vec<u8>), ProtocolError> {
        let whole = parse_pieces(&[input], request_is_head);
        for split in 0..=input.len() {
            let (a, b) = input.split_at(split);
            assert_eq!(
                parse_pieces(&[a, b], request_is_head),
                whole,
                "split at {split}"
            );
        }
        let bytes: Vec<&[u8]> = input.chunks(1).collect();
        assert_eq!(
            parse_pieces(&bytes, request_is_head),
            whole,
            "byte at a time"
        );
        whole
    }

    #[test]
    fn status_lines_versions_and_reason_phrases() {
        let mut parser = HeadParser::new(&limits());
        assert_eq!(parser.feed(b"\r").unwrap(), None);
        assert_eq!(parser.feed(b"\nHTT").unwrap(), None);
        assert_eq!(
            HeadParser::new(&limits()).feed(b"<").unwrap_err(),
            ProtocolError::InvalidStatusLine
        );
        let (parsed, rest) = head(b"HTTP/1.1 200 OK\r\nA: 1\r\n\r\nbody")
            .unwrap()
            .unwrap();
        assert_eq!(
            (parsed.version(), parsed.status(), parsed.reason()),
            (HttpVersion::Http11, 200, &b"OK"[..])
        );
        assert_eq!(rest, b"body");
        let (parsed, _) = head(b"HTTP/1.0 404\n\n").unwrap().unwrap();
        assert_eq!(
            (parsed.version(), parsed.status(), parsed.reason()),
            (HttpVersion::Http10, 404, &b""[..])
        );
        let (parsed, _) = head(b"\r\nHTTP/1.2 599 Odd Reason Here\r\n\r\n")
            .unwrap()
            .unwrap();
        assert_eq!(
            (parsed.version(), parsed.status()),
            (HttpVersion::Http11, 599)
        );
        assert_eq!(parsed.reason(), b"Odd Reason Here");
        for bad in [
            &b"HTTP/2 200 OK\r\n\r\n"[..],
            b"HTTP/1.1 20 OK\r\n\r\n",
            b"HTTP/1.1 2000 OK\r\n\r\n",
            b"HTTP/1.1 099 OK\r\n\r\n",
            b"HTTP/1.1  200 OK\r\n\r\n",
            b"http/1.1 200 OK\r\n\r\n",
            b"<html>\r\n\r\n",
        ] {
            assert_eq!(
                head(bad).unwrap_err(),
                ProtocolError::InvalidStatusLine,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn header_lines_folding_and_malformed_lines() {
        let (parsed, _) =
            head(b"HTTP/1.1 200 OK\r\nX-A:  one \r\n  two\r\n\tthree\r\nX-B:\r\n\r\n")
                .unwrap()
                .unwrap();
        assert_eq!(parsed.headers().get("x-a"), Some(&b"one two three"[..]));
        assert_eq!(parsed.headers().get("x-b"), Some(&b""[..]));
        for bad in [
            &b"HTTP/1.1 200 OK\r\n folded-first\r\n\r\n"[..],
            b"HTTP/1.1 200 OK\r\nNoColon\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nName : v\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nA: b\rc\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nA: b\0c\r\n\r\n",
            b"HTTP/1.1 200 OK\r\n: v\r\n\r\n",
        ] {
            assert_eq!(
                head(bad).unwrap_err(),
                ProtocolError::InvalidHeader,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn head_limits_count_interim_responses_and_header_lines() {
        let mut small = limits();
        small.max_head_bytes = 40;
        let mut parser = HeadParser::new(&small);
        assert_eq!(parser.feed(b"HTTP/1.1 200 OK\r\n").unwrap(), None);
        assert_eq!(
            parser.feed(b"X-Long: aaaaaaaaaaaaaaaaaaaaaaa").unwrap_err(),
            ProtocolError::HeadTooLarge
        );
        let mut parser = HeadParser::new(&small);
        assert_eq!(
            parser
                .feed(b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\n\r\n")
                .unwrap_err(),
            ProtocolError::HeadTooLarge
        );
        let mut few = limits();
        few.max_header_count = 2;
        assert_eq!(
            HeadParser::new(&few)
                .feed(b"HTTP/1.1 200 OK\r\nA: 1\r\nB: 2\r\nC: 3\r\n\r\n")
                .unwrap_err(),
            ProtocolError::HeadTooLarge
        );
        few.max_interim_responses = 1;
        let mut parser = HeadParser::new(&few);
        assert_eq!(
            parser
                .feed(b"HTTP/1.1 100 A\r\n\r\nHTTP/1.1 103 B\r\n\r\n")
                .unwrap_err(),
            ProtocolError::TooManyInterimResponses
        );
        assert_eq!(
            head(b"HTTP/1.1 101 Switching\r\n\r\n").unwrap_err(),
            ProtocolError::UnexpectedUpgrade
        );
    }

    #[test]
    fn interim_responses_are_skipped_across_splits() {
        let (parsed, body) =
            parse_all_splits(b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 103 Hints\r\nLink: x\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi", false)
                .unwrap();
        assert_eq!(parsed.status(), 200);
        assert!(!parsed.headers().contains("link"));
        assert_eq!(body, b"hi");
    }

    #[test]
    fn content_length_bodies() {
        let (_, body) = parse_all_splits(
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhelloEXTRA",
            false,
        )
        .unwrap();
        assert_eq!(body, b"hello");
        let (_, body) = parse_all_splits(
            b"HTTP/1.1 200 OK\r\nContent-Length: 3, 3\r\ncontent-length: 3\r\n\r\nabc",
            false,
        )
        .unwrap();
        assert_eq!(body, b"abc");
        assert_eq!(
            parse_all_splits(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhel", false)
                .unwrap_err(),
            ProtocolError::Truncated
        );
        for bad in ["3, 4", "-1", "+3", "0x3", "", "99999999999999999999999"] {
            let input = format!("HTTP/1.1 200 OK\r\nContent-Length: {bad}\r\n\r\nabc");
            assert_eq!(
                parse_pieces(&[input.as_bytes()], false).unwrap_err(),
                ProtocolError::InvalidContentLength,
                "{bad}"
            );
        }
    }

    #[test]
    fn bodiless_responses_ignore_their_framing() {
        for status in ["204 No Content", "304 Not Modified"] {
            let input = format!("HTTP/1.1 {status}\r\nContent-Length: 10\r\n\r\n");
            assert_eq!(parse_all_splits(input.as_bytes(), false).unwrap().1, b"");
        }
        let (_, body) =
            parse_all_splits(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n", true).unwrap();
        assert_eq!(body, b"");
        let (_, body) = parse_all_splits(
            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\nignored",
            false,
        )
        .unwrap();
        assert_eq!(body, b"");
    }

    #[test]
    fn close_delimited_bodies() {
        let (_, body) = parse_all_splits(b"HTTP/1.1 200 OK\r\n\r\nuntil close", false).unwrap();
        assert_eq!(body, b"until close");
        let (_, body) = parse_all_splits(
            b"HTTP/1.0 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 1\r\n\r\n5\r\nhello",
            false,
        )
        .unwrap();
        assert_eq!(body, b"5\r\nhello");
    }

    #[test]
    fn chunked_bodies_with_extensions_and_trailers() {
        let input = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: Chunked\r\nContent-Length: 999\r\n\r\n\
            5;name=value\r\nhello\r\n1 ; ext\r\n \r\nA\nabcdefghij\n0\r\nExpires: never\r\nX: y\r\n\r\nGARBAGE";
        let (_, body) = parse_all_splits(input, false).unwrap();
        assert_eq!(body, b"hello abcdefghij");
        assert_eq!(
            parse_all_splits(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
                false
            )
            .unwrap()
            .1,
            b""
        );
    }

    #[test]
    fn chunked_errors() {
        let base = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
        for (chunks, error) in [
            ("5\r\nhello", ProtocolError::Truncated),
            ("5\r\nhelloX\r\n0\r\n\r\n", ProtocolError::InvalidChunk),
            ("\r\n", ProtocolError::InvalidChunk),
            ("g\r\n", ProtocolError::InvalidChunk),
            ("5 x\r\nhello\r\n", ProtocolError::InvalidChunk),
            ("-5\r\n", ProtocolError::InvalidChunk),
            ("10000000000000000\r\n", ProtocolError::InvalidChunk),
            ("0\r\nbad trailer\r\n\r\n", ProtocolError::InvalidTrailer),
            ("0\r\nX: y", ProtocolError::Truncated),
        ] {
            let input = format!("{base}{chunks}");
            assert_eq!(
                parse_all_splits(input.as_bytes(), false).unwrap_err(),
                error,
                "{chunks:?}"
            );
        }
        for coding in ["gzip, chunked", "chunked, chunked", "identity", "gzip"] {
            let input = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: {coding}\r\n\r\n");
            assert_eq!(
                parse_pieces(&[input.as_bytes()], false).unwrap_err(),
                ProtocolError::UnsupportedTransferCoding,
                "{coding}"
            );
        }
    }

    #[test]
    fn chunk_line_trailer_and_body_limits() {
        let mut tight = limits();
        tight.max_chunk_line_bytes = 8;
        tight.max_trailer_bytes = 8;
        tight.max_body_bytes = 4;
        let run = |input: &[u8]| {
            let mut decoder = BodyDecoder::new(Framing::Chunked, &tight);
            let mut out = Vec::new();
            decoder.feed(input, &mut out).map(|()| out)
        };
        assert_eq!(run(b"1;aaaaaaaaaaa"), Err(ProtocolError::InvalidChunk));
        assert_eq!(
            run(b"0\r\nX: aaaaaaaaaaa\r\n"),
            Err(ProtocolError::InvalidTrailer)
        );
        assert_eq!(run(b"5\r\nhello\r\n"), Err(ProtocolError::BodyTooLarge));
        assert_eq!(run(b"4\r\nhell\r\n0\r\n\r\n").unwrap(), b"hell");
        let mut decoder = BodyDecoder::new(Framing::UntilClose, &tight);
        let mut out = Vec::new();
        decoder.feed(b"abcd", &mut out).unwrap();
        assert_eq!(
            decoder.feed(b"e", &mut out),
            Err(ProtocolError::BodyTooLarge)
        );
        assert_eq!(out, b"abcd");
    }

    #[test]
    fn arbitrary_input_never_panics() {
        // A small deterministic generator over HTTP-ish bytes.
        let alphabet = b"HTTP/1.1 200 OK\r\n\t:;0123456789abcdefABCDEF,Transfer-Encoding chunked Content-Length\0\x7f\xff";
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..3000 {
            let length = (next() % 120) as usize;
            let mut input = b"HTTP/1.1 200 OK\r\n".to_vec();
            input.extend((0..length).map(|_| alphabet[(next() % alphabet.len() as u64) as usize]));
            let split = (next() % (input.len() as u64 + 1)) as usize;
            let (a, b) = input.split_at(split);
            let _ = parse_pieces(&[a, b], false);
        }
    }
}
