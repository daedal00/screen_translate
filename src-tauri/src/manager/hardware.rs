//! Hardware detection and the rules that follow from it: how much of a model fits, which llama.cpp backend to use.

use super::runtime::{Backend, CudaVer};

/// Context and KV-cache bytes on top of a model's file size.
pub const OVERHEAD: u64 = 1_500_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Vendor {
    Nvidia,
    Amd,
    Intel,
    Apple,
    Other,
}

/// PCI vendor id (`DXGI_ADAPTER_DESC1::VendorId`) to vendor.
pub fn vendor_from_id(id: u32) -> Vendor {
    match id {
        0x10DE => Vendor::Nvidia,
        0x1002 => Vendor::Amd,
        0x8086 => Vendor::Intel,
        _ => Vendor::Other,
    }
}

/// One DXGI adapter.
#[derive(Debug, Clone)]
pub struct Adapter {
    pub name: String,
    pub vendor_id: u32,
    pub dedicated_vram: u64,
    /// User-mode driver version, e.g. "32.0.15.6094".
    pub driver: Option<String>,
}

/// The adapter with the most dedicated VRAM, ignoring Microsoft's software adapters.
pub fn pick_adapter(adapters: &[Adapter]) -> Option<&Adapter> {
    adapters
        .iter()
        .filter(|a| a.vendor_id != 0x1414 && !a.name.contains("Microsoft Basic Render"))
        .reduce(|best, a| if a.dedicated_vram > best.dedicated_vram { a } else { best })
}

/// Parses `nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv,noheader,nounits`
/// into (name, bytes, driver) of the line with the most memory.
pub fn parse_nvidia_smi_query(csv: &str) -> Option<(String, u64, String)> {
    csv.lines()
        .filter_map(|line| {
            let mut f = line.rsplitn(3, ',').map(str::trim);
            let (driver, mib, name) = (f.next()?, f.next()?, f.next()?);
            let bytes = mib.parse::<u64>().ok()?.checked_mul(1024 * 1024)?;
            (!name.is_empty() && !driver.is_empty()).then(|| (name.to_string(), bytes, driver.to_string()))
        })
        .max_by_key(|g| g.1)
}

/// The "CUDA Version: 12.6" of the plain `nvidia-smi` banner.
pub fn parse_cuda_header(nvidia_smi: &str) -> Option<CudaVer> {
    let rest = nvidia_smi.split_once("CUDA Version:")?.1;
    let (major, minor) = rest.split_whitespace().next()?.split_once('.')?;
    Some(CudaVer { major: major.parse().ok()?, minor: minor.parse().ok()? })
}

/// NVIDIA driver version from a DXGI user-mode driver version: "32.0.15.6094" -> "560.94"
/// (the last five digits of the last two groups).
pub fn nvidia_driver_from_dxgi(umd: &str) -> Option<String> {
    let groups: Vec<&str> = umd.split('.').collect();
    if groups.len() != 4 || !groups.iter().all(|g| !g.is_empty() && g.bytes().all(|c| c.is_ascii_digit())) {
        return None;
    }
    let digits = format!("{}{}", groups[2], groups[3]);
    let last5 = digits.get(digits.len().checked_sub(5)?..)?;
    Some(format!("{}.{}", &last5[..3], &last5[3..]))
}

