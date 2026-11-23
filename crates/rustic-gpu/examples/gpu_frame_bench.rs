//! `compositor.parity` frame-time benchmark: composite a page with frame
//! layers on the GPU and with the CPU reference, and print
//! `sample <scene>.<gpu|cpu> <ms> <check>` for every run, in the format
//! `tools/cross_engine_bench.py` reads (`--gpu`).
//!
//! ```text
//! cargo run --release -p rustic-gpu --example gpu_frame_bench -- [--runs K]
//! ```
//!
//! Each GPU sample is one whole frame as the GPU process does it: upload the
//! page and layers, dispatch, and read the result back. Scenes are generated
//! here, so they are the same on every device. The check value is a checksum
//! of the composited pixels; the GPU and CPU checks of a scene must match
//! (byte-exact parity), or the GPU samples are not printed. Without an
//! adapter the GPU scenes print `not runnable` on standard error, and with
//! `--require` the run fails instead.

use std::process::ExitCode;
use std::time::Instant;

use rustic_gpu::{AdapterChoice, Compositor, Layer};
use rustic_raster::{RasterLimits, Surface};

/// A deterministic generator (xorshift), so scenes match on every device.
struct Bytes(u64);

impl Bytes {
    fn next(&mut self) -> u8 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 24) as u8
    }

    /// Premultiplied RGBA, mostly opaque like real page content.
    fn surface(&mut self, width: u32, height: u32) -> Surface {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..width * height {
            let alpha = if self.next() % 8 == 0 {
                self.next()
            } else {
                255
            };
            for _ in 0..3 {
                let value = self.next();
                pixels.push((u16::from(value) % (u16::from(alpha) + 1)) as u8);
            }
            pixels.push(alpha);
        }
        Surface::from_premultiplied(width, height, pixels, RasterLimits::default())
            .expect("a valid scene surface")
    }

    /// Full coverage inside, antialiased (partial) at the edges.
    fn coverage(&mut self, width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                let edge = x == 0 || y == 0 || x + 1 == width || y + 1 == height;
                out.push(if edge { self.next() } else { 255 });
            }
        }
        out
    }
}

/// A page size and its frame rectangles (each frame is the size of its
/// rectangle, as a composited iframe is).
struct Scene {
    name: &'static str,
    page: (u32, u32),
    frames: &'static [(u32, u32, u32, u32)],
}

const SCENES: &[Scene] = &[
    // A 720p page with an ad-sized frame and a video-sized one.
    Scene {
        name: "frames_720p",
        page: (1280, 720),
        frames: &[(40, 40, 300, 250), (420, 120, 800, 450)],
    },
    // The same layout at device scale 2 (a 1440p backing store) with four
    // frames, two overlapping.
    Scene {
        name: "frames_1440p",
        page: (2560, 1440),
        frames: &[
            (80, 80, 600, 500),
            (840, 240, 1600, 900),
            (1200, 600, 800, 600),
            (100, 900, 640, 480),
        ],
    },
];

fn checksum(surface: &Surface) -> u64 {
    surface
        .premultiplied()
        .iter()
        .enumerate()
        .fold(0u64, |sum, (index, &byte)| {
            sum.wrapping_mul(31)
                .wrapping_add(u64::from(byte) ^ index as u64)
        })
}

fn millis(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn main() -> ExitCode {
    let mut runs = 11usize;
    let mut require = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--runs" => runs = args.next().and_then(|n| n.parse().ok()).unwrap_or(runs),
            // Accepted for the harness's common command line.
            "--size" => {
                args.next();
            }
            "--require" => require = true,
            other => {
                eprintln!("unknown argument {other}");
                return ExitCode::from(2);
            }
        }
    }
    let gpu = match Compositor::open(AdapterChoice::Any) {
        Ok(gpu) => {
            let info = gpu.info();
            println!(
                "adapter {} {} {}",
                info.backend,
                if info.software {
                    "software"
                } else {
                    "hardware"
                },
                info.name
            );
            Some(gpu)
        }
        Err(error) => {
            eprintln!("gpu not runnable: {error}");
            if require {
                return ExitCode::FAILURE;
            }
            None
        }
    };
    let mut bytes = Bytes(0x2545_F491_4F6C_DD1D);
    for scene in SCENES {
        let page = bytes.surface(scene.page.0, scene.page.1);
        let frames: Vec<Surface> = scene
            .frames
            .iter()
            .map(|&(_, _, w, h)| bytes.surface(w, h))
            .collect();
        let coverages: Vec<Vec<u8>> = scene
            .frames
            .iter()
            .map(|&(_, _, w, h)| bytes.coverage(w, h))
            .collect();
        let layers: Vec<Layer<'_>> = scene
            .frames
            .iter()
            .enumerate()
            .map(|(i, &rect)| Layer {
                frame: &frames[i],
                rect,
                coverage: &coverages[i],
            })
            .collect();

        let mut cpu_samples = Vec::with_capacity(runs);
        let mut expected = 0;
        for _ in 0..runs {
            let start = Instant::now();
            let mut out = page.clone();
            for layer in &layers {
                out.add_frame(layer.frame, layer.rect, layer.coverage)
                    .expect("scene layers fit the page");
            }
            cpu_samples.push(millis(start));
            expected = checksum(&out);
        }
        for ms in cpu_samples {
            println!("sample {}.cpu {ms:.3} {expected}", scene.name);
        }

        let Some(gpu) = &gpu else {
            eprintln!("{}.gpu not runnable: no adapter", scene.name);
            continue;
        };
        // One untimed frame compiles the pipeline and warms the driver.
        if let Err(error) = gpu.composite(&page, &layers) {
            eprintln!("{}.gpu not runnable: {error}", scene.name);
            if require {
                return ExitCode::FAILURE;
            }
            continue;
        }
        let mut gpu_samples = Vec::with_capacity(runs);
        for _ in 0..runs {
            let start = Instant::now();
            let result = gpu.composite(&page, &layers);
            let ms = millis(start);
            match result {
                Ok(out) if checksum(&out) == expected => gpu_samples.push(ms),
                Ok(_) => {
                    eprintln!(
                        "{}.gpu not runnable: differs from the CPU reference",
                        scene.name
                    );
                    return ExitCode::FAILURE;
                }
                Err(error) => {
                    eprintln!("{}.gpu not runnable: {error}", scene.name);
                    return ExitCode::FAILURE;
                }
            }
        }
        for ms in gpu_samples {
            println!("sample {}.gpu {ms:.3} {expected}", scene.name);
        }
    }
    ExitCode::SUCCESS
}
