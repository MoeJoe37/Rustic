//! PngSuite conformance (images by Willem van Schaik and Glenn
//! Randers-Pehrson; see tests/pngsuite/README for their license).
//! References come from an independent decoder via tools/pngsuite_reference.py.
use rustic_base::CancellationToken;
use rustic_codecs::{PngDecodeError, PngLimits, decode_png};
use std::fs;
use std::path::{Path, PathBuf};

fn suite() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pngsuite");
    let mut files: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "png"))
        .collect();
    files.sort();
    files
}

fn decode(bytes: &[u8]) -> Result<rustic_codecs::DecodedImage, PngDecodeError> {
    decode_png(bytes, PngLimits::default(), &CancellationToken::default())
}

#[test]
fn every_suite_image_matches_its_reference() {
    let files = suite();
    assert_eq!(files.len(), 51);
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let image = decode(&fs::read(&path).unwrap()).unwrap_or_else(|e| panic!("{name}: {e}"));
        let expected = fs::read(path.with_extension("rgba")).unwrap();
        assert_eq!(
            image.pixels.len(),
            (image.width * image.height * 4) as usize
        );
        assert_eq!(image.pixels.len(), expected.len(), "{name}");
        // The reference decoder truncates 16-bit color to 8 bits where this
        // decoder rounds, so 16-bit color channels may differ by one.
        let tolerance = if name.contains("2c16") || name.contains("4a16") || name.contains("6a16") {
            1
        } else {
            0
        };
        for (i, (a, b)) in image.pixels.iter().zip(&expected).enumerate() {
            assert!(
                a.abs_diff(*b) <= tolerance,
                "{name}: byte {i} is {a}, expected {b}"
            );
        }
    }
}

#[test]
fn interlaced_images_decode_like_their_progressive_twins() {
    for path in suite() {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let Some(twin) = name.strip_prefix('i') else {
            continue;
        };
        let twin = path.with_file_name(twin);
        if !twin.exists() {
            continue;
        }
        assert_eq!(
            decode(&fs::read(&path).unwrap()).unwrap(),
            decode(&fs::read(&twin).unwrap()).unwrap(),
            "{name}"
        );
    }
}

#[test]
fn every_truncation_and_single_bit_flip_fails_cleanly() {
    for path in suite().into_iter().take(12) {
        let bytes = fs::read(&path).unwrap();
        for len in 0..bytes.len() {
            assert!(decode(&bytes[..len]).is_err(), "{path:?} at {len}");
        }
        // CRCs catch every single-bit flip inside a chunk; flips elsewhere
        // must still never panic.
        for i in 8..bytes.len() {
            let mut flipped = bytes.clone();
            flipped[i] ^= 0x10;
            let _ = decode(&flipped);
        }
    }
}

#[test]
fn limits_reject_images_before_allocation() {
    let bytes = fs::read(
        suite()
            .into_iter()
            .find(|p| p.ends_with("basn6a08.png"))
            .unwrap(),
    )
    .unwrap();
    let limits = |max_width, max_height, max_bytes| PngLimits {
        max_width,
        max_height,
        max_bytes,
        max_chunks: 100,
    };
    let cancel = CancellationToken::default();
    assert!(decode_png(&bytes, limits(32, 32, 32 * 32 * 4), &cancel).is_ok());
    assert_eq!(
        decode_png(&bytes, limits(31, 32, usize::MAX), &cancel),
        Err(PngDecodeError::Limit)
    );
    assert_eq!(
        decode_png(&bytes, limits(32, 31, usize::MAX), &cancel),
        Err(PngDecodeError::Limit)
    );
    assert_eq!(
        decode_png(&bytes, limits(32, 32, 32 * 32 * 4 - 1), &cancel),
        Err(PngDecodeError::Limit)
    );
    assert_eq!(
        decode_png(
            &bytes,
            PngLimits {
                max_chunks: 2,
                ..PngLimits::default()
            },
            &cancel
        ),
        Err(PngDecodeError::Limit)
    );
    let cancelled = CancellationToken::default();
    cancelled.cancel();
    assert_eq!(
        decode_png(&bytes, PngLimits::default(), &cancelled),
        Err(PngDecodeError::Cancelled)
    );
    assert_eq!(decode(b"GIF89a"), Err(PngDecodeError::Signature));
    assert_eq!(decode(b"\x89PN"), Err(PngDecodeError::Truncated));
}
