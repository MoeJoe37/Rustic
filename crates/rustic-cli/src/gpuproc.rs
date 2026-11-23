//! The GPU process (M8, UI-05; ADR 0068).
//!
//! In a split run the broker no longer composites out-of-process frames
//! into their parent pages itself: it validates the renderer's surface,
//! resolves the frame handles from its own table, checks every layer's
//! geometry, and hands the work to a `rustic __gpu` child. The child holds
//! no authority (it is confined like a renderer) and keeps nothing
//! between commands, so a restarted one has nothing stale to reuse.
//!
//! A GPU process that crashes, hangs or breaks the protocol is killed; the
//! frame it was compositing is composited here on the CPU with the same
//! reference code, so the page does not change, and the next frame starts
//! a fresh process. After [`MAX_GPU_STARTS`] starts, or when one cannot
//! start, the run composites on the CPU (software fallback).

use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use rustic_base::EscapedText;
use rustic_ipc::gpu::{Composite, GpuCommand, GpuEvent, Image};
use rustic_ipc::protocol::{FrameLayer, Surface as WireSurface};
use rustic_ipc::{Channel, IpcError, Limits, Role};
use rustic_raster::{RasterLimits, Surface};

/// The hidden subcommand that runs the GPU process.
pub const GPU_COMMAND: &str = "__gpu";

/// GPU processes started per run, the first included.
pub const MAX_GPU_STARTS: u32 = 4;

/// Largest frame on the GPU channel. A composition that would not fit is
/// composited on the CPU instead (the process is not at fault).
const GPU_FRAME_BYTES: usize = 256 * 1024 * 1024;

fn limits() -> Limits {
    Limits {
        max_frame_bytes: GPU_FRAME_BYTES,
    }
}

/// The reference compositor, used by the GPU process and by the broker's
/// software fallback alike. `None` when a layer does not fit or names a
/// frame that does not exist.
fn composite_cpu(page: &Surface, layers: &[FrameLayer], frames: &[&Surface]) -> Option<Surface> {
    let mut out = page.clone();
    for layer in layers {
        let frame = frames.get(usize::try_from(layer.handle).ok()?)?;
        out.add_frame(
            frame,
            (layer.x, layer.y, layer.width, layer.height),
            &layer.coverage,
        )
        .ok()?;
    }
    Some(out)
}

// ---------------------------------------------------------------------
// Broker side
// ---------------------------------------------------------------------

type Link = Channel<Box<dyn Read + Send>, Box<dyn Write + Send>>;

/// A running GPU process.
struct Running {
    process: crate::broker::Process,
    channel: Link,
    next_id: u64,
}

