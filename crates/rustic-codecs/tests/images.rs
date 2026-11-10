//! JPEG, GIF and WebP conformance against references decoded by an
//! independent decoder (see tools/codec_reference.py and tests/images/README).
use rustic_base::CancellationToken;
use rustic_codecs::{Animation, DecodeLimits, ImageError, decode_animation, decode_image};
use std::fs;
use std::path::{Path, PathBuf};

struct Case {
    path: PathBuf,
    width: u32,
    height: u32,
    frames: usize,
    tolerance: u8,
}

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/images")
}

fn manifest() -> Vec<Case> {
    fs::read_to_string(dir().join("MANIFEST"))
        .unwrap()
        .lines()
        .map(|line| {
            let f: Vec<&str> = line.split(' ').collect();
            Case {
                path: dir().join(f[0]),
                width: f[1].parse().unwrap(),
                height: f[2].parse().unwrap(),
                frames: f[3].parse().unwrap(),
                tolerance: f[4].parse().unwrap(),
            }
        })
        .collect()
}

fn animation(bytes: &[u8]) -> Result<Animation, ImageError> {
    decode_animation(
        bytes,
        DecodeLimits::default(),
        &CancellationToken::default(),
    )
}

fn check(case: &Case, extension: &str) -> usize {
    let name = case
        .path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    if !name.ends_with(extension) {
        return 0;
    }
    let bytes = fs::read(&case.path).unwrap();
    let decoded = animation(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(decoded.frames.len(), case.frames, "{name}: frame count");
    let expected = fs::read(case.path.with_extension("rgba")).unwrap();
    let frame_len = (case.width * case.height * 4) as usize;
    assert_eq!(expected.len(), frame_len * case.frames, "{name}");
    let mut worst = 0;
    for (index, frame) in decoded.frames.iter().enumerate() {
        let image = &frame.image;
        assert_eq!(
            (image.width, image.height),
            (case.width, case.height),
            "{name}"
        );
        let reference = &expected[index * frame_len..][..frame_len];
        for (i, (a, b)) in image.pixels.iter().zip(reference).enumerate() {
            worst = worst.max(a.abs_diff(*b));
            assert!(
                a.abs_diff(*b) <= case.tolerance,
                "{name} frame {index}: byte {i} (pixel {}, {}) is {a}, expected {b}",
                (i / 4) % case.width as usize,
                (i / 4) / case.width as usize
            );
        }
    }
    // The single-frame entry point returns the first frame.
    let first = decode_image(
        &bytes,
        DecodeLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    assert_eq!(first, decoded.frames[0].image, "{name}");
    eprintln!("{name}: max difference {worst}");
    1
}

#[test]
fn every_jpeg_matches_its_reference() {
    assert_eq!(
        manifest().iter().map(|c| check(c, ".jpg")).sum::<usize>(),
        20
    );
}

#[test]
fn every_gif_matches_its_reference() {
    assert_eq!(
        manifest().iter().map(|c| check(c, ".gif")).sum::<usize>(),
        8
    );
}

#[test]
fn every_webp_matches_its_reference() {
    assert_eq!(
        manifest().iter().map(|c| check(c, ".webp")).sum::<usize>(),
        19
    );
}

#[test]
fn animation_timing_and_loop_counts_are_reported() {
    let gif = animation(&fs::read(dir().join("animated-disposal-1.gif")).unwrap()).unwrap();
    let delays: Vec<u32> = gif.frames.iter().map(|f| f.delay_ms).collect();
    assert_eq!(delays, [70, 120, 30, 200]);
    assert_eq!(gif.loop_count, 0);
    let webp = animation(&fs::read(dir().join("animated-lossless.webp")).unwrap()).unwrap();
    let delays: Vec<u32> = webp.frames.iter().map(|f| f.delay_ms).collect();
    assert_eq!(delays, [50, 80, 120, 40, 90]);
    assert_eq!(webp.loop_count, 3);
    assert_eq!(webp.byte_len(), 5 * 40 * 30 * 4);
    let still = animation(&fs::read(dir().join("still-16.gif")).unwrap()).unwrap();
    assert_eq!(still.loop_count, 1, "no NETSCAPE block plays once");
}

#[test]
fn every_truncation_and_bit_flip_fails_or_decodes_cleanly() {
    // Truncated or corrupted files must never panic; a successful decode
    // must still have consistent dimensions.
    let limits = DecodeLimits {
        max_width: 4096,
        max_height: 4096,
        max_bytes: 64 << 20,
        max_frames: 16,
    };
    for case in manifest() {
        let bytes = fs::read(&case.path).unwrap();
        let step = (bytes.len() / 97).max(1);
        let mut variants: Vec<Vec<u8>> = (0..bytes.len())
            .step_by(step)
            .map(|len| bytes[..len].to_vec())
            .collect();
        for i in (0..bytes.len()).step_by(step) {
            let mut flipped = bytes.clone();
            flipped[i] ^= 1 << (i % 8);
            variants.push(flipped);
        }
        for variant in variants {
            let cancel = CancellationToken::default();
            if let Ok(decoded) = decode_animation(&variant, limits, &cancel) {
                assert!(!decoded.frames.is_empty());
                for frame in &decoded.frames {
                    let image = &frame.image;
                    assert_eq!(
                        image.pixels.len(),
                        image.width as usize * image.height as usize * 4
                    );
                }
            }
        }
    }
}

#[test]
fn limits_and_cancellation_stop_before_decoding() {
    for case in manifest() {
        let bytes = fs::read(&case.path).unwrap();
        let name = case.path.display();
        let small = DecodeLimits {
            max_width: case.width - 1,
            ..DecodeLimits::default()
        };
        if case.width > 1 {
            assert_eq!(
                decode_image(&bytes, small, &CancellationToken::default()),
                Err(ImageError::Limit),
                "{name}"
            );
        }
        let tight = DecodeLimits {
            max_bytes: (case.width * case.height * 4) as usize - 1,
            ..DecodeLimits::default()
        };
        assert_eq!(
            decode_image(&bytes, tight, &CancellationToken::default()),
            Err(ImageError::Limit),
            "{name}"
        );
        let cancel = CancellationToken::default();
        cancel.cancel();
        assert_eq!(
            decode_animation(&bytes, DecodeLimits::default(), &cancel),
            Err(ImageError::Cancelled),
            "{name}"
        );
    }
}

#[test]
fn frame_limits_keep_the_leading_frames() {
    let bytes = fs::read(dir().join("animated-mixed.webp")).unwrap();
    let limits = DecodeLimits {
        max_frames: 2,
        ..DecodeLimits::default()
    };
    let two = decode_animation(&bytes, limits, &CancellationToken::default()).unwrap();
    let all = animation(&bytes).unwrap();
    assert_eq!(two.frames[..], all.frames[..2]);
    // A byte budget for three canvases keeps three frames.
    let bytes = fs::read(dir().join("animated-disposal-2.gif")).unwrap();
    let limits = DecodeLimits {
        max_bytes: 3 * 24 * 18 * 4,
        ..DecodeLimits::default()
    };
    let three = decode_animation(&bytes, limits, &CancellationToken::default()).unwrap();
    assert_eq!(three.frames.len(), 3);
}

#[test]
fn exif_orientation_swaps_natural_dimensions() {
    let bytes = fs::read(dir().join("exif-orientation-6.jpg")).unwrap();
    let image = decode_image(
        &bytes,
        DecodeLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    assert_eq!((image.width, image.height), (13, 21));
}

#[test]
fn probing_matches_the_decoded_size_of_every_image() {
    for case in manifest() {
        let bytes = fs::read(&case.path).unwrap();
        let info = rustic_codecs::probe(&bytes).unwrap();
        assert_eq!(
            (info.width, info.height),
            (case.width, case.height),
            "{:?}",
            case.path
        );
    }
}
