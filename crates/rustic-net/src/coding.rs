//! Content codings (RFC 9110 §8.4.1): `gzip`, `deflate` and `br`, decoded
//! under the transport's body limit.
//!
//! `Content-Encoding` lists codings in the order they were applied, so
//! they are undone from last to first. Brotli decodes incrementally;
//! `gzip` and `deflate` buffer their input (bounded by `max_body_bytes`)
//! and decode at the end of the body with `rustic-codecs` (TD-162). A
//! response with an unknown coding is delivered undecoded, as browsers do;
//! Rustic only advertises the codings it decodes.

use rustic_base::CancellationToken;

use crate::Headers;
use crate::mime::split_header_value;
use crate::response::ProtocolError;

/// The `Accept-Encoding` value Rustic sends.
pub(crate) const ACCEPT_ENCODING: &[u8] = b"gzip, deflate, br";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Coding {
    Gzip,
    Deflate,
    Brotli,
}

/// The codings to undo, last applied first; `None` when there is nothing
/// to decode or a coding is unknown (the body is then passed through).
fn codings(headers: &Headers) -> Option<Vec<Coding>> {
    let combined = headers.combined("content-encoding")?;
    let mut list = Vec::new();
    for name in split_header_value(&combined) {
        let name = name.to_ascii_lowercase();
        match name.as_str() {
            "identity" | "" => {}
            "gzip" | "x-gzip" => list.push(Coding::Gzip),
            "deflate" => list.push(Coding::Deflate),
            "br" => list.push(Coding::Brotli),
            _ => return None,
        }
    }
    if list.is_empty() {
        return None;
    }
    list.reverse();
    Some(list)
}

/// Undoes a response's content codings as body bytes arrive.
pub(crate) struct ContentDecoder {
    layers: Vec<Layer>,
    limit: usize,
    produced: usize,
}

enum Layer {
    Buffered { coding: Coding, input: Vec<u8> },
    Brotli(Box<Brotli>),
}

impl ContentDecoder {
    /// A decoder for `headers`, or `None` when the body is delivered as is.
    pub(crate) fn for_response(headers: &Headers, max_body_bytes: u64) -> Option<Self> {
        let layers = codings(headers)?
            .into_iter()
            .map(|coding| match coding {
                Coding::Brotli => Layer::Brotli(Box::default()),
                coding => Layer::Buffered {
                    coding,
                    input: Vec::new(),
                },
            })
            .collect();
        Some(Self {
            layers,
            limit: usize::try_from(max_body_bytes).unwrap_or(usize::MAX),
            produced: 0,
        })
    }

    /// Decode `input`, appending what is ready to `output`.
    pub(crate) fn feed(&mut self, input: &[u8], output: &mut Vec<u8>) -> Result<(), ProtocolError> {
        self.run(input, false, output)
    }

    /// The body ended: flush every layer.
    pub(crate) fn finish(&mut self, output: &mut Vec<u8>) -> Result<(), ProtocolError> {
        self.run(&[], true, output)
    }

    fn run(&mut self, input: &[u8], last: bool, output: &mut Vec<u8>) -> Result<(), ProtocolError> {
        let mut data = input.to_vec();
        let limit = self.limit;
        for layer in &mut self.layers {
            let mut next = Vec::new();
            layer.step(&data, last, limit, &mut next)?;
            data = next;
        }
        self.produced = self
            .produced
            .checked_add(data.len())
            .filter(|&total| total <= limit)
            .ok_or(ProtocolError::BodyTooLarge)?;
        output.extend_from_slice(&data);
        Ok(())
    }
}

impl Layer {
    fn step(
        &mut self,
        input: &[u8],
        last: bool,
        limit: usize,
        output: &mut Vec<u8>,
    ) -> Result<(), ProtocolError> {
        match self {
            Self::Buffered {
                coding,
                input: buffer,
            } => {
                if buffer.len() + input.len() > limit {
                    return Err(ProtocolError::BodyTooLarge);
                }
                buffer.extend_from_slice(input);
                if last {
                    let decoded = decode_buffered(*coding, buffer, limit)?;
                    buffer.clear();
                    output.extend_from_slice(&decoded);
                }
                Ok(())
            }
            Self::Brotli(state) => {
                state.feed(input, limit, output)?;
                if last && !state.done {
                    return Err(ProtocolError::InvalidContentCoding);
                }
                Ok(())
            }
        }
    }
}

