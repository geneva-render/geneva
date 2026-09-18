//! Finding a device and checking it can do what the compositor needs.

use thiserror::Error;

/// Which adapters may be picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preference {
    /// A hardware device only: discrete, integrated, or the virtual
    /// device a virtual machine passes through. This is what `auto`
    /// means, so a machine without a GPU never composites on a software
    /// implementation of one, which is slower than the CPU renderer.
    Hardware,
    /// Whatever there is, hardware first. A software implementation
    /// (Mesa's lavapipe, SwiftShader) counts, which is how the GPU path
    /// runs where there is no GPU: in tests, and on a machine asked for
    /// it outright.
    Any,
    /// A software implementation only, for checking the GPU path against
    /// the CPU renderer on a machine that has a GPU too.
    Software,
}

impl Preference {
    /// The preference for `auto`, read from `GENEVA_GPU`: `software`
    /// forces the software adapter and anything else leaves the default.
    pub fn from_env(default: Self) -> Self {
        match std::env::var("GENEVA_GPU").as_deref() {
            Ok("software") => Self::Software,
            _ => default,
        }
    }
}

/// What was found, for the render report and for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The adapter's name as the driver gives it.
    pub name: String,
    /// The API in use: Vulkan, Metal or DirectX 12.
    pub backend: String,
    /// The kind of device: discrete, integrated, virtual, software, or
    /// unknown.
    pub kind: String,
    /// The driver and its version, where the API says.
    pub driver: String,
    /// Whether the device is a software implementation.
    pub software: bool,
    /// Whether shaders may compute in 16-bit floats. The compositor does
    /// not need it (its arithmetic is 32-bit; only storage is 16-bit),
    /// so this is information, not a requirement.
    pub shader_f16: bool,
}

impl Report {
    /// One line for a report: the name, the API and the kind, as in
    /// `llvmpipe (LLVM 20.1.2, 256 bits) on Vulkan, software`.
    pub fn line(&self) -> String {
        format!("{} on {}, {}", self.name, self.backend, self.kind)
    }
}

/// Why no device could be used.
#[derive(Debug, Error)]
pub enum GpuError {
    /// No adapter matched the preference.
    #[error("no {wanted} GPU adapter was found")]
    NoAdapter {
        /// What was asked for, in words.
        wanted: &'static str,
    },
    /// The adapter cannot render the working format.
    #[error("{name} cannot {what} Rgba16Float textures, which the compositor draws into")]
    Format {
        /// The adapter's name.
        name: String,
        /// What it cannot do.
        what: &'static str,
    },
    /// The device could not be opened.
    #[error("the device {name} could not be opened: {reason}")]
    Device {
        /// The adapter's name.
        name: String,
        /// The driver's reason.
        reason: String,
    },
}

/// An open device, with the queue the compositor submits to. Cloning
/// shares the device: two renderers on one `Gpu` draw on the same
/// hardware, one after the other.
#[derive(Clone)]
pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    report: Report,
}

impl std::fmt::Debug for Gpu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gpu")
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

/// The working format: premultiplied linear light, 16-bit floats.
pub const WORKING_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

impl Gpu {
    /// Finds an adapter matching `preference`, checks it can render and
    /// blend the working format, and opens it. Adapters are ranked
    /// discrete, integrated, virtual, software, and the first that
    /// qualifies is taken.
    pub fn probe(preference: Preference) -> Result<Self, GpuError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
        adapters.sort_by_key(|a| rank(a.get_info().device_type));
        let wanted = match preference {
            Preference::Hardware => "hardware",
            Preference::Any => "usable",
            Preference::Software => "software",
        };
        let adapter = adapters
            .into_iter()
            .find(|a| {
                let kind = a.get_info().device_type;
                match preference {
                    Preference::Hardware => matches!(
                        kind,
                        wgpu::DeviceType::DiscreteGpu
                            | wgpu::DeviceType::IntegratedGpu
                            | wgpu::DeviceType::VirtualGpu
                    ),
                    Preference::Any => true,
                    Preference::Software => kind == wgpu::DeviceType::Cpu,
                }
            })
            .ok_or(GpuError::NoAdapter { wanted })?;
        let info = adapter.get_info();
        let features = adapter.get_texture_format_features(WORKING_FORMAT);
        let usages = wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC;
        if !features.allowed_usages.contains(usages) {
            return Err(GpuError::Format {
                name: info.name.clone(),
                what: "render into and read back",
            });
        }
        let flags = wgpu::TextureFormatFeatureFlags::BLENDABLE
            | wgpu::TextureFormatFeatureFlags::FILTERABLE;
        if !features.flags.contains(flags) {
            return Err(GpuError::Format {
                name: info.name.clone(),
                what: "blend and filter",
            });
        }
        let report = Report {
            name: info.name.clone(),
            backend: match info.backend {
                wgpu::Backend::Vulkan => "Vulkan",
                wgpu::Backend::Metal => "Metal",
                wgpu::Backend::Dx12 => "DirectX 12",
                wgpu::Backend::Gl => "OpenGL",
                wgpu::Backend::BrowserWebGpu => "WebGPU",
                wgpu::Backend::Noop => "no-op",
            }
            .to_owned(),
            kind: match info.device_type {
                wgpu::DeviceType::DiscreteGpu => "discrete",
                wgpu::DeviceType::IntegratedGpu => "integrated",
                wgpu::DeviceType::VirtualGpu => "virtual",
                wgpu::DeviceType::Cpu => "software",
                wgpu::DeviceType::Other => "unknown",
            }
            .to_owned(),
            driver: match (info.driver.is_empty(), info.driver_info.is_empty()) {
                (true, true) => String::new(),
                (false, true) => info.driver.clone(),
                (true, false) => info.driver_info.clone(),
                (false, false) => format!("{} {}", info.driver, info.driver_info),
            },
            software: info.device_type == wgpu::DeviceType::Cpu,
            shader_f16: adapter.features().contains(wgpu::Features::SHADER_F16),
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("geneva"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .map_err(|e| GpuError::Device {
            name: info.name.clone(),
            reason: e.to_string(),
        })?;
        Ok(Self {
            device,
            queue,
            report,
        })
    }

    /// What was found.
    pub fn report(&self) -> &Report {
        &self.report
    }

    /// The device.
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// The queue.
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }
}

/// Adapters in the order they are tried.
fn rank(kind: wgpu::DeviceType) -> u8 {
    match kind {
        wgpu::DeviceType::DiscreteGpu => 0,
        wgpu::DeviceType::IntegratedGpu => 1,
        wgpu::DeviceType::VirtualGpu => 2,
        wgpu::DeviceType::Cpu => 3,
        wgpu::DeviceType::Other => 4,
    }
}
