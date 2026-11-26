mod common;

use common::{decode, module};
use rustic_base::CancellationToken;
use rustic_wasm::{BinaryErrorKind, DecodeLimits, StreamingDecoder, validate_module};

#[test]
fn streaming_decodes_at_every_split_and_retains_reflection_bytes() {
    let mut custom = vec![4, b'n', b'a', b'm', b'e'];
    custom.resize(150, 42);
    let bytes = module(&[
        (0, &custom),
        (1, &[1, 0x60, 0, 1, 0x7f]),
        (3, &[1, 0]),
        (5, &[1, 0, 1]),
        (7, &[1, 3, b'r', b'u', b'n', 0, 0]),
        (12, &[1]),
        (10, &[1, 4, 0, 0x41, 42, 0x0b]),
        (11, &[1, 1, 3, 1, 2, 3]),
    ]);
    let limits = DecodeLimits::default();
    let cancellation = CancellationToken::default();
    let expected = decode(&bytes).unwrap();
    let finish = |decoder: StreamingDecoder<'_>| {
        let (original, ()) = decoder
            .finish(|snapshot, actual| {
                assert_eq!(snapshot, bytes);
                assert_eq!(actual, expected);
                validate_module(actual, Default::default(), &cancellation).unwrap();
            })
            .unwrap();
        assert_eq!(original, bytes);
    };
    for split in 0..=bytes.len() {
        let mut decoder = StreamingDecoder::new(limits, &cancellation);
        decoder.push(&bytes[..split]).unwrap();
        decoder.push(&bytes[split..]).unwrap();
        finish(decoder);
    }
    let mut decoder = StreamingDecoder::new(limits, &cancellation);
    for byte in &bytes {
        decoder.push(std::slice::from_ref(byte)).unwrap();
    }
    finish(decoder);
}

#[test]
fn streaming_reports_early_errors_and_checks_limits_and_eof() {
    let limits = DecodeLimits::default();
    let cancellation = CancellationToken::default();
    let bytes = module(&[
        (1, &[1, 0x60, 0, 0]),
        (3, &[1, 0]),
        (10, &[1, 3, 0, 0xff, 0x0b]),
        (0, &[0, 1, 2]),
    ]);
    let mut decoder = StreamingDecoder::new(limits, &cancellation);
    let suffix = bytes.len() - 5;
    let early = decoder.push(&bytes[..suffix]).unwrap_err();
    assert_eq!(early.kind, BinaryErrorKind::InvalidInstruction);
    assert_eq!(decoder.push(&bytes[suffix..]).unwrap_err(), early);
    assert_eq!(decoder.finish(|_, _| ()).unwrap_err(), early);

    let bytes = module(&[(0, &[0, 1, 2])]);
    let mut truncated = StreamingDecoder::new(limits, &cancellation);
    truncated.push(&bytes[..bytes.len() - 1]).unwrap();
    assert_eq!(
        truncated.finish(|_, _| ()).unwrap_err().kind,
        BinaryErrorKind::UnexpectedEnd
    );
    let mut exact = limits;
    exact.binary.module_bytes = bytes.len();
    exact.binary.sections = 1;
    let mut too_long = StreamingDecoder::new(exact, &cancellation);
    too_long.push(&bytes).unwrap();
    assert_eq!(
        too_long.push(&[0]).unwrap_err().kind,
        BinaryErrorKind::LimitExceeded
    );
    exact.binary.module_bytes += 2;
    let mut too_many = StreamingDecoder::new(exact, &cancellation);
    too_many.push(&bytes).unwrap();
    assert_eq!(
        too_many.push(&[0, 0]).unwrap_err().kind,
        BinaryErrorKind::LimitExceeded
    );

    let mut cancelled = StreamingDecoder::new(limits, &cancellation);
    cancelled.push(&bytes).unwrap();
    cancellation.cancel();
    assert_eq!(
        cancelled.finish(|_, _| ()).unwrap_err().kind,
        BinaryErrorKind::Cancelled
    );
}