fn decode_buffered(coding: Coding, input: &[u8], limit: usize) -> Result<Vec<u8>, ProtocolError> {
    use rustic_codecs::InflateError;
    let cancel = CancellationToken::default();
    let result = match coding {
        Coding::Gzip => rustic_codecs::gzip_decompress(input, limit, &cancel),
        // `deflate` means zlib; some servers send raw DEFLATE, which
        // browsers accept too.
        Coding::Deflate => {
            rustic_codecs::zlib_decompress(input, limit, &cancel).or_else(|error| match error {
                InflateError::Invalid => rustic_codecs::inflate(input, limit, &cancel),
                other => Err(other),
            })
        }
        Coding::Brotli => unreachable!("brotli streams"),
    };
    result.map_err(|error| match error {
        InflateError::OutputLimit => ProtocolError::BodyTooLarge,
        _ => ProtocolError::InvalidContentCoding,
    })
}

/// `Box<[T]>` cells for `brotli-decompressor`'s allocator interface, so the
/// crate's `std` feature (and its unsafe allocator) stays off.
struct Cell<T>(Box<[T]>);

impl<T> Default for Cell<T> {
    fn default() -> Self {
        Self(Vec::new().into_boxed_slice())
    }
}

impl<T> brotli_decompressor::SliceWrapper<T> for Cell<T> {
    fn slice(&self) -> &[T] {
        &self.0
    }
}

impl<T> brotli_decompressor::SliceWrapperMut<T> for Cell<T> {
    fn slice_mut(&mut self) -> &mut [T] {
        &mut self.0
    }
}

#[derive(Clone, Copy, Default)]
struct Heap;

impl<T: Clone + Default> brotli_decompressor::Allocator<T> for Heap {
    type AllocatedMemory = Cell<T>;
    fn alloc_cell(&mut self, len: usize) -> Cell<T> {
        Cell(vec![T::default(); len].into_boxed_slice())
    }
    fn free_cell(&mut self, _cell: Cell<T>) {}
}

/// Incremental brotli (RFC 7932). The decoder's window is at most 16 MiB
/// (large windows are not enabled).
struct Brotli {
    state: brotli_decompressor::BrotliState<Heap, Heap, Heap>,
    done: bool,
}

impl Default for Brotli {
    fn default() -> Self {
        Self {
            state: brotli_decompressor::BrotliState::new(Heap, Heap, Heap),
            done: false,
        }
    }
}

