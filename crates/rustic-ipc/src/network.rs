//! The protocol between the broker and its network process (ISO-07).
//!
//! The broker decides *whether* a request may be sent (policy, CSP, CORS,
//! cookies, cache) and *where* (the address its policy grants); the network
//! process only runs the exchange: socket, TLS, HTTP/1.1 framing and
//! content decoding, the code that parses hostile bytes from servers. The
//! broker starts it with [`NetworkCommand::Configure`], then opens numbered
//! exchanges. Replies stream back as [`NetworkEvent`]s: one head, body
//! pieces, then one final event. Body bytes follow credit: the network
//! process may have at most the exchange's `window` bytes unacknowledged,
//! and the broker returns credit with [`NetworkCommand::Credit`] as the
//! consumer reads. The broker treats every reply as hostile input and
//! revalidates it.

use crate::Message;
use crate::protocol::{get_headers, get_opt_bytes, put_headers, put_opt_bytes};
use crate::wire::{DecodeError, Reader, Writer};

/// Largest `Configure` certificate bundle.
pub const MAX_CA_BYTES: usize = 1024 * 1024;

/// The per-exchange limits the broker imposes; the network process
/// enforces them while parsing. Times are milliseconds.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExchangeLimits {
    /// Status line plus header bytes.
    pub max_head_bytes: u64,
    /// Header fields in the head.
    pub max_header_count: u64,
    /// Decoded body bytes.
    pub max_body_bytes: u64,
    /// One chunk-size line.
    pub max_chunk_line_bytes: u64,
    /// The trailer section.
    pub max_trailer_bytes: u64,
    /// Interim responses skipped.
    pub max_interim_responses: u64,
    /// Body bytes in flight before the network process waits for credit.
    pub window: u64,
    /// One socket read.
    pub read_chunk: u64,
    /// Connecting.
    pub connect_ms: u64,
    /// No socket progress.
    pub idle_ms: u64,
    /// The whole exchange, if bounded.
    pub total_ms: Option<u64>,
    /// Longest blocking step between checks.
    pub poll_ms: u64,
}

/// One request the broker has already granted.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Open {
    /// The exchange number, unique for the life of the process.
    pub id: u64,
    /// The method token.
    pub method: String,
    /// The absolute `http`/`https` URL.
    pub url: String,
    /// Request header fields.
    pub headers: Vec<(String, Vec<u8>)>,
    /// The request body.
    pub body: Vec<u8>,
    /// The socket address the broker's policy grants for the URL's origin.
    pub address: String,
    /// Limits for this exchange.
    pub limits: ExchangeLimits,
}

/// Broker to network process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NetworkCommand {
    /// The first message: trust roots replacing the platform store (a
    /// fixture certificate authority), if any.
    Configure {
        /// PEM certificates.
        ca_pem: Option<Vec<u8>>,
    },
    /// Start an exchange.
    Open(Box<Open>),
    /// Stop an exchange; a `Failed` (cancelled) or its natural end follows.
    Cancel {
        /// The exchange.
        id: u64,
    },
    /// The consumer read `bytes` more body bytes.
    Credit {
        /// The exchange.
        id: u64,
        /// Bytes returned to the window.
        bytes: u64,
    },
}

/// A response head.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Head {
    /// The exchange.
    pub id: u64,
    /// `true` for HTTP/1.1, `false` for HTTP/1.0.
    pub http11: bool,
    /// The status code.
    pub status: u16,
    /// The reason phrase.
    pub reason: Vec<u8>,
    /// Header fields.
    pub headers: Vec<(String, Vec<u8>)>,
}

/// Network process to broker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NetworkEvent {
    /// The final response head.
    Head(Box<Head>),
    /// Decoded body bytes (never empty).
    Body {
        /// The exchange.
        id: u64,
        /// The bytes.
        bytes: Vec<u8>,
    },
    /// The body is complete.
    Finished {
        /// The exchange.
        id: u64,
    },
    /// The exchange failed; `code` and `detail` name the transport error.
    Failed {
        /// The exchange.
        id: u64,
        /// The error kind.
        code: u16,
        /// A sub-kind (I/O error kind, certificate error, protocol error).
        detail: u16,
    },
}

fn put_limits(writer: &mut Writer, limits: &ExchangeLimits) {
    for value in [
        limits.max_head_bytes,
        limits.max_header_count,
        limits.max_body_bytes,
        limits.max_chunk_line_bytes,
        limits.max_trailer_bytes,
        limits.max_interim_responses,
        limits.window,
        limits.read_chunk,
        limits.connect_ms,
        limits.idle_ms,
    ] {
        writer.u64(value);
    }
    match limits.total_ms {
        Some(total) => {
            writer.u8(1);
            writer.u64(total);
        }
        None => writer.u8(0),
    }
    writer.u64(limits.poll_ms);
}

fn get_limits(reader: &mut Reader<'_>) -> Result<ExchangeLimits, DecodeError> {
    Ok(ExchangeLimits {
        max_head_bytes: reader.u64()?,
        max_header_count: reader.u64()?,
        max_body_bytes: reader.u64()?,
        max_chunk_line_bytes: reader.u64()?,
        max_trailer_bytes: reader.u64()?,
        max_interim_responses: reader.u64()?,
        window: reader.u64()?,
        read_chunk: reader.u64()?,
        connect_ms: reader.u64()?,
        idle_ms: reader.u64()?,
        total_ms: match reader.u8()? {
            0 => None,
            1 => Some(reader.u64()?),
            _ => return Err(DecodeError::InvalidFlag),
        },
        poll_ms: reader.u64()?,
    })
}

