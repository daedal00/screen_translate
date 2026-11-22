use screen_translate_lib::manager::hardware::{
    budget, choose_backend, fit, mac_hardware, max_cuda_for_driver, nvidia_driver_from_dxgi, parse_cuda_header,
    parse_nvidia_smi_query, pick_adapter, vendor_from_id, Adapter, Budget, Fit, Gpu, Hardware, Vendor, OVERHEAD,
};
use screen_translate_lib::manager::runtime::{Backend, CudaVer};
use serde_json::json;

const GB: u64 = 1024 * 1024 * 1024;

fn cv(major: u32, minor: u32) -> CudaVer {
    CudaVer { major, minor }
}

#[test]
fn vendor_ids() {
    assert_eq!(vendor_from_id(0x10DE), Vendor::Nvidia);
    assert_eq!(vendor_from_id(0x1002), Vendor::Amd);
    assert_eq!(vendor_from_id(0x8086), Vendor::Intel);
    assert_eq!(vendor_from_id(0x1414), Vendor::Other);
    assert_eq!(vendor_from_id(0), Vendor::Other);
}

#[test]
fn vendor_serializes_lowercase() {
    assert_eq!(serde_json::to_value(Vendor::Nvidia).unwrap(), json!("nvidia"));
    assert_eq!(serde_json::to_value(Vendor::Apple).unwrap(), json!("apple"));
}

fn ad(name: &str, vendor_id: u32, vram: u64) -> Adapter {
    Adapter { name: name.into(), vendor_id, dedicated_vram: vram, driver: None }
}

#[test]
fn pick_adapter_most_vram_wins() {
    let a = [ad("Intel UHD", 0x8086, GB), ad("RTX 3070", 0x10DE, 8 * GB), ad("RX 580", 0x1002, 4 * GB)];
    assert_eq!(pick_adapter(&a).unwrap().name, "RTX 3070");
}

#[test]
fn pick_adapter_skips_microsoft_vendor_and_basic_render() {
    let a = [ad("Whatever", 0x1414, 64 * GB), ad("Microsoft Basic Render Driver", 0x8086, 32 * GB), ad("Intel UHD", 0x8086, GB)];
    assert_eq!(pick_adapter(&a).unwrap().name, "Intel UHD");
}

#[test]
fn pick_adapter_none_when_empty_or_only_software() {
    assert!(pick_adapter(&[]).is_none());
    assert!(pick_adapter(&[ad("Microsoft Basic Render Driver", 0x1414, 0)]).is_none());
}

#[test]
fn nvidia_smi_query_single() {
    assert_eq!(
        parse_nvidia_smi_query("NVIDIA GeForce RTX 3070, 8192, 560.94\n"),
        Some(("NVIDIA GeForce RTX 3070".to_string(), 8192 * 1024 * 1024, "560.94".to_string()))
    );
}

#[test]
fn nvidia_smi_query_multiple_picks_most_memory() {
    let csv = "NVIDIA GeForce GTX 1050, 2048, 560.94\nNVIDIA GeForce RTX 4090, 24564, 560.94\nNVIDIA T400, 4096, 560.94\n";
    let got = parse_nvidia_smi_query(csv).unwrap();
    assert_eq!(got.0, "NVIDIA GeForce RTX 4090");
    assert_eq!(got.1, 24564 * 1024 * 1024);
}

#[test]
fn nvidia_smi_query_garbage_is_none() {
    for s in ["", "\n", "NVIDIA-SMI has failed because it couldn't communicate with the NVIDIA driver.", "a, b, c", "name, 12"] {
        assert_eq!(parse_nvidia_smi_query(s), None, "{s:?}");
    }
}

#[test]
fn cuda_header() {
    let banner = "| NVIDIA-SMI 560.94                 Driver Version: 560.94         CUDA Version: 12.6     |";
    assert_eq!(parse_cuda_header(banner), Some(cv(12, 6)));
    assert_eq!(parse_cuda_header("Driver Version: 580.10   CUDA Version: 13.0"), Some(cv(13, 0)));
    assert_eq!(parse_cuda_header("no banner here"), None);
    assert_eq!(parse_cuda_header(""), None);
}

#[test]
fn driver_from_dxgi() {
    assert_eq!(nvidia_driver_from_dxgi("32.0.15.6094").as_deref(), Some("560.94"));
    assert_eq!(nvidia_driver_from_dxgi("31.0.15.2802").as_deref(), Some("528.02"));
    for bad in ["", "garbage", "1.2.3", "32.0.15", "a.b.c.d"] {
        assert_eq!(nvidia_driver_from_dxgi(bad), None, "{bad:?}");
    }
}