impl Running {
    fn stop(mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

struct State {
    running: Option<Running>,
    starts: u32,
    fallback_noted: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    running: None,
    starts: 0,
    fallback_noted: false,
});

/// Whether this build and run use a GPU process at all: split runs on
/// desktop systems (Android and iOS hosts come with M14, ADR 0067).
fn wanted() -> bool {
    !cfg!(any(target_os = "android", target_os = "ios"))
}

/// Composite `layers` (already checked by the broker: each handle names
/// `frames[handle]` and fits the page) into `page`, in the GPU process
/// when one is available, otherwise here on the CPU. `None` only when the
/// layers do not fit, which the broker treats as a renderer's protocol
/// violation.
pub(crate) fn composite(
    page: &Surface,
    layers: &[FrameLayer],
    frames: &[&Surface],
) -> Option<Surface> {
    if layers.is_empty() {
        return Some(page.clone());
    }
    if wanted() {
        let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(surface) = through_process(&mut state, page, layers, frames) {
            return Some(surface);
        }
        if !state.fallback_noted && state.running.is_none() && state.starts >= MAX_GPU_STARTS {
            state.fallback_noted = true;
            eprintln!(
                "rustic: the GPU process keeps failing; compositing in the browser process for the rest of this run"
            );
        }
    }
    composite_cpu(page, layers, frames)
}

fn image(surface: &Surface) -> Image {
    Image {
        width: surface.width(),
        height: surface.height(),
        pixels: surface.premultiplied().to_vec(),
    }
}

/// Try one composition in the GPU process; `None` means "do it on the
/// CPU" (no process, too large, or the process failed and was stopped).
fn through_process(
    state: &mut State,
    page: &Surface,
    layers: &[FrameLayer],
    frames: &[&Surface],
) -> Option<Surface> {
    let bytes = frames
        .iter()
        .map(|frame| frame.premultiplied().len())
        .chain(std::iter::once(page.premultiplied().len()))
        .chain(layers.iter().map(|layer| layer.coverage.len()))
        .try_fold(0usize, usize::checked_add)?;
    if bytes > GPU_FRAME_BYTES / 2 {
        return None;
    }
    if state.running.is_none() {
        if state.starts >= MAX_GPU_STARTS {
            return None;
        }
        state.starts += 1;
        match start() {
            Ok(running) => state.running = Some(running),
            Err(error) => {
                eprintln!("rustic: GPU process: {error}");
                // Starting failed (no sandbox here, for one): do not retry.
                state.starts = MAX_GPU_STARTS;
                return None;
            }
        }
    }
    let running = state.running.as_mut()?;
    let id = running.next_id;
    running.next_id += 1;
    let command = GpuCommand::Composite(Box::new(Composite {
        id,
        page: WireSurface {
            width: page.width(),
            height: page.height(),
            pixels: page.premultiplied().to_vec(),
            layers: layers.to_vec(),
        },
        frames: frames.iter().map(|frame| image(frame)).collect(),
    }));
    // The parent's renderer waits for its frames' compositions, so a hung
    // GPU process must be stopped well within the renderer's own timeout:
    // an eighth of it, between 1 and 15 seconds.
    let timeout = (crate::broker::renderer_timeout() / 8)
        .clamp(Duration::from_secs(1), Duration::from_secs(15));
    let kill = running.process.killer();
    let (done, watch) = std::sync::mpsc::channel::<()>();
    let watchdog = std::thread::spawn(move || {
        if watch.recv_timeout(timeout) == Err(RecvTimeoutError::Timeout) {
            kill();
        }
    });
    let reply = running
        .channel
        .send(&command)
        .and_then(|()| running.channel.recv::<GpuEvent>());
    let _ = done.send(());
    let _ = watchdog.join();
    let problem = match reply {
        Ok(GpuEvent::Composited {
            id: got,
            width,
            height,
            pixels,
        }) if got == id && (width, height) == (page.width(), page.height()) => {
            match Surface::from_premultiplied(width, height, pixels, RasterLimits::default()) {
                Some(surface) => return Some(surface),
                None => "the GPU process sent pixels that are not premultiplied".to_owned(),
            }
        }
        Ok(GpuEvent::Composited { .. }) => {
            "the GPU process answered another request or changed the page's size".to_owned()
        }
        Ok(GpuEvent::Failed { .. }) => "the GPU process refused a valid composition".to_owned(),
        Err(IpcError::Closed) => "the GPU process stopped".to_owned(),
        Err(error) => format!("the GPU process broke the protocol: {error}"),
    };
    eprintln!("rustic: {problem}; this frame is composited in the browser process");
    if let Some(running) = state.running.take() {
        running.stop();
    }
    None
}

type Started = (
    crate::broker::Process,
    Box<dyn Read + Send>,
    Box<dyn Write + Send>,
    Box<dyn Read + Send>,
);

fn spawn_plain(exe: &std::path::Path) -> io::Result<Started> {
    let mut child = Command::new(exe)
        .arg(GPU_COMMAND)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    Ok((
        crate::broker::Process::Child(child),
        Box::new(stdout),
        Box::new(stdin),
        Box::new(stderr),
    ))
}

/// Linux: bubblewrap with every namespace unshared and the renderer
/// seccomp filter, as for a renderer (ADR 0058).
#[cfg(not(any(windows, target_os = "macos")))]
fn spawn_confined(exe: &std::path::Path) -> io::Result<Started> {
    let (child, filter, stdout, stdin, stderr) =
        crate::broker::spawn_bubblewrap(exe, GPU_COMMAND, crate::broker::Confinement::Gpu)?;
    Ok((
        crate::broker::Process::Sandboxed {
            child,
            _filter: filter,
        },
        stdout,
        stdin,
        Box::new(stderr),
    ))
}

/// macOS: the renderer's Seatbelt profile, applied by the child to itself
/// before it reads from the broker (ADR 0063).
#[cfg(target_os = "macos")]
fn spawn_confined(exe: &std::path::Path) -> io::Result<Started> {
    let mut child = Command::new(exe)
        .arg(GPU_COMMAND)
        .env_clear()
        .envs(crate::broker::sandbox_env())
        .env(crate::broker::SEATBELT_VARIABLE, "gpu")
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    Ok((
        crate::broker::Process::Child(child),
        Box::new(stdout),
        Box::new(stdin),
        Box::new(stderr),
    ))
}

/// Windows: the capability-less `Rustic.Renderer` AppContainer in a
/// kill-on-close job (ADR 0060).
#[cfg(windows)]
fn spawn_confined(exe: &std::path::Path) -> io::Result<Started> {
    let command = std::ffi::OsStr::new(GPU_COMMAND);
    let (confined, pipes) = rustic_sandbox::spawn_app_container(
        rustic_sandbox::Container::Renderer,
        exe,
        &[command],
        &crate::broker::sandbox_env(),
    )?;
    Ok((
        crate::broker::Process::Confined(confined),
        Box::new(pipes.stdout),
        Box::new(pipes.stdin),
        Box::new(pipes.stderr),
    ))
}

fn start() -> io::Result<Running> {
    let exe = std::env::current_exe()?;
    let (mut process, stdout, stdin, stderr) = if crate::broker::sandboxed() {
        spawn_confined(&exe)?
    } else {
        spawn_plain(&exe)?
    };
    // Relay the child's diagnostics escaped: it is untrusted.
    std::thread::Builder::new()
        .name("rustic gpu stderr".into())
        .spawn(move || {
            for line in BufReader::new(stderr).split(b'\n').map_while(Result::ok) {
                let line = String::from_utf8_lossy(&line);
                eprintln!("{}", EscapedText(line.trim_end()));
            }
        })?;
    match Channel::connect_to(stdout, stdin, Role::Broker, Role::Gpu, limits()) {
        Ok(channel) => Ok(Running {
            process,
            channel,
            next_id: 1,
        }),
        Err(error) => {
            let _ = process.kill();
            let _ = process.wait();
            Err(io::Error::other(error.to_string()))
        }
    }
}

// ---------------------------------------------------------------------
// GPU process side
// ---------------------------------------------------------------------

/// Device opens one GPU process makes at most (the first included); after
/// that it composites on the CPU (`compositor.recovery`, device loss).
pub const MAX_DEVICE_OPENS: u32 = 3;

/// The GPU process's hardware compositor (ADR 0071): opened on first use,
/// reopened after a lost device, given up after [`MAX_DEVICE_OPENS`].
#[derive(Default)]
struct Device {
    compositor: Option<rustic_gpu::Compositor>,
    opens: u32,
    /// No device will be used: `RUSTIC_GPU=cpu`, no adapter, or too many
    /// losses.
    disabled: bool,
    /// Compositions so far (test hooks count them).
    count: usize,
}

/// Which adapters `RUSTIC_GPU` allows: `cpu` none, `any` software ones
/// too (WARP, lavapipe: CI runners without GPUs), otherwise hardware.
fn adapter_choice() -> Option<rustic_gpu::AdapterChoice> {
    match std::env::var("RUSTIC_GPU").as_deref() {
        Ok("cpu") => None,
        Ok("any") => Some(rustic_gpu::AdapterChoice::Any),
        _ => Some(rustic_gpu::AdapterChoice::HardwareOnly),
    }
}

impl Device {
    /// Composite on the GPU, or `None` to composite this one on the CPU.
    fn composite(
        &mut self,
        page: &Surface,
        layers: &[FrameLayer],
        frames: &[&Surface],
        test: Option<&str>,
    ) -> Option<Surface> {
        self.count += 1;
        let report = test.is_some_and(|mode| mode.split(',').any(|part| part == "report"));
        let lose_at = test.and_then(|mode| {
            mode.split(',')
                .find_map(|part| part.strip_prefix("lose-on:"))
                .and_then(|n| n.parse::<usize>().ok())
        });
        let layers: Vec<rustic_gpu::Layer<'_>> = layers
            .iter()
            .map(|layer| {
                Some(rustic_gpu::Layer {
                    frame: frames.get(usize::try_from(layer.handle).ok()?)?,
                    rect: (layer.x, layer.y, layer.width, layer.height),
                    coverage: &layer.coverage,
                })
            })
            .collect::<Option<_>>()?;
        // A lost device is reopened once per composition.
        let lose = lose_at == Some(self.count);
        for attempt in 0..2 {
            let compositor = self.open(report)?;
            if lose && attempt == 0 {
                compositor.lose_device();
            }
            match compositor.composite(page, &layers) {
                Ok(surface) => {
                    if report {
                        eprintln!(
                            "rustic GPU process: composited on {} ({})",
                            compositor.info().backend,
                            compositor.info().name
                        );
                    }
                    return Some(surface);
                }
                Err(rustic_gpu::GpuError::Lost) => {
                    self.compositor = None;
                    eprintln!("rustic GPU process: the GPU device was lost; reopening it");
                }
                Err(error) => {
                    if report {
                        eprintln!("rustic GPU process: {error}; this frame composites on the CPU");
                    }
                    return None;
                }
            }
        }
        None
    }

