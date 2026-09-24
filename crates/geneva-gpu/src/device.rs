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
    #[error("no {wanted} GPU adapter was found{hint}")]
    NoAdapter {
        /// What was asked for, in words.
        wanted: &'static str,
        /// What is missing, where that can be told, ready to append to
        /// the sentence above. Empty when there is nothing to add.
        hint: String,
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
            .ok_or_else(|| GpuError::NoAdapter {
                wanted,
                hint: hint().map_or_else(String::new, |h| format!(": {h}")),
            })?;
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

/// The directories a shared library could be in, for [`hint`].
#[cfg(target_os = "linux")]
const LIB_DIRS: &[&str] = &[
    "/lib/x86_64-linux-gnu",
    "/usr/lib/x86_64-linux-gnu",
    "/lib/aarch64-linux-gnu",
    "/usr/lib/aarch64-linux-gnu",
    "/lib64",
    "/usr/lib64",
    "/usr/lib",
];

/// The libraries NVIDIA's Vulkan driver needs before it will start, and
/// the packages that carry them on Debian and Ubuntu.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const NEEDED: &[(&str, &str)] = &[
    ("libX11.so.6", "libx11-6"),
    ("libXext.so.6", "libxext6"),
    ("libGLdispatch.so.0", "libglvnd0"),
    ("libEGL.so.1", "libegl1"),
    ("libGL.so.1", "libgl1"),
];

/// Why a machine that has a GPU may still offer no usable device, where
/// the filesystem says enough to tell.
///
/// The case this exists for is a container built for CUDA, which is what
/// a rented GPU usually is. `nvidia-smi` works, the compute libraries
/// are all present, and the graphics ones are not. Vulkan then reports
/// no device, or only a software one, and the renderer falls back to the
/// CPU on a machine someone is paying for by the hour. Nothing along
/// that path says why, so this does.
///
/// It only reports what it can check: that an NVIDIA GPU is present,
/// that the driver's Vulkan manifest is or is not installed, and which
/// libraries the driver needs are missing. It never claims to know that
/// installing them will be enough.
pub fn hint() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let nvidia = std::path::Path::new("/dev/nvidiactl").exists()
            || std::fs::read_dir("/proc/driver/nvidia/gpus").is_ok_and(|mut d| d.next().is_some());
        let icd = ["/usr/share/vulkan/icd.d", "/etc/vulkan/icd.d"]
            .iter()
            .filter_map(|d| std::fs::read_dir(d).ok())
            .flatten()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().contains("nvidia"));
        let missing: Vec<&str> = NEEDED
            .iter()
            .filter(|(lib, _)| {
                !LIB_DIRS
                    .iter()
                    .any(|d| std::path::Path::new(d).join(lib).exists())
            })
            .map(|(lib, _)| *lib)
            .collect();
        hint_from(nvidia, icd, &missing)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// The wording for [`hint`], kept apart from the filesystem so it can be
/// checked on a machine in any state.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn hint_from(nvidia: bool, icd: bool, missing: &[&str]) -> Option<String> {
    if !nvidia {
        return None;
    }
    if !icd {
        return Some(
            "an NVIDIA GPU is here but the driver's Vulkan manifest is not, so this container              was given the driver's compute half and not its graphics half. Start it with              NVIDIA_DRIVER_CAPABILITIES=all"
                .to_owned(),
        );
    }
    if missing.is_empty() {
        return Some(
            "an NVIDIA GPU is here and its Vulkan driver looks complete, so the reason is              something this cannot see. `vulkaninfo --summary` prints the loader's own account"
                .to_owned(),
        );
    }
    let packages: Vec<&str> = NEEDED
        .iter()
        .filter(|(lib, _)| missing.contains(lib))
        .map(|(_, pkg)| *pkg)
        .collect();
    Some(format!(
        "an NVIDIA GPU is here and its Vulkan driver is installed, but {} {} missing. The driver          is a GLVND vendor library and will not start without {}. On Debian or Ubuntu: apt-get          install {}",
        missing.join(", "),
        if missing.len() == 1 { "is" } else { "are" },
        if missing.len() == 1 { "it" } else { "them" },
        packages.join(" "),
    ))
}

#[cfg(test)]
mod hint_tests {
    use super::hint_from;

    /// A machine with no NVIDIA GPU gets no NVIDIA advice. There is
    /// nothing useful to say: it may have no GPU at all, or an AMD one,
    /// and this only knows about the one case.
    #[test]
    fn nothing_is_said_about_a_machine_with_no_nvidia_gpu() {
        assert_eq!(hint_from(false, false, &["libEGL.so.1"]), None);
    }

    /// The container was given the driver's compute half only, which is
    /// the default for the images a rented GPU comes with.
    #[test]
    fn a_missing_manifest_points_at_the_driver_capabilities() {
        let h = hint_from(true, false, &[]).expect("a hint");
        assert!(h.contains("NVIDIA_DRIVER_CAPABILITIES=all"), "{h}");
    }

    /// The driver is there and cannot start. This is the case that cost
    /// a morning: the libraries it needs are named, and so are the
    /// packages that carry them.
    #[test]
    fn missing_libraries_are_named_with_their_packages() {
        let h = hint_from(true, true, &["libGLdispatch.so.0", "libEGL.so.1"]).expect("a hint");
        assert!(
            h.contains("libGLdispatch.so.0, libEGL.so.1 are missing"),
            "{h}"
        );
        assert!(h.contains("libglvnd0 libegl1"), "{h}");
        assert!(
            !h.contains("libx11-6"),
            "only the packages for what is missing: {h}"
        );
    }

    /// One missing library reads as one, not as a list.
    #[test]
    fn one_missing_library_is_singular() {
        let h = hint_from(true, true, &["libEGL.so.1"]).expect("a hint");
        assert!(h.contains("libEGL.so.1 is missing"), "{h}");
        assert!(h.contains("without it"), "{h}");
    }

    /// Everything checkable is in place, so this says so rather than
    /// guessing, and points at the tool that does know.
    #[test]
    fn a_complete_install_admits_it_cannot_tell() {
        let h = hint_from(true, true, &[]).expect("a hint");
        assert!(h.contains("vulkaninfo"), "{h}");
    }
}
