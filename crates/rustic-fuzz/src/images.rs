//! Mutated JPEG, GIF and WebP files exercise the bounded image decoders.

use crate::Target;
use rustic_base::CancellationToken;
use rustic_codecs::{DecodeLimits, decode_animation, decode_image};
use std::path::Path;

/// One image format's fuzz target.
pub(crate) struct ImageTarget {
    name: &'static str,
    extension: &'static str,
    dictionary: &'static [&'static [u8]],
}

pub(crate) const JPEG: ImageTarget = ImageTarget {
    name: "codec-jpeg",
    extension: "jpg",
    dictionary: &[
        b"\xff\xd8\xff",
        b"\xff\xc0\x00\x11\x08",
        b"\xff\xc2\x00\x11\x08",
        b"\xff\xc4",
        b"\xff\xdb\x00\x43\x00",
        b"\xff\xda\x00\x0c\x03",
        b"\xff\xdd\x00\x04\x00\x01",
        b"\xff\xd0",
        b"\xff\xd9",
        b"\xff\x00",
        b"\x11\x22\x33",
    ],
};

pub(crate) const GIF: ImageTarget = ImageTarget {
    name: "codec-gif",
    extension: "gif",
    dictionary: &[
        b"GIF89a",
        b"\x21\xf9\x04",
        b"\x21\xff\x0bNETSCAPE2.0\x03\x01",
        b"\x2c\x00\x00\x00\x00",
        b"\x02\x02\x44\x01\x00",
        b"\x3b",
        b"\x00",
        b"\xff",
    ],
};

pub(crate) const WEBP: ImageTarget = ImageTarget {
    name: "codec-webp",
    extension: "webp",
    dictionary: &[
        b"RIFF",
        b"WEBPVP8 ",
        b"WEBPVP8L",
        b"VP8X",
        b"ALPH",
        b"ANIM",
        b"ANMF",
        b"\x9d\x01\x2a",
        b"\x2f",
        b"\x00\x00\x00\x00",
        b"\xff\xff\xff\xff",
    ],
};

impl Target for ImageTarget {
    fn name(&self) -> &'static str {
        self.name
    }
    fn max_len(&self) -> usize {
        32_768
    }
    fn dictionary(&self) -> &'static [&'static [u8]] {
        self.dictionary
    }
    fn check(&self, input: &[u8]) -> Result<(), String> {
        let limits = DecodeLimits {
            max_width: 2048,
            max_height: 2048,
            max_bytes: 8 << 20,
            max_frames: 8,
        };
        let cancel = CancellationToken::default();
        let first = decode_animation(input, limits, &cancel);
        if let Ok(animation) = &first {
            if animation.frames.is_empty() || animation.frames.len() > limits.max_frames {
                return Err("animation has a frame count outside its limits".into());
            }
            if animation.byte_len() > limits.max_bytes {
                return Err("animation exceeds its byte budget".into());
            }
            for frame in &animation.frames {
                let image = &frame.image;
                if image.pixels.len() != image.width as usize * image.height as usize * 4 {
                    return Err("decoded pixel buffer has the wrong size".into());
                }
            }
            match decode_image(input, limits, &cancel) {
                Ok(still) if still == animation.frames[0].image => {}
                _ => return Err("first frame differs from the still decode".into()),
            }
        }
        if decode_animation(input, limits, &cancel) != first {
            return Err("image decoding is not deterministic".into());
        }
        Ok(())
    }
}

/// The conformance images of this format, each also cut at every
/// sixteenth of its length to seed the error paths.
fn format_seeds(root: &Path, extension: &str) -> std::io::Result<Vec<Vec<u8>>> {
    let dir = root.join("crates/rustic-codecs/tests/images");
    let mut files: Vec<_> = std::fs::read_dir(dir)?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<_>>()?;
    files.retain(|p| p.extension().is_some_and(|e| e == extension));
    files.sort();
    let mut seeds = Vec::new();
    for file in files {
        let bytes = std::fs::read(file)?;
        for part in 1..16 {
            seeds.push(bytes[..bytes.len() * part / 16].to_vec());
        }
        seeds.push(bytes);
    }
    Ok(seeds)
}

impl ImageTarget {
    pub(crate) fn seeds(&self, root: &Path) -> std::io::Result<Vec<Vec<u8>>> {
        format_seeds(root, self.extension)
    }
}
