use rustic_base::CancellationToken;
use rustic_wasm::{BinaryError, DecodeLimits, DecodedModule, decode_module};

pub fn module(sections: &[(u8, &[u8])]) -> Vec<u8> {
    let mut bytes = b"\0asm\x01\0\0\0".to_vec();
    for (id, payload) in sections {
        bytes.push(*id);
        if payload.len() < 128 {
            bytes.push(short_len(payload));
        } else {
            let mut len = u32::try_from(payload.len()).unwrap();
            while len >= 0x80 {
                bytes.push((len as u8 & 0x7f) | 0x80);
                len >>= 7;
            }
            bytes.push(len as u8);
        }
        bytes.extend(*payload);
    }
    bytes
}

pub fn short_len(bytes: &[u8]) -> u8 {
    assert!(bytes.len() < 128);
    bytes.len().try_into().unwrap()
}

pub fn decode(bytes: &[u8]) -> Result<DecodedModule<'_>, BinaryError> {
    decode_module(
        bytes,
        DecodeLimits::default(),
        &CancellationToken::default(),
    )
}