impl Brotli {
    fn feed(
        &mut self,
        input: &[u8],
        limit: usize,
        output: &mut Vec<u8>,
    ) -> Result<(), ProtocolError> {
        use brotli_decompressor::{BrotliDecompressStream, BrotliResult};
        if self.done {
            // Data after the end of the stream.
            return if input.is_empty() {
                Ok(())
            } else {
                Err(ProtocolError::InvalidContentCoding)
            };
        }
        let mut available_in = input.len();
        let mut input_offset = 0;
        let mut total_out = 0;
        let mut buffer = vec![0; 16 * 1024];
        loop {
            let mut available_out = buffer.len();
            let mut output_offset = 0;
            let result = BrotliDecompressStream(
                &mut available_in,
                &mut input_offset,
                input,
                &mut available_out,
                &mut output_offset,
                &mut buffer,
                &mut total_out,
                &mut self.state,
            );
            if output.len() + output_offset > limit {
                return Err(ProtocolError::BodyTooLarge);
            }
            output.extend_from_slice(&buffer[..output_offset]);
            match result {
                BrotliResult::NeedsMoreOutput => {}
                BrotliResult::NeedsMoreInput => return Ok(()),
                BrotliResult::ResultSuccess => {
                    self.done = true;
                    return if available_in == 0 {
                        Ok(())
                    } else {
                        Err(ProtocolError::InvalidContentCoding)
                    };
                }
                BrotliResult::ResultFailure => return Err(ProtocolError::InvalidContentCoding),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &[u8] = b"hello hello hello\n";
    const GZIP: &[u8] = include_bytes!("../tests/fixtures/coding/hello.gz");
    const ZLIB: &[u8] = include_bytes!("../tests/fixtures/coding/hello.zlib");
    const RAW: &[u8] = include_bytes!("../tests/fixtures/coding/hello.deflate");
    const BROTLI: &[u8] = include_bytes!("../tests/fixtures/coding/hello.br");
    /// `br` applied to the gzip file: `Content-Encoding: gzip, br`.
    const GZIP_BR: &[u8] = include_bytes!("../tests/fixtures/coding/hello.gz.br");

    fn headers(value: &str) -> Headers {
        let mut headers = Headers::new();
        headers
            .append("Content-Encoding", value.as_bytes())
            .unwrap();
        headers
    }

    fn decode_split(
        value: &str,
        body: &[u8],
        piece: usize,
        limit: u64,
    ) -> Result<Vec<u8>, ProtocolError> {
        let mut decoder = ContentDecoder::for_response(&headers(value), limit).expect("decodable");
        let mut output = Vec::new();
        for chunk in body.chunks(piece.max(1)) {
            decoder.feed(chunk, &mut output)?;
        }
        decoder.finish(&mut output)?;
        Ok(output)
    }

    #[test]
    fn every_coding_decodes_at_every_split() {
        for (value, body) in [
            ("gzip", GZIP),
            ("x-gzip", GZIP),
            ("deflate", ZLIB),
            ("deflate", RAW),
            ("br", BROTLI),
            ("BR", BROTLI),
            ("gzip, br", GZIP_BR),
            ("identity, gzip", GZIP),
        ] {
            for piece in [1, 2, 7, body.len()] {
                assert_eq!(
                    decode_split(value, body, piece, 1 << 20).unwrap(),
                    TEXT,
                    "{value} / {piece}"
                );
            }
        }
    }

    #[test]
    fn unknown_and_identity_codings_pass_through() {
        assert!(ContentDecoder::for_response(&Headers::new(), 10).is_none());
        assert!(ContentDecoder::for_response(&headers("identity"), 10).is_none());
        assert!(ContentDecoder::for_response(&headers("zstd"), 10).is_none());
        assert!(ContentDecoder::for_response(&headers("gzip, compress"), 10).is_none());
    }

    #[test]
    fn corrupt_truncated_and_oversized_bodies_fail() {
        assert_eq!(
            decode_split("gzip", &GZIP[..GZIP.len() - 3], 4, 1 << 20),
            Err(ProtocolError::InvalidContentCoding)
        );
        assert_eq!(
            decode_split("br", &BROTLI[..BROTLI.len() - 2], 4, 1 << 20),
            Err(ProtocolError::InvalidContentCoding)
        );
        let mut trailing = BROTLI.to_vec();
        trailing.push(0);
        assert_eq!(
            decode_split("br", &trailing, 3, 1 << 20),
            Err(ProtocolError::InvalidContentCoding)
        );
        assert_eq!(
            decode_split("gzip", b"not gzip", 3, 1 << 20),
            Err(ProtocolError::InvalidContentCoding)
        );
        assert_eq!(
            decode_split("br", BROTLI, 3, 17),
            Err(ProtocolError::BodyTooLarge)
        );
        assert_eq!(
            decode_split("gzip", GZIP, 3, 17),
            Err(ProtocolError::BodyTooLarge)
        );
        assert_eq!(decode_split("br", BROTLI, 3, 18).unwrap(), TEXT);
    }

    #[test]
    fn a_decompression_bomb_stops_at_the_limit() {
        let bomb = include_bytes!("../tests/fixtures/coding/zeros-16m.br");
        assert!(bomb.len() < 64);
        assert_eq!(
            decode_split("br", bomb, 1024, 1 << 20),
            Err(ProtocolError::BodyTooLarge)
        );
        let gzip_bomb = include_bytes!("../tests/fixtures/coding/zeros-16m.gz");
        assert_eq!(
            decode_split("gzip", gzip_bomb, 1024, 1 << 20),
            Err(ProtocolError::BodyTooLarge)
        );
    }
}
