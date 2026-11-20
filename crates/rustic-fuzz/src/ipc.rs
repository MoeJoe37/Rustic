//! `ipc-frame`: the broker/renderer protocol decoder (`rustic-ipc`) over
//! arbitrary frames from a hostile peer. Decoding never panics, and the
//! encoding is canonical: a frame that decodes re-encodes to the same bytes.

use rustic_ipc::protocol::{
    BrokerMessage, DocumentGrant, RendererMessage, ResourceKind, Start, StorageArea, StorageOp,
};
use rustic_ipc::{Message, decode_frame, encode_frame};

use crate::Target;

pub(crate) struct IpcFrameTarget;

pub(crate) fn seeds() -> Vec<Vec<u8>> {
    let mut seeds = Vec::new();
    for id in 0..30u32 {
        let mut broker = vec![0u8];
        broker.extend(encode_frame(&BrokerMessage::Start(Start {
            command: format!("dump-{id}"),
            args: (0..id % 4).map(|n| n.to_string()).collect(),
            document: DocumentGrant {
                url: "https://a.test/".into(),
                bytes: vec![id as u8; id as usize],
                transport_charset: (id % 2 == 0).then(|| "utf-8".into()),
                local: id % 3 == 0,
                csp: vec!["default-src 'self'".into()],
                ..DocumentGrant::default()
            },
        })));
        seeds.push(broker);
        for message in [
            RendererMessage::Resource {
                id,
                url: format!("https://a.test/{id}.css"),
                kind: if id % 2 == 0 {
                    ResourceKind::Image
                } else {
                    ResourceKind::Script {
                        nonce: Some("n".into()),
                        parser_inserted: id % 3 == 0,
                        module: false,
                    }
                },
                max_bytes: u64::from(id) * 1000,
            },
            RendererMessage::Storage {
                id,
                area: StorageArea::Local,
                op: StorageOp::Set(vec![id as u16], vec![0xD800]),
            },
            RendererMessage::Done {
                status: (id % 3) as u8,
                output: vec![b'x'; id as usize],
            },
        ] {
            let mut renderer = vec![1u8];
            renderer.extend(encode_frame(&message));
            seeds.push(renderer);
        }
    }
    seeds
}

fn canonical<M: Message>(payload: &[u8]) -> Result<(), String> {
    if let Ok(message) = decode_frame::<M>(payload) {
        if encode_frame(&message) != payload {
            return Err("a decoded frame re-encodes differently".into());
        }
    }
    Ok(())
}

impl Target for IpcFrameTarget {
    fn name(&self) -> &'static str {
        "ipc-frame"
    }

    fn max_len(&self) -> usize {
        4096
    }

    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[
            b"\x01\x00",
            b"\x02\x00",
            b"\x03\x00",
            b"\x04\x00",
            b"\x05\x00",
            b"\x00\x00\x00\x00",
            b"\xff\xff\xff\xff",
            b"\x01",
            b"\x02",
        ]
    }

    fn check(&self, input: &[u8]) -> Result<(), String> {
        let Some((&side, payload)) = input.split_first() else {
            return Ok(());
        };
        if side & 1 == 0 {
            canonical::<BrokerMessage>(payload)
        } else {
            canonical::<RendererMessage>(payload)
        }
    }
}