impl Message for NetworkCommand {
    fn encode(&self, writer: &mut Writer) {
        match self {
            Self::Configure { ca_pem } => {
                writer.u16(1);
                put_opt_bytes(writer, ca_pem.as_deref());
            }
            Self::Open(open) => {
                writer.u16(2);
                writer.u64(open.id);
                writer.str(&open.method);
                writer.str(&open.url);
                put_headers(writer, &open.headers);
                writer.bytes(&open.body);
                writer.str(&open.address);
                put_limits(writer, &open.limits);
            }
            Self::Cancel { id } => {
                writer.u16(3);
                writer.u64(*id);
            }
            Self::Credit { id, bytes } => {
                writer.u16(4);
                writer.u64(*id);
                writer.u64(*bytes);
            }
        }
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        match reader.u16()? {
            1 => {
                let ca_pem = get_opt_bytes(reader)?;
                if ca_pem.as_ref().is_some_and(|pem| pem.len() > MAX_CA_BYTES) {
                    return Err(DecodeError::OutOfRange);
                }
                Ok(Self::Configure { ca_pem })
            }
            2 => Ok(Self::Open(Box::new(Open {
                id: reader.u64()?,
                method: reader.string()?,
                url: reader.string()?,
                headers: get_headers(reader)?,
                body: reader.bytes()?,
                address: reader.string()?,
                limits: get_limits(reader)?,
            }))),
            3 => Ok(Self::Cancel { id: reader.u64()? }),
            4 => Ok(Self::Credit {
                id: reader.u64()?,
                bytes: reader.u64()?,
            }),
            tag => Err(DecodeError::UnknownTag(tag)),
        }
    }
}

impl Message for NetworkEvent {
    fn encode(&self, writer: &mut Writer) {
        match self {
            Self::Head(head) => {
                writer.u16(1);
                writer.u64(head.id);
                writer.bool(head.http11);
                writer.u16(head.status);
                writer.bytes(&head.reason);
                put_headers(writer, &head.headers);
            }
            Self::Body { id, bytes } => {
                writer.u16(2);
                writer.u64(*id);
                writer.bytes(bytes);
            }
            Self::Finished { id } => {
                writer.u16(3);
                writer.u64(*id);
            }
            Self::Failed { id, code, detail } => {
                writer.u16(4);
                writer.u64(*id);
                writer.u16(*code);
                writer.u16(*detail);
            }
        }
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        match reader.u16()? {
            1 => Ok(Self::Head(Box::new(Head {
                id: reader.u64()?,
                http11: reader.bool()?,
                status: reader.u16()?,
                reason: reader.bytes()?,
                headers: get_headers(reader)?,
            }))),
            2 => Ok(Self::Body {
                id: reader.u64()?,
                bytes: reader.bytes()?,
            }),
            3 => Ok(Self::Finished { id: reader.u64()? }),
            4 => Ok(Self::Failed {
                id: reader.u64()?,
                code: reader.u16()?,
                detail: reader.u16()?,
            }),
            tag => Err(DecodeError::UnknownTag(tag)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode_frame, encode_frame};

    fn round_trip<M: Message + PartialEq + std::fmt::Debug>(message: M) {
        let bytes = encode_frame(&message);
        assert_eq!(decode_frame::<M>(&bytes).unwrap(), message);
        for cut in 0..bytes.len() {
            assert!(decode_frame::<M>(&bytes[..cut]).is_err(), "prefix {cut}");
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(decode_frame::<M>(&longer).is_err());
    }

    #[test]
    fn every_message_round_trips_and_every_prefix_is_refused() {
        round_trip(NetworkCommand::Configure { ca_pem: None });
        round_trip(NetworkCommand::Configure {
            ca_pem: Some(b"-----BEGIN".to_vec()),
        });
        round_trip(NetworkCommand::Open(Box::new(Open {
            id: 7,
            method: "POST".into(),
            url: "https://a.test/x".into(),
            headers: vec![("Accept".into(), b"*/*".to_vec())],
            body: b"body".to_vec(),
            address: "127.0.0.1:443".into(),
            limits: ExchangeLimits {
                total_ms: Some(5),
                window: 9,
                ..ExchangeLimits::default()
            },
        })));
        round_trip(NetworkCommand::Cancel { id: 3 });
        round_trip(NetworkCommand::Credit { id: 3, bytes: 99 });
        round_trip(NetworkEvent::Head(Box::new(Head {
            id: 1,
            http11: true,
            status: 404,
            reason: b"Not Found".to_vec(),
            headers: vec![("A".into(), b"b".to_vec())],
        })));
        round_trip(NetworkEvent::Body {
            id: 1,
            bytes: vec![0, 1, 2],
        });
        round_trip(NetworkEvent::Finished { id: 1 });
        round_trip(NetworkEvent::Failed {
            id: 1,
            code: 4,
            detail: 2,
        });
    }

    #[test]
    fn unknown_tags_and_oversized_bundles_are_refused() {
        assert!(decode_frame::<NetworkCommand>(&[9, 0]).is_err());
        assert!(decode_frame::<NetworkEvent>(&[0, 0]).is_err());
        let huge = encode_frame(&NetworkCommand::Configure {
            ca_pem: Some(vec![b'a'; MAX_CA_BYTES + 1]),
        });
        assert!(decode_frame::<NetworkCommand>(&huge).is_err());
    }
}