    fn open(&mut self, report: bool) -> Option<&rustic_gpu::Compositor> {
        if self.disabled {
            return None;
        }
        if self.compositor.is_none() {
            let Some(choice) = adapter_choice() else {
                self.disabled = true;
                return None;
            };
            if self.opens >= MAX_DEVICE_OPENS {
                self.disabled = true;
                eprintln!(
                    "rustic GPU process: the GPU device keeps failing; compositing on the CPU"
                );
                return None;
            }
            self.opens += 1;
            match rustic_gpu::Compositor::open(choice) {
                Ok(compositor) => self.compositor = Some(compositor),
                Err(error) => {
                    // No adapter is not a failure: the CPU composites.
                    self.disabled = true;
                    if report {
                        eprintln!("rustic GPU process: {error}; compositing on the CPU");
                    }
                    return None;
                }
            }
        }
        self.compositor.as_ref()
    }
}

/// Composite one command; `None` when its input is invalid.
fn serve(composite: &Composite, device: &mut Device, test: Option<&str>) -> Option<Surface> {
    let page = Surface::from_premultiplied(
        composite.page.width,
        composite.page.height,
        composite.page.pixels.clone(),
        RasterLimits::default(),
    )?;
    let frames = composite
        .frames
        .iter()
        .map(|frame| {
            Surface::from_premultiplied(
                frame.width,
                frame.height,
                frame.pixels.clone(),
                RasterLimits::default(),
            )
        })
        .collect::<Option<Vec<_>>>()?;
    let frames: Vec<&Surface> = frames.iter().collect();
    // The device checks layers as the CPU path does; whatever it refuses
    // or cannot do composites on the CPU, which refuses invalid input.
    device
        .composite(&page, &composite.page.layers, &frames, test)
        .or_else(|| composite_cpu(&page, &composite.page.layers, &frames))
}

/// Misbehave as a test asks (`RUSTIC_TEST_GPU_PROCESS`); `Some` replaces
/// the reply, `None` answers normally.
fn misbehave(mode: &str, composite: &Composite, good: &GpuEvent) -> Option<GpuEvent> {
    let crash_on = mode
        .strip_prefix("crash-on-layers:")
        .and_then(|count| count.parse::<usize>().ok());
    match mode {
        "crash" => panic!("simulated GPU process crash"),
        _ if crash_on == Some(composite.page.layers.len()) => {
            panic!("simulated GPU process crash")
        }
        "hang" => loop {
            std::thread::sleep(Duration::from_secs(3600));
        },
        "wrong-size" => Some(GpuEvent::Composited {
            id: composite.id,
            width: composite.page.width + 1,
            height: composite.page.height,
            pixels: vec![
                0;
                (composite.page.width as usize + 1) * composite.page.height as usize * 4
            ],
        }),
        "wrong-id" => match good {
            GpuEvent::Composited {
                width,
                height,
                pixels,
                ..
            } => Some(GpuEvent::Composited {
                id: composite.id + 1,
                width: *width,
                height: *height,
                pixels: pixels.clone(),
            }),
            GpuEvent::Failed { .. } => None,
        },
        "not-premultiplied" => Some(GpuEvent::Composited {
            id: composite.id,
            width: composite.page.width,
            height: composite.page.height,
            pixels: [255, 255, 255, 0].repeat(composite.page.pixels.len() / 4),
        }),
        "escape" => {
            eprintln!(
                "{}",
                crate::renderer::escape_report()
                    .trim_end()
                    .replace('\n', "; ")
            );
            None
        }
        _ => None,
    }
}

/// `rustic __gpu`: serve compositions from the broker over standard input
/// and output until it hangs up.
pub fn run() -> u8 {
    let stdin: Box<dyn Read + Send> = Box::new(io::stdin());
    let stdout: Box<dyn Write + Send> = Box::new(io::stdout());
    let mut channel = match Channel::connect(stdin, stdout, Role::Gpu, limits()) {
        Ok(channel) => channel,
        Err(error) => {
            eprintln!("rustic GPU process: {error}");
            return 1;
        }
    };
    let test_mode = std::env::var("RUSTIC_TEST_GPU_PROCESS").ok();
    let device_test = std::env::var("RUSTIC_TEST_GPU_DEVICE").ok();
    let mut device = Device::default();
    loop {
        let command = match channel.recv::<GpuCommand>() {
            Ok(command) => command,
            Err(IpcError::Closed) => return 0,
            Err(error) => {
                eprintln!("rustic GPU process: {error}");
                return 1;
            }
        };
        let GpuCommand::Composite(composite) = command;
        let reply = match serve(&composite, &mut device, device_test.as_deref()) {
            Some(surface) => GpuEvent::Composited {
                id: composite.id,
                width: surface.width(),
                height: surface.height(),
                pixels: surface.premultiplied().to_vec(),
            },
            None => GpuEvent::Failed { id: composite.id },
        };
        let reply = test_mode
            .as_deref()
            .and_then(|mode| misbehave(mode, &composite, &reply))
            .unwrap_or(reply);
        if channel.send(&reply).is_err() {
            return 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cpu() -> Device {
        Device {
            disabled: true,
            ..Device::default()
        }
    }

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Surface {
        Surface::from_premultiplied(
            width,
            height,
            rgba.repeat((width * height) as usize),
            RasterLimits::default(),
        )
        .unwrap()
    }

    #[test]
    fn the_process_side_matches_the_reference_compositor_and_refuses_bad_input() {
        let page = solid(4, 2, [0, 255, 0, 255]);
        let frame = solid(2, 2, [0, 0, 255, 255]);
        let layers = vec![FrameLayer {
            handle: 0,
            x: 2,
            y: 0,
            width: 2,
            height: 2,
            coverage: vec![255, 128, 255, 0],
        }];
        let expected = composite_cpu(&page, &layers, &[&frame]).unwrap();
        let mut command = Composite {
            id: 1,
            page: WireSurface {
                width: 4,
                height: 2,
                pixels: page.premultiplied().to_vec(),
                layers: layers.clone(),
            },
            frames: vec![image(&frame)],
        };
        assert_eq!(serve(&command, &mut cpu(), None).unwrap(), expected);
        // A handle without a frame, a layer outside the page, pixels that
        // are not premultiplied: refused, never a panic.
        command.page.layers[0].handle = 1;
        assert!(serve(&command, &mut cpu(), None).is_none());
        command.page.layers[0].handle = 0;
        command.page.layers[0].x = 3;
        assert!(serve(&command, &mut cpu(), None).is_none());
        command.page.layers[0].x = 2;
        command.frames[0].pixels = [255, 255, 255, 0].repeat(4);
        assert!(serve(&command, &mut cpu(), None).is_none());
    }
}
