use rustic_base::CancellationToken;
use rustic_wasm::{BinaryError, DecodeLimits, DecodedModule, decode_module};

pub fn module(sections: &[(u8, &[u8])]) -> Vec<u8> {
    let mut bytes = b"\0asm\x01\0\0\0".to_vec();
    for (id, payload) in sections {
        bytes.extend([*id, short_len(payload)]);
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