#[test]
fn max_cuda_thresholds() {
    let cases = [
        ("580.00", Some(cv(13, 0))),
        ("600.5", Some(cv(13, 0))),
        ("1000.1", Some(cv(13, 0))),
        ("579.99", Some(cv(12, 8))),
        ("570.00", Some(cv(12, 8))),
        ("569.99", Some(cv(12, 6))),
        ("560.94", Some(cv(12, 6))),
        ("560.00", Some(cv(12, 6))),
        ("559.99", Some(cv(12, 4))),
        ("551.61", Some(cv(12, 4))),
        ("551.6", Some(cv(12, 0))),
        ("551.60", Some(cv(12, 0))),
        ("528.33", Some(cv(12, 0))),
        ("528.32", None),
        ("472.12", None),
        ("", None),
        ("abc", None),
    ];
    for (d, want) in cases {
        assert_eq!(max_cuda_for_driver(d), want, "{d}");
    }
}

fn gpu(vendor: Vendor, vram: u64, cuda: Option<CudaVer>) -> Gpu {
    Gpu { name: "g".into(), vendor, vram, driver: None, max_cuda: cuda }
}

#[test]
fn mac_hardware_parses() {
    let h = mac_hardware("Apple M1 Pro\n", "17179869184\n");
    assert_eq!(h.ram, 17179869184);
    assert!(h.unified);
    let g = h.gpu.unwrap();
    assert_eq!(g.name, "Apple M1 Pro");
    assert_eq!(g.vendor, Vendor::Apple);
    assert_eq!(g.vram, 17179869184);
}

#[test]
fn mac_hardware_unparsable_memsize() {
    let h = mac_hardware("Apple M1 Pro\n", "oops");
    assert_eq!(h.ram, 0);
    assert!(h.gpu.is_none());
}

#[test]
fn hardware_serializes() {
    let v = serde_json::to_value(Hardware { gpu: Some(gpu(Vendor::Nvidia, 8 * GB, Some(cv(12, 6)))), ram: 16 * GB, unified: false }).unwrap();
    assert_eq!(v["gpu"]["vendor"], "nvidia");
    assert_eq!(v["gpu"]["max_cuda"], json!({ "major": 12, "minor": 6 }));
    assert_eq!(v["unified"], false);
}

#[test]
fn budget_unified_is_65_percent_of_ram_floored() {
    let h = Hardware { gpu: Some(gpu(Vendor::Apple, 16 * GB, None)), ram: 16 * GB, unified: true };
    let b = budget(&h);
    assert_eq!(b.gpu, 16 * GB * 65 / 100);
    assert_eq!(b.total, b.gpu);
    let odd = Hardware { gpu: None, ram: 101, unified: true };
    assert_eq!(budget(&odd).gpu, 65);
}

#[test]
fn budget_discrete() {
    let h = Hardware { gpu: Some(gpu(Vendor::Nvidia, 8 * GB, None)), ram: 16 * GB, unified: false };
    let b = budget(&h);
    assert_eq!(b.gpu, 8 * GB);
    assert_eq!(b.total, 8 * GB + 8 * GB);
}

#[test]
fn budget_no_gpu() {
    let b = budget(&Hardware { gpu: None, ram: 16 * GB, unified: false });
    assert_eq!((b.gpu, b.total), (0, 8 * GB));
}

#[test]
fn overhead_const() {
    assert_eq!(OVERHEAD, 1_500_000_000);
}

#[test]
fn fit_boundaries() {
    let b = Budget { gpu: 1000, total: 2000 };
    assert_eq!(fit(999, &b), Fit::Full);
    assert_eq!(fit(1000, &b), Fit::Full);
    assert_eq!(fit(1001, &b), Fit::Partial);
    assert_eq!(fit(2000, &b), Fit::Partial);
    assert_eq!(fit(2001, &b), Fit::TooBig);
}

#[test]
fn fit_without_gpu_is_never_full() {
    let b = Budget { gpu: 0, total: 2000 };
    assert_eq!(fit(0, &b), Fit::Partial);
    assert_eq!(fit(1, &b), Fit::Partial);
    assert_eq!(fit(2000, &b), Fit::Partial);
    assert_eq!(fit(2001, &b), Fit::TooBig);
}

#[test]
fn fit_serializes_lowercase() {
    assert_eq!(serde_json::to_value(Fit::TooBig).unwrap(), json!("toobig"));
}

#[test]
fn backend_choice() {
    assert_eq!(choose_backend(Some(&gpu(Vendor::Nvidia, GB, Some(cv(12, 4))))), Backend::Cuda);
    assert_eq!(choose_backend(Some(&gpu(Vendor::Nvidia, GB, None))), Backend::Vulkan);
    assert_eq!(choose_backend(Some(&gpu(Vendor::Amd, GB, None))), Backend::Vulkan);
    assert_eq!(choose_backend(Some(&gpu(Vendor::Intel, GB, None))), Backend::Vulkan);
    assert_eq!(choose_backend(Some(&gpu(Vendor::Apple, GB, None))), Backend::Cpu);
    assert_eq!(choose_backend(Some(&gpu(Vendor::Other, GB, None))), Backend::Cpu);
    assert_eq!(choose_backend(None), Backend::Cpu);
}
