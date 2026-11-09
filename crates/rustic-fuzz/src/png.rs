//! Mutated PNG files exercise the bounded inflate and PNG decoders.

use crate::Target;
use rustic_base::CancellationToken;
use rustic_codecs::{PngLimits, decode_png, inflate};
use std::path::Path;

pub(crate) struct PngTarget;
impl Target for PngTarget {
    fn name(&self) -> &'static str {
        "codec-png"
    }
    fn max_len(&self) -> usize {
        16_384
    }
    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[
            b"\x89PNG\r\n\x1a\n",
            b"IHDR",
            b"PLTE",
            b"tRNS",
            b"IDAT",
            b"IEND",
            b"\x78\x9c",
            b"\x78\x01",
            b"\x00\x00\x00\x00",
            b"\xff\xff\xff\xff",
        ]
    }
    fn check(&self, input: &[u8]) -> Result<(), String> {
        let limits = PngLimits {
            max_width: 2048,
            max_height: 2048,
            max_bytes: 4 << 20,
            max_chunks: 4096,
        };
        let cancel = CancellationToken::default();
        let first = decode_png(input, limits, &cancel);
        if let Ok(image) = &first {
            if image.pixels.len() != image.width as usize * image.height as usize * 4 {
                return Err("decoded pixel buffer has the wrong size".into());
            }
        }
        if decode_png(input, limits, &cancel) != first {
            return Err("PNG decoding is not deterministic".into());
        }
        // Raw inflate of the same bytes must respect its output bound.
        if let Ok(out) = inflate(input, 1 << 16, &cancel) {
            if out.len() > 1 << 16 {
                return Err("inflate exceeded its output limit".into());
            }
        }
        Ok(())
    }
}

/// PngSuite images vendored for the decoder's conformance tests.
pub(crate) fn seeds(root: &Path) -> std::io::Result<Vec<Vec<u8>>> {
    let dir = root.join("crates/rustic-codecs/tests/pngsuite");
    let mut files: Vec<_> = std::fs::read_dir(dir)?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<_>>()?;
    files.retain(|p| p.extension().is_some_and(|e| e == "png"));
    files.sort();
    files.iter().map(std::fs::read).collect()
}