/// Newest CUDA level a Windows NVIDIA driver supports (NVIDIA's minimum driver per toolkit release);
/// None for older or unparsable versions. Versions compare numerically, so "551.6" is older than "551.61".
pub fn max_cuda_for_driver(driver: &str) -> Option<CudaVer> {
    let (major, minor) = driver.trim().split_once('.').unwrap_or((driver.trim(), "0"));
    let v = (major.parse::<u32>().ok()?, minor.parse::<u32>().ok()?);
    let cuda = |major, minor| Some(CudaVer { major, minor });
    match v {
        v if v >= (580, 0) => cuda(13, 0),
        v if v >= (570, 0) => cuda(12, 8),
        v if v >= (560, 0) => cuda(12, 6),
        v if v >= (551, 61) => cuda(12, 4),
        v if v >= (528, 33) => cuda(12, 0),
        _ => None,
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Gpu {
    pub name: String,
    pub vendor: Vendor,
    /// Dedicated video memory in bytes (all of RAM on Apple silicon).
    pub vram: u64,
    pub driver: Option<String>,
    pub max_cuda: Option<CudaVer>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Hardware {
    pub gpu: Option<Gpu>,
    /// System RAM in bytes.
    pub ram: u64,
    /// CPU and GPU share one memory pool (Apple silicon).
    pub unified: bool,
}

/// Apple silicon from `sysctl -n machdep.cpu.brand_string` and `sysctl -n hw.memsize`.
pub fn mac_hardware(brand: &str, memsize: &str) -> Hardware {
    let Ok(ram) = memsize.trim().parse::<u64>() else {
        return Hardware { gpu: None, ram: 0, unified: true };
    };
    let gpu = Gpu { name: brand.trim().to_string(), vendor: Vendor::Apple, vram: ram, driver: None, max_cuda: None };
    Hardware { gpu: Some(gpu), ram, unified: true }
}

/// Bytes available to a model and its overhead.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct Budget {
    /// Fully on the GPU.
    pub gpu: u64,
    /// Across GPU and RAM.
    pub total: u64,
}

/// Unified memory: 65% of RAM. Discrete GPU: its VRAM, plus half of RAM overall. No GPU: half of RAM.
pub fn budget(hw: &Hardware) -> Budget {
    if hw.unified {
        let b = hw.ram * 65 / 100;
        return Budget { gpu: b, total: b };
    }
    let gpu = hw.gpu.as_ref().map_or(0, |g| g.vram);
    Budget { gpu, total: gpu + hw.ram / 2 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    Full,
    Partial,
    TooBig,
}

/// `required` is the file size plus `OVERHEAD`. Full needs a GPU.
pub fn fit(required: u64, b: &Budget) -> Fit {
    if b.gpu > 0 && required <= b.gpu {
        Fit::Full
    } else if required <= b.total {
        Fit::Partial
    } else {
        Fit::TooBig
    }
}

/// CUDA for NVIDIA with a driver new enough for a CUDA build, Vulkan for other discrete GPUs, else CPU
/// (macOS ignores the backend: Metal is built in).
pub fn choose_backend(gpu: Option<&Gpu>) -> Backend {
    match gpu {
        Some(Gpu { vendor: Vendor::Nvidia, max_cuda: Some(_), .. }) => Backend::Cuda,
        Some(Gpu { vendor: Vendor::Nvidia | Vendor::Amd | Vendor::Intel, .. }) => Backend::Vulkan,
        _ => Backend::Cpu,
    }
}

/// Detects this machine's hardware (spawns a few short commands, so cache the result).
pub fn detect() -> Hardware {
    #[cfg(target_os = "macos")]
    {
        let sysctl = |key: &str| {
            std::process::Command::new("/usr/sbin/sysctl")
                .args(["-n", key])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default()
        };
        mac_hardware(&sysctl("machdep.cpu.brand_string"), &sysctl("hw.memsize"))
    }
    #[cfg(windows)]
    {
        windows_hardware()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Hardware { gpu: None, ram: 0, unified: false }
    }
}

/// Runs `cmd` (no console window) and returns its stdout if it exits successfully within `timeout`.
#[cfg(windows)]
fn run_with_timeout(mut cmd: std::process::Command, timeout: std::time::Duration) -> Option<String> {
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x08000000).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    let mut child = cmd.spawn().ok()?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait().ok()? {
            Some(status) if status.success() => break,
            Some(_) => return None,
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    Some(out)
}

/// DXGI adapters plus RAM; for an NVIDIA adapter, `nvidia-smi` (when installed) refines name, memory, driver and CUDA level.
#[cfg(windows)]
fn windows_hardware() -> Hardware {
    use std::process::Command;
    use std::time::Duration;
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIDevice, IDXGIFactory1};
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows::core::Interface;

    let mut mem = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    let ram = if unsafe { GlobalMemoryStatusEx(&mut mem) }.is_ok() { mem.ullTotalPhys } else { 0 };

    let mut adapters = Vec::new();
    if let Ok(factory) = unsafe { CreateDXGIFactory1::<IDXGIFactory1>() } {
        let mut i = 0;
        while let Ok(adapter) = unsafe { factory.EnumAdapters1(i) } {
            i += 1;
            let Ok(desc) = (unsafe { adapter.GetDesc1() }) else { continue };
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            let len = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
            let driver = unsafe { adapter.CheckInterfaceSupport(&IDXGIDevice::IID) }.ok().map(|v| {
                let v = v as u64;
                format!("{}.{}.{}.{}", v >> 48, (v >> 32) & 0xffff, (v >> 16) & 0xffff, v & 0xffff)
            });
            adapters.push(Adapter {
                name: String::from_utf16_lossy(&desc.Description[..len]),
                vendor_id: desc.VendorId,
                dedicated_vram: desc.DedicatedVideoMemory as u64,
                driver,
            });
        }
    }
    let Some(adapter) = pick_adapter(&adapters) else { return Hardware { gpu: None, ram, unified: false } };
    let vendor = vendor_from_id(adapter.vendor_id);
    let mut gpu = Gpu { name: adapter.name.clone(), vendor, vram: adapter.dedicated_vram, driver: adapter.driver.clone(), max_cuda: None };
    if vendor == Vendor::Nvidia {
        gpu.driver = adapter.driver.as_deref().and_then(nvidia_driver_from_dxgi);
        let mut query = Command::new("nvidia-smi");
        query.args(["--query-gpu=name,memory.total,driver_version", "--format=csv,noheader,nounits"]);
        if let Some((name, vram, driver)) = run_with_timeout(query, Duration::from_secs(5)).as_deref().and_then(parse_nvidia_smi_query) {
            (gpu.name, gpu.vram, gpu.driver) = (name, vram, Some(driver));
        }
        gpu.max_cuda = run_with_timeout(Command::new("nvidia-smi"), Duration::from_secs(5))
            .as_deref()
            .and_then(parse_cuda_header)
            .or_else(|| gpu.driver.as_deref().and_then(max_cuda_for_driver));
    }
    Hardware { gpu: Some(gpu), ram, unified: false }
}
