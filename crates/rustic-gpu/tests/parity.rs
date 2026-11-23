//! `compositor.parity`: the GPU compositor's output is byte-identical to
//! the CPU reference (`Surface::add_frame`) on every fixture, and a lost
//! device is reported and recovered by reopening. Without an adapter the
//! tests report the fallback and pass: the CPU reference then composites.

use rustic_gpu::{AdapterChoice, Compositor, GpuError, Layer};
use rustic_raster::{RasterLimits, Surface};

/// A small deterministic generator (xorshift), so fixtures are the same on
/// every platform.
struct Bytes(u64);

impl Bytes {
    fn next(&mut self) -> u8 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 24) as u8
    }

    /// Premultiplied RGBA: every channel at most its alpha.
    fn surface(&mut self, width: u32, height: u32) -> Surface {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..width * height {
            let alpha = match self.next() % 4 {
                0 => 0,
                1 => 255,
                _ => self.next(),
            };
            for _ in 0..3 {
                let value = self.next();
                pixels.push((u16::from(value) % (u16::from(alpha) + 1)) as u8);
            }
            pixels.push(alpha);
        }
        Surface::from_premultiplied(width, height, pixels, RasterLimits::default()).unwrap()
    }

    fn coverage(&mut self, count: usize) -> Vec<u8> {
        (0..count)
            .map(|_| match self.next() % 3 {
                0 => 0,
                1 => 255,
                _ => self.next(),
            })
            .collect()
    }
}

fn open() -> Option<Compositor> {
    match Compositor::open(AdapterChoice::Any) {
        Ok(compositor) => {
            eprintln!("compositor: {:?}", compositor.info());
            Some(compositor)
        }
        Err(error) => {
            // CI sets this where an adapter exists (lavapipe on Linux), so
            // the parity gate cannot pass by skipping.
            assert!(
                std::env::var_os("RUSTIC_REQUIRE_GPU").is_none(),
                "RUSTIC_REQUIRE_GPU is set but no adapter opened: {error}"
            );
            eprintln!("no GPU here ({error}); the CPU reference composites");
            None
        }
    }
}

fn reference(page: &Surface, layers: &[Layer<'_>]) -> Surface {
    let mut out = page.clone();
    for layer in layers {
        out.add_frame(layer.frame, layer.rect, layer.coverage)
            .unwrap();
    }
    out
}

/// A frame's size and the rectangle it fills.
type LayerCase = ((u32, u32), (u32, u32, u32, u32));

#[test]
fn gpu_composition_is_byte_identical_to_the_cpu_reference() {
    let Some(gpu) = open() else {
        return;
    };
    let mut bytes = Bytes(0x9E37_79B9_7F4A_7C15);
    // Odd sizes, frames smaller than their rectangles (edge pixels
    // repeat), overlapping layers, zero coverage, empty rectangles.
    let cases: &[((u32, u32), &[LayerCase])] = &[
        ((1, 1), &[((1, 1), (0, 0, 1, 1))]),
        (
            (37, 23),
            &[((10, 7), (3, 4, 20, 15)), ((37, 23), (0, 0, 37, 23))],
        ),
        (
            (64, 64),
            &[
                ((8, 8), (60, 60, 4, 4)),
                ((1, 1), (0, 0, 64, 1)),
                ((5, 9), (10, 10, 0, 5)),
            ],
        ),
        (
            (300, 200),
            &[
                ((120, 90), (17, 33, 150, 100)),
                ((50, 50), (100, 50, 120, 140)),
            ],
        ),
    ];
    for (index, &((width, height), layers)) in cases.iter().enumerate() {
        let page = bytes.surface(width, height);
        let frames: Vec<Surface> = layers
            .iter()
            .map(|&((fw, fh), _)| bytes.surface(fw, fh))
            .collect();
        let coverages: Vec<Vec<u8>> = layers
            .iter()
            .map(|&(_, (_, _, w, h))| bytes.coverage((w * h) as usize))
            .collect();
        let layers: Vec<Layer<'_>> = layers
            .iter()
            .enumerate()
            .map(|(i, &(_, rect))| Layer {
                frame: &frames[i],
                rect,
                coverage: &coverages[i],
            })
            .collect();
        let expected = reference(&page, &layers);
        let actual = gpu.composite(&page, &layers).unwrap();
        assert_eq!(
            actual.premultiplied(),
            expected.premultiplied(),
            "case {index} differs from the CPU reference on {:?}",
            gpu.info()
        );
    }
}

#[test]
fn bad_layers_are_refused_and_a_lost_device_reopens() {
    let Some(gpu) = open() else {
        return;
    };
    let mut bytes = Bytes(7);
    let page = bytes.surface(8, 8);
    let frame = bytes.surface(4, 4);
    let coverage = vec![255; 16];
    for rect in [(6, 0, 4, 4), (0, 0, 4, 3)] {
        let layer = Layer {
            frame: &frame,
            rect,
            coverage: &coverage,
        };
        assert_eq!(
            gpu.composite(&page, &[layer]).err(),
            Some(GpuError::Invalid)
        );
    }
    let layer = Layer {
        frame: &frame,
        rect: (2, 2, 4, 4),
        coverage: &coverage,
    };
    assert!(gpu.composite(&page, &[layer]).is_ok());
    gpu.lose_device();
    assert!(gpu.is_lost());
    assert_eq!(gpu.composite(&page, &[layer]).err(), Some(GpuError::Lost));
    // Recovery: a new device composites again, with the same result.
    let fresh = Compositor::open(AdapterChoice::Any).unwrap();
    assert_eq!(
        fresh.composite(&page, &[layer]).unwrap().premultiplied(),
        reference(&page, &[layer]).premultiplied()
    );
}
