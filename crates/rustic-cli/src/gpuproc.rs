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
        crate::broker::spawn_bubblewrap(exe, GPU_COMMAND, false)?;
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

/// Composite one command; `None` when its input is invalid.
fn serve(composite: &Composite) -> Option<Surface> {
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
    composite_cpu(&page, &composite.page.layers, &frames)
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
        let reply = match serve(&composite) {
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
        assert_eq!(serve(&command).unwrap(), expected);
        // A handle without a frame, a layer outside the page, pixels that
        // are not premultiplied: refused, never a panic.
        command.page.layers[0].handle = 1;
        assert!(serve(&command).is_none());
        command.page.layers[0].handle = 0;
        command.page.layers[0].x = 3;
        assert!(serve(&command).is_none());
        command.page.layers[0].x = 2;
        command.frames[0].pixels = [255, 255, 255, 0].repeat(4);
        assert!(serve(&command).is_none());
    }
}
