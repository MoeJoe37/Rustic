//! Rustic's hardware compositor (UI-05, `compositor.backend`; ADR 0071).
//!
//! The GPU process ([ADR 0068](../../../docs/adr/0068-gpu-process.md))
//! composites out-of-process frames into their parent pages. This crate
//! does that work on a GPU through wgpu: Vulkan (Linux, Windows, Android),
//! Metal (macOS, iOS) and Direct3D 12 (Windows). It runs the reference
//! compositor's integer arithmetic (`Surface::add_frame`) in a compute
//! shader, so its output is byte-identical to the CPU reference: the
//! parity gate compares exact bytes, not a tolerance.
//!
//! Nothing here holds authority. The caller passes surfaces and layers
//! the broker already checked; a failure of any kind (no adapter, an
//! allocation the device refuses, a lost device) is an error the caller
//! answers by compositing on the CPU, and a lost device is reopened for
//! the next frame.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rustic_raster::{RasterLimits, Surface};

/// One frame to composite: `coverage / 255` of `frame`'s pixel at each
/// offset is added inside the rectangle (`x, y, width, height`) of the
/// page, exactly as [`Surface::add_frame`] does.
#[derive(Clone, Copy, Debug)]
pub struct Layer<'a> {
    /// The frame's composited pixels.
    pub frame: &'a Surface,
    /// Left, top, width and height in page pixels.
    pub rect: (u32, u32, u32, u32),
    /// One weight per pixel of the rectangle, row by row.
    pub coverage: &'a [u8],
}

/// Which adapters may be used.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AdapterChoice {
    /// The best hardware adapter, else a software one (WARP, lavapipe,
    /// SwiftShader).
    #[default]
    Any,
    /// Hardware adapters only.
    HardwareOnly,
}

/// What the compositor runs on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterInfo {
    /// `vulkan`, `metal` or `dx12`.
    pub backend: &'static str,
    /// The adapter's name as the driver reports it.
    pub name: String,
    /// A CPU implementation of the API (WARP, lavapipe, SwiftShader).
    pub software: bool,
}

/// Why the GPU did not composite. Every case falls back to the CPU.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GpuError {
    /// No usable adapter (no driver, no device, or refused by the choice).
    NoAdapter(String),
    /// The adapter refused to create a device or a resource.
    Device(String),
    /// The device was lost (driver reset, removal, or a test); reopen.
    Lost,
    /// The surfaces exceed what the device can bind in one buffer.
    TooLarge,
    /// A layer does not fit the page or its coverage has the wrong length.
    Invalid,
}

impl std::fmt::Display for GpuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoAdapter(why) => write!(f, "no GPU adapter: {why}"),
            Self::Device(why) => write!(f, "the GPU refused: {why}"),
            Self::Lost => f.write_str("the GPU device was lost"),
            Self::TooLarge => f.write_str("the composition is larger than the GPU can bind"),
            Self::Invalid => f.write_str("a layer does not fit its page"),
        }
    }
}

impl std::error::Error for GpuError {}

/// The compute shader: [`Surface::add_frame`]'s arithmetic, one invocation
/// per pixel of a layer's rectangle. Pixels are RGBA bytes packed little
/// endian into `u32`s (red lowest).
const SHADER: &str = r"
struct Params {
    page_width: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    frame_width: u32,
    frame_height: u32,
    pad: u32,
}

@group(0) @binding(0) var<storage, read_write> page: array<u32>;
@group(0) @binding(1) var<storage, read> frame: array<u32>;
@group(0) @binding(2) var<storage, read> coverage: array<u32>;
@group(0) @binding(3) var<uniform> params: Params;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.width || id.y >= params.height) {
        return;
    }
    let index = id.y * params.width + id.x;
    let weight = (coverage[index / 4u] >> ((index % 4u) * 8u)) & 255u;
    if (weight == 0u) {
        return;
    }
    let row = min(id.y, params.frame_height - 1u);
    let column = min(id.x, params.frame_width - 1u);
    let source = frame[row * params.frame_width + column];
    let dest = (params.y + id.y) * params.page_width + params.x + id.x;
    let before = page[dest];
    let alpha = before >> 24u;
    var after = before & 0xff000000u;
    for (var channel = 0u; channel < 3u; channel = channel + 1u) {
        let shift = channel * 8u;
        let added = (weight * ((source >> shift) & 255u) + 127u) / 255u;
        after = after | (min(((before >> shift) & 255u) + added, alpha) << shift);
    }
    page[dest] = after;
}
";

/// A GPU compositor on one device.
pub struct Compositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    info: AdapterInfo,
    max_binding: u64,
    lost: Arc<AtomicBool>,
}

impl std::fmt::Debug for Compositor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Compositor")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

/// Wait for a wgpu future. Native wgpu completes adapter and device
/// requests before returning them, so the first poll is ready; a pending
/// one is polled again after yielding.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return value;
        }
        std::thread::yield_now();
    }
}

fn backend_name(backend: wgpu::Backend) -> &'static str {
    match backend {
        wgpu::Backend::Vulkan => "vulkan",
        wgpu::Backend::Metal => "metal",
        wgpu::Backend::Dx12 => "dx12",
        wgpu::Backend::Gl => "gl",
        wgpu::Backend::BrowserWebGpu => "webgpu",
        wgpu::Backend::Noop => "noop",
    }
}

/// Bytes padded to whole `u32`s. The shader reads them as little-endian
/// words (red lowest), whatever this machine's byte order, because GPU
/// storage is little endian and the bytes are copied unchanged.
fn pack(bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    out.resize(bytes.len().div_ceil(4) * 4, 0);
    out
}

impl Compositor {
    /// Open the best adapter `choice` allows on this platform's native
    /// APIs (Vulkan, Metal, Direct3D 12) and prepare the compositing
    /// pipeline.
    pub fn open(choice: AdapterChoice) -> Result<Self, GpuError> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::METAL | wgpu::Backends::DX12,
            flags: wgpu::InstanceFlags::empty(),
            ..wgpu::InstanceDescriptor::default()
        });
        let hardware = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }));
        let adapter = match (hardware, choice) {
            (Ok(adapter), _)
                if adapter.get_info().device_type != wgpu::DeviceType::Cpu
                    || choice == AdapterChoice::Any =>
            {
                adapter
            }
            (_, AdapterChoice::HardwareOnly) => {
                return Err(GpuError::NoAdapter("no hardware adapter".into()));
            }
            (Err(_), AdapterChoice::Any) => {
                block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    force_fallback_adapter: true,
                    compatible_surface: None,
                }))
                .map_err(|error| GpuError::NoAdapter(error.to_string()))?
            }
            (Ok(adapter), AdapterChoice::Any) => adapter,
        };
        let details = adapter.get_info();
        let info = AdapterInfo {
            backend: backend_name(details.backend),
            name: details.name.clone(),
            software: details.device_type == wgpu::DeviceType::Cpu,
        };
        let limits = adapter.limits();
        let max_binding =
            u64::from(limits.max_storage_buffer_binding_size).min(limits.max_buffer_size);
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("rustic compositor"),
            required_features: wgpu::Features::empty(),
            required_limits: limits,
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|error| GpuError::Device(error.to_string()))?;
        let lost = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&lost);
        device.set_device_lost_callback(move |_, _| flag.store(true, Ordering::SeqCst));
        // Errors are caught by error scopes and become `GpuError`s; one that
        // escapes a scope must not abort the process.
        device.on_uncaptured_error(Box::new(|_| {}));
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rustic add_frame"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let storage = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rustic add_frame"),
            entries: &[
                storage(0, false),
                storage(1, true),
                storage(2, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rustic add_frame"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("rustic add_frame"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        if let Some(error) = block_on(device.pop_error_scope()) {
            return Err(GpuError::Device(describe(&error)));
        }
        Ok(Self {
            device,
            queue,
            pipeline,
            layout,
            info,
            max_binding,
            lost,
        })
    }

    /// The adapter this compositor runs on.
    pub fn info(&self) -> &AdapterInfo {
        &self.info
    }

    /// Whether the device was lost (reopen with [`Self::open`]).
    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }

    /// Destroy the device as a driver reset would (recovery tests): every
    /// later composition fails with [`GpuError::Lost`].
    pub fn lose_device(&self) {
        self.device.destroy();
        self.lost.store(true, Ordering::SeqCst);
    }

    /// Composite `layers` into a copy of `page`, in order.
    pub fn composite(&self, page: &Surface, layers: &[Layer<'_>]) -> Result<Surface, GpuError> {
        if self.is_lost() {
            return Err(GpuError::Lost);
        }
        let fits = |start: u32, extent: u32, limit: u32| {
            start.checked_add(extent).is_some_and(|end| end <= limit)
        };
        for layer in layers {
            let (x, y, width, height) = layer.rect;
            let count = u64::from(width) * u64::from(height);
            if !fits(x, width, page.width())
                || !fits(y, height, page.height())
                || u64::try_from(layer.coverage.len()).ok() != Some(count)
            {
                return Err(GpuError::Invalid);
            }
        }
        let live: Vec<&Layer<'_>> = layers
            .iter()
            .filter(|layer| {
                layer.frame.width() > 0
                    && layer.frame.height() > 0
                    && layer.rect.2 > 0
                    && layer.rect.3 > 0
            })
            .collect();
        if live.is_empty() || page.width() == 0 || page.height() == 0 {
            return Ok(page.clone());
        }
        let page_bytes = page.premultiplied().len() as u64;
        if page_bytes > self.max_binding
            || live.iter().any(|layer| {
                layer.frame.premultiplied().len() as u64 > self.max_binding
                    || layer.coverage.len() as u64 > self.max_binding
            })
        {
            return Err(GpuError::TooLarge);
        }
        self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let result = self.run(page, &live);
        let validation = block_on(self.device.pop_error_scope());
        let memory = block_on(self.device.pop_error_scope());
        if self.is_lost() {
            return Err(GpuError::Lost);
        }
        if let Some(error) = validation.or(memory) {
            return Err(GpuError::Device(describe(&error)));
        }
        result
    }

    fn buffer(&self, label: &str, bytes: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (bytes.len() as u64).max(4),
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        if !bytes.is_empty() {
            self.queue.write_buffer(&buffer, 0, bytes);
        }
        buffer
    }

    fn run(&self, page: &Surface, layers: &[&Layer<'_>]) -> Result<Surface, GpuError> {
        let size = page.premultiplied().len() as u64;
        let target = self.buffer(
            "rustic page",
            page.premultiplied(),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustic readback"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("rustic composite"),
            });
        // Each layer gets its own inputs and dispatch; WebGPU orders the
        // dispatches of a pass, so later layers see earlier ones.
        let mut groups = Vec::with_capacity(layers.len());
        for layer in layers {
            let (x, y, width, height) = layer.rect;
            let frame = self.buffer(
                "rustic frame",
                &pack(layer.frame.premultiplied()),
                wgpu::BufferUsages::STORAGE,
            );
            let coverage = self.buffer(
                "rustic coverage",
                &pack(layer.coverage),
                wgpu::BufferUsages::STORAGE,
            );
            let params: Vec<u8> = [
                page.width(),
                x,
                y,
                width,
                height,
                layer.frame.width(),
                layer.frame.height(),
                0,
            ]
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
            let params = self.buffer("rustic params", &params, wgpu::BufferUsages::UNIFORM);
            let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("rustic add_frame"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: target.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: frame.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: coverage.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: params.as_entire_binding(),
                    },
                ],
            });
            groups.push((group, width.div_ceil(8), height.div_ceil(8)));
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("rustic composite"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            for (group, columns, rows) in &groups {
                pass.set_bind_group(0, group, &[]);
                pass.dispatch_workgroups(*columns, *rows, 1);
            }
        }
        encoder.copy_buffer_to_buffer(&target, 0, &readback, 0, size);
        self.queue.submit(Some(encoder.finish()));
        let slice = readback.slice(..);
        let mapped = Arc::new(std::sync::Mutex::new(None));
        let slot = Arc::clone(&mapped);
        slice.map_async(wgpu::MapMode::Read, move |result| {
            if let Ok(mut slot) = slot.lock() {
                *slot = Some(result);
            }
        });
        self.device
            .poll(wgpu::PollType::Wait)
            .map_err(|_| GpuError::Lost)?;
        let status = mapped.lock().ok().and_then(|mut slot| slot.take());
        match status {
            Some(Ok(())) => {}
            _ if self.is_lost() => return Err(GpuError::Lost),
            Some(Err(error)) => return Err(GpuError::Device(error.to_string())),
            None => return Err(GpuError::Device("the readback never completed".into())),
        }
        let pixels = slice.get_mapped_range().to_vec();
        readback.unmap();
        Surface::from_premultiplied(page.width(), page.height(), pixels, RasterLimits::default())
            .ok_or_else(|| {
                GpuError::Device("the device returned pixels that are not premultiplied".into())
            })
    }
}

/// A wgpu error with its cause: `Display` alone prints only the error's
/// kind ("Validation Error"), which says nothing about what was refused.
fn describe(error: &wgpu::Error) -> String {
    let text = match error {
        wgpu::Error::Validation { description, .. }
        | wgpu::Error::Internal { description, .. } => description.trim().to_owned(),
        other => other.to_string(),
    };
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
