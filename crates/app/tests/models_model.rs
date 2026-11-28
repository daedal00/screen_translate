//! Milestone 3d RED: Settings Models tab (port of src/models.ts). Needs `screen_translate_app::models`.
//! Exact API under test (all in `screen_translate_app::models`):
//!   fn size(u64) -> String            // >=1e9: "{:.1} GB", else "{round(n/1e6)} MB"
//!   fn memory(u64) -> String          // "{n/1e9 rounded to 1 decimal, trailing .0 dropped} GB"
//!   fn compact(u64) -> String         // Intl compact en, max 1 fraction digit: 999, 1.2K, 15.3K, 1M, 1.5M, 2B
//!   enum Tone { Neutral, Accent, Success, Warning, Danger }
//!   struct Badge { pub text: String, pub tone: Tone }
//!   struct Button { pub label: String, pub enabled: bool }
//!   struct Progress { pub job: String, pub file: String, pub downloaded: u64, pub total: Option<u64> }
//!   consts: EMPTY_RECOMMENDED, EMPTY_INSTALLED, EMPTY_FILES (the old empty-state strings, asserted verbatim)
//!   consts: MSG_DOWNLOADED "Model downloaded.", MSG_DELETED "Deleted.", MSG_STOPPED "Stopped.", MSG_SAVED "Saved.",
//!           MSG_INSTALLED "llama.cpp installed.", MSG_VULKAN "Switched to Vulkan.", MSG_STARTING "Starting..."
//!   enum RepoFiles { Loading, Error, Loaded(Vec<QuantRow>) }
//!   enum SearchState { Idle, Searching, Done, Error }
//!   struct ModelsModel { pub info: Option<ManagerInfo>, pub load_error: bool, pub confirm_delete: Option<String>,
//!       pub chosen_backend: Option<Backend>, pub active_key: Option<String>, pub last: Option<Progress>, pub speed: f64,
//!       pub search: SearchState, pub search_error: String, pub repos: Vec<HfRepo>, pub expanded: Option<String>,
//!       pub files_by_repo: HashMap<String, RepoFiles>, pub announcement: String }
//!   ModelsModel::new() -> Self   (info None, everything idle)
//!   -- state, ported from refresh()/download()/the progress listener --
//!   refresh_ok(&mut self, ManagerInfo)         // sets info, load_error=false; if !downloading clears active_key + last
//!   refresh_failed(&mut self)                  // load_error = true only when info is still None
//!   start_download(&mut self, key: &str)       // active_key=key, last=None, speed=0, sample reset, info.downloading=true
//!   on_progress(&mut self, Progress, now_ms: f64)  // ignored unless info.downloading; speed EMA 0.7/0.3; sets active_key if None
//!   busy / running / downloading_key(&str) -> bool
//!   -- status card (renderStatus) --
//!   status_pill() -> Option<(Tone, &'static str)>   status_name() -> String   offload_text() -> String
//!   server_button() -> Option<Button>   status_hint() -> Option<&'static str>
//!   install_button() -> Option<Button>  install_backend_arg() -> Option<Backend>
//!   cuda_on_cpu() -> bool   switch_vulkan_enabled() -> bool
//!   error_message() -> Option<String>   error_log() -> Option<String>   autostart() -> bool
//!   use_model_stops_first() -> bool   (useModel: stop_server first when running)
//!   -- hardware (renderHardware) --
//!   gpu_line() -> (String, String)    memory_line() -> (String, String)   // (label, value)
//!   budget_text() -> String   runtime_text() -> String   selected_backend() -> Backend
//!   backend_label(Backend) -> String   choose_backend(&mut self, Backend)
//!   installed_note() -> Option<String>   reinstall_button() -> Option<Button>   stop_to_reinstall_hint() -> bool
//!   -- cards / rows --
//!   fit_badge(Fit, too_big: &str) -> Badge     text_badge(&str) -> Badge     model_name(Option<&str>) -> String
//!   recommended_cards() -> Vec<Card>   Card { name, description, note: Option<String>, tag: bool, badges: Vec<Badge>, action: CardAction }
//!   enum CardAction { Progress, InUse, Use { enabled: bool }, Download { primary: bool, enabled: bool } }
//!   another_download_hint() -> bool
//!   installed_rows() -> Vec<InstalledItem>
//!     InstalledItem { file, name, size: String, fit: Badge, in_use: bool, use_enabled: bool, confirming: bool, delete_enabled: bool }
//!   ask_delete(&mut self, &str)   cancel_delete(&mut self)   confirm_delete_click(&mut self) -> Option<String>
//!   -- progress (progressSlot) --
//!   progress_text() -> String   progress_fraction() -> Option<f32>
//!   -- Hugging Face search --
//!   begin_search(&mut self, &str) -> Option<u64>      // None (and idle) for empty query, else Some(seq)
//!   finish_search(&mut self, seq: u64, Result<Vec<HfRepo>, String>) -> bool   // false = stale, ignored
//!   enum SearchView { Hint, Searching, Error { details: Option<String> }, NoResults(String), Results(Vec<RepoRow>) }
//!   search_view(&self, query: &str) -> SearchView
//!   RepoRow { id, downloads: String, likes: String, expanded: bool, files: Option<FilesView> }
//!   enum FilesView { Loading, Error, Empty, Rows(Vec<FileRow>) }
//!   FileRow { label, size: String, fit: Badge, action: FileAction }   enum FileAction { Progress, Download { enabled: bool } }
//!   toggle_repo(&mut self, &str) -> bool   // true: caller must fetch hf_files (state is now Loading)
//!   set_files(&mut self, &str, Result<Vec<QuantRow>, String>)
//!   -- custom source --
//!   custom_download(&mut self, &str) -> Option<String>   // trimmed source; None if blank; else start_download("custom")
//!   custom_enabled() -> bool
//!   ModelsView::new(ModelsModel, &mut Window, &mut Context<ModelsView>) -> ModelsView
use gpui::{AppContext, TestAppContext};
use screen_translate_app::models::*;
use screen_translate_core::manager::{
    BackendInfo, CatalogRow, InstalledRow, ManagerInfo, QuantRow,
    catalog::{Entry, Recommendation},
    hardware::{Budget, Fit, Gpu, Hardware, Vendor},
    models::{HfRepo, InstalledModel, QuantFile},
    runtime::{Backend, CudaVer},
    server::{Offload, Status},
};

const GB: u64 = 1_000_000_000;

fn entry(id: &str, file: &str, size: u64, note: Option<&str>) -> Entry {
    Entry {
        id: id.into(),
        name: format!("Name {id}"),
        description: format!("Desc {id}"),
        repo: "o/r".into(),
        file: file.into(),
        size,
        sha256: None,
        min_vram: 0,
        tags: vec![],
        prompt_template: String::new(),
        note: note.map(Into::into),
    }
}

fn rec(e: &Entry, fit: Fit, badges: &[&str]) -> Recommendation {
    Recommendation {
        entry: e.clone(),
        fit,
        badges: badges.iter().map(|s| s.to_string()).collect(),
    }
}

fn gpu_hw(unified: bool) -> Hardware {
    let gpu = Gpu {
        name: "RTX 4070".into(),
        vendor: Vendor::Nvidia,
        vram: 12 * GB,
        driver: None,
        max_cuda: Some(CudaVer {
            major: 12,
            minor: 4,
        }),
    };
    Hardware {
        gpu: Some(gpu),
        ram: 32 * GB,
        unified,
    }
}

/// macOS, unified 16 GB Apple GPU, runtime installed, two catalog entries recommended, one installed, server stopped.
fn base() -> ManagerInfo {
    let a: &'static Entry = Box::leak(Box::new(entry("a", "a.gguf", 5 * GB, Some("License note"))));
    let b: &'static Entry = Box::leak(Box::new(entry("b", "b.gguf", 2 * GB, None)));
    ManagerInfo {
        runtime: Some("/x/llama-server".into()),
        os: "macos",
        hardware: Hardware {
            gpu: Some(Gpu {
                name: "Apple M2".into(),
                vendor: Vendor::Apple,
                vram: 16 * GB,
                driver: None,
                max_cuda: None,
            }),
            ram: 16 * GB,
            unified: true,
        },
        budget: Budget {
            gpu: 10 * GB,
            total: 12 * GB,
        },
        recommended: vec![
            rec(a, Fit::Full, &["Best quality", "Fits your GPU"]),
            rec(b, Fit::Partial, &["Fastest", "Partly on CPU, slower"]),
        ],
        catalog: vec![
            CatalogRow {
                entry: a,
                fit: Fit::Full,
            },
            CatalogRow {
                entry: b,
                fit: Fit::Partial,
            },
        ],
        backend: BackendInfo {
            recommended: Backend::Cpu,
            installed: None,
            installed_gpu: None,
        },
        fallback_note: None,
        installed: vec![InstalledRow {
            model: InstalledModel {
                file: "a.gguf".into(),
                size: 5 * GB,
            },
            fit: Fit::Full,
        }],
        server: Status::Stopped,
        base_url: "http://127.0.0.1:8080".into(),
        autostart: false,
        selected_model: None,
        downloading: false,
    }
}

fn windows() -> ManagerInfo {
    let mut i = base();
    i.os = "windows";
    i.hardware = gpu_hw(false);
    i.budget = Budget {
        gpu: 12 * GB,
        total: 20 * GB,
    };
    i.backend = BackendInfo {
        recommended: Backend::Cuda,
        installed: Some(Backend::Cuda),
        installed_gpu: Some("RTX 4070".into()),
    };
    i
}

fn model(info: ManagerInfo) -> ModelsModel {
    let mut m = ModelsModel::new();
    m.refresh_ok(info);
    m
}

fn ready(file: &str, offload: Option<Offload>) -> Status {
    Status::Ready {
        model: file.into(),
        offload,
    }
}

fn off(device: Option<&str>, layers: u32, total: u32) -> Offload {
    Offload {
        device: device.map(Into::into),
        layers,
        total,
    }
}

fn prog(job: &str, file: &str, downloaded: u64, total: Option<u64>) -> Progress {
    Progress {
        job: job.into(),
        file: file.into(),
        downloaded,
        total,
    }
}

// ---- formatting ----

#[test]
fn size_formats() {
    assert_eq!(size(5_000_000_000), "5.0 GB");
    assert_eq!(size(4_920_000_000), "4.9 GB");
    assert_eq!(size(1_000_000_000), "1.0 GB");
    assert_eq!(size(999_999_999), "1000 MB");
    assert_eq!(size(450_400_000), "450 MB");
    assert_eq!(size(0), "0 MB");
}

#[test]
fn memory_drops_trailing_zero() {
    assert_eq!(memory(16 * GB), "16 GB");
    assert_eq!(memory(17_179_869_184), "17.2 GB");
    assert_eq!(memory(8 * GB), "8 GB");
    assert_eq!(memory(0), "0 GB");
}

#[test]
fn compact_matches_intl_en() {
    assert_eq!(compact(0), "0");
    assert_eq!(compact(999), "999");
    assert_eq!(compact(1_000), "1K");
    assert_eq!(compact(1_234), "1.2K");
    assert_eq!(compact(15_300), "15.3K");
    assert_eq!(compact(1_000_000), "1M");
    assert_eq!(compact(1_500_000), "1.5M");
    assert_eq!(compact(12_345_678), "12.3M");
    assert_eq!(compact(2_000_000_000), "2B");
}

#[test]
fn messages_are_the_old_strings() {
    assert_eq!(MSG_DOWNLOADED, "Model downloaded.");
    assert_eq!(MSG_DELETED, "Deleted.");
    assert_eq!(MSG_STOPPED, "Stopped.");
    assert_eq!(MSG_SAVED, "Saved.");
    assert_eq!(MSG_INSTALLED, "llama.cpp installed.");
    assert_eq!(MSG_VULKAN, "Switched to Vulkan.");
    assert_eq!(MSG_STARTING, "Starting...");
}

// ---- badges ----

#[test]
fn fit_badge_texts_follow_gpu_budget() {
    let m = model(base());
    let b = m.fit_badge(Fit::Full, "Too big");
    assert_eq!((b.text.as_str(), b.tone), ("Fits your GPU", Tone::Success));
    let b = m.fit_badge(Fit::Partial, "Too big");
    assert_eq!(
        (b.text.as_str(), b.tone),
        ("Partly on CPU, slower", Tone::Warning)
    );
    let b = m.fit_badge(Fit::TooBig, "Too big for this computer");
    assert_eq!(
        (b.text.as_str(), b.tone),
        ("Too big for this computer", Tone::Neutral)
    );

    let mut cpu = base();
    cpu.budget = Budget {
        gpu: 0,
        total: 8 * GB,
    };
    assert_eq!(
        model(cpu).fit_badge(Fit::Partial, "x").text,
        "Runs on CPU, slower"
    );
    assert_eq!(
        ModelsModel::new().fit_badge(Fit::Partial, "x").text,
        "Runs on CPU, slower"
    );
}

#[test]
fn text_badge_tones() {
    let m = ModelsModel::new();
    assert_eq!(m.text_badge("Best quality").tone, Tone::Accent);
    assert_eq!(m.text_badge("fastest").tone, Tone::Accent);
    assert_eq!(m.text_badge("Fits your GPU").tone, Tone::Success);
    assert_eq!(m.text_badge("Partly on CPU, slower").tone, Tone::Warning);
    assert_eq!(m.text_badge("Runs on CPU, slower").tone, Tone::Warning);
    assert_eq!(m.text_badge("Multilingual").tone, Tone::Neutral);
    assert_eq!(m.text_badge("Multilingual").text, "Multilingual");
}

#[test]
fn model_name_uses_catalog_or_file() {
    let m = model(base());
    assert_eq!(m.model_name(Some("a.gguf")), "Name a");
    assert_eq!(m.model_name(Some("other.gguf")), "other.gguf");
    assert_eq!(m.model_name(None), "");
    assert_eq!(ModelsModel::new().model_name(Some("a.gguf")), "a.gguf");
}

// ---- status card ----

#[test]
fn loading_state() {
    let m = ModelsModel::new();
    assert!(m.info.is_none() && !m.load_error);
    assert!(m.status_pill().is_none() && m.server_button().is_none() && !m.busy() && !m.running());
    let mut m = m;
    m.refresh_failed();
    assert!(m.load_error);
    m.refresh_ok(base());
    assert!(!m.load_error);
    m.refresh_failed();
    assert!(
        !m.load_error,
        "a failed refresh after a good load keeps the old info"
    );
    assert!(m.info.is_some());
}

#[test]
fn status_pill_and_name() {
    let mut i = base();
    let m = model(base());
    assert_eq!(m.status_pill(), Some((Tone::Neutral, "Stopped")));
    assert_eq!(m.status_name(), "No model running");

    i.server = Status::Loading {
        model: "a.gguf".into(),
    };
    let m = model(i);
    assert_eq!(m.status_pill(), Some((Tone::Warning, "Loading")));
    assert_eq!(m.status_name(), "Name a");

    let mut i = base();
    i.server = ready("zzz.gguf", None);
    let m = model(i);
    assert_eq!(m.status_pill(), Some((Tone::Success, "Ready")));
    assert_eq!(m.status_name(), "zzz.gguf");

    let mut i = base();
    i.server = Status::Error {
        message: "boom".into(),
        log: vec![],
    };
    assert_eq!(model(i).status_pill(), Some((Tone::Danger, "Error")));
}

#[test]
fn status_name_falls_back_to_selected_model() {
    let mut i = base();
    i.selected_model = Some("a.gguf".into());
    assert_eq!(model(i).status_name(), "Name a");
}

#[test]
fn offload_text_only_when_ready() {
    let mut i = base();
    i.server = ready("a.gguf", Some(off(Some("Metal"), 33, 33)));
    assert_eq!(model(i).offload_text(), "Running on GPU: 33/33 layers");
    let mut i = base();
    i.server = ready("a.gguf", Some(off(Some("Metal"), 20, 33)));
    assert_eq!(model(i).offload_text(), "Partly on GPU: 20/33 layers");
    let mut i = base();
    i.server = ready("a.gguf", Some(off(None, 0, 33)));
    assert_eq!(model(i).offload_text(), "Running on CPU");
    let mut i = base();
    i.server = ready("a.gguf", None);
    assert_eq!(model(i).offload_text(), "");
    let mut i = base();
    i.server = Status::Loading {
        model: "a.gguf".into(),
    };
    assert_eq!(model(i).offload_text(), "");
}

#[test]
fn server_button_states() {
    let b = model(base()).server_button().unwrap();
    assert_eq!(
        (b.label.as_str(), b.enabled),
        ("Start", true),
        "installed model, selected falls back to first installed"
    );

    let mut i = base();
    i.installed.clear();
    let m = model(i);
    let b = m.server_button().unwrap();
    assert_eq!((b.label.as_str(), b.enabled), ("Start", false));
    assert_eq!(m.status_hint(), Some("Download a model first."));

    let mut i = base();
    i.installed.clear();
    i.downloading = true;
    assert_eq!(model(i).status_hint(), Some("Downloading..."));

    let mut i = base();
    i.server = Status::Loading {
        model: "a.gguf".into(),
    };
    let b = model(i).server_button().unwrap();
    assert_eq!((b.label.as_str(), b.enabled), ("Stop", true));

    let mut i = base();
    i.server = ready("a.gguf", None);
    let m = model(i);
    assert_eq!(m.server_button().unwrap().label, "Stop");
    assert_eq!(m.status_hint(), None);

    let mut i = base();
    i.server = Status::Error {
        message: "x".into(),
        log: vec![],
    };
    assert_eq!(model(i).server_button().unwrap().label, "Try again");

    let mut i = base();
    i.runtime = None;
    let m = model(i);
    assert!(m.server_button().is_none());
    assert_eq!(m.status_hint(), None, "no hint without a runtime");
}

#[test]
fn use_model_stops_a_running_server_first() {
    assert!(!model(base()).use_model_stops_first());
    let mut i = base();
    i.server = Status::Loading {
        model: "a.gguf".into(),
    };
    assert!(model(i).use_model_stops_first());
    let mut i = base();
    i.server = Status::Error {
        message: "x".into(),
        log: vec![],
    };
    assert!(!model(i).use_model_stops_first());
}

#[test]
fn install_button_and_backend_arg() {
    let mut i = base();
    i.runtime = None;
    let m = model(i);
    let b = m.install_button().unwrap();
    assert_eq!((b.label.as_str(), b.enabled), ("Install llama.cpp", true));
    assert_eq!(m.install_backend_arg(), None, "macOS passes no backend");
    assert!(model(base()).install_button().is_none());

    let mut i = windows();
    i.runtime = None;
    i.backend.installed = None;
    let mut m = model(i);
    assert_eq!(
        m.install_backend_arg(),
        Some(Backend::Cuda),
        "recommended when nothing chosen"
    );
    m.choose_backend(Backend::Vulkan);
    assert_eq!(m.install_backend_arg(), Some(Backend::Vulkan));

    let mut i = base();
    i.runtime = None;
    i.downloading = true;
    assert!(!model(i).install_button().unwrap().enabled);
}

#[test]
fn cuda_on_cpu_warning() {
    let mut i = windows();
    i.server = ready("a.gguf", Some(off(None, 0, 33)));
    let m = model(i);
    assert!(m.cuda_on_cpu());
    assert!(m.switch_vulkan_enabled());
    let mut i = windows();
    i.server = ready("a.gguf", None);
    assert!(model(i).cuda_on_cpu(), "unknown offload counts as CPU");
    let mut i = windows();
    i.server = ready("a.gguf", Some(off(Some("CUDA0"), 33, 33)));
    assert!(!model(i).cuda_on_cpu());
    let mut i = windows();
    i.backend.installed = Some(Backend::Vulkan);
    i.server = ready("a.gguf", Some(off(None, 0, 33)));
    assert!(!model(i).cuda_on_cpu());
    let mut i = windows();
    i.server = Status::Loading {
        model: "a.gguf".into(),
    };
    assert!(!model(i).cuda_on_cpu());
    let mut i = windows();
    i.server = ready("a.gguf", Some(off(None, 0, 33)));
    i.downloading = true;
    assert!(!model(i).switch_vulkan_enabled());
}

#[test]
fn error_message_and_log() {
    let mut i = base();
    i.server = Status::Error {
        message: "boom".into(),
        log: vec!["l1".into(), "l2".into()],
    };
    let m = model(i);
    assert_eq!(m.error_message().as_deref(), Some("boom"));
    assert_eq!(m.error_log().as_deref(), Some("l1\nl2"));
    let mut i = base();
    i.server = Status::Error {
        message: String::new(),
        log: vec![],
    };
    let m = model(i);
    assert_eq!(
        m.error_message().as_deref(),
        Some("The server stopped unexpectedly.")
    );
    assert_eq!(m.error_log(), None);
    assert_eq!(model(base()).error_message(), None);
}

#[test]
fn autostart_reflects_info() {
    assert!(!model(base()).autostart());
    let mut i = base();
    i.autostart = true;
    assert!(model(i).autostart());
    assert!(!ModelsModel::new().autostart());
}

// ---- hardware ----

#[test]
fn hardware_lines() {
    let m = model(base());
    assert_eq!(m.gpu_line(), ("Chip".to_string(), "Apple M2".to_string()));
    assert_eq!(
        m.memory_line(),
        ("Memory".to_string(), "16 GB unified".to_string())
    );
    let m = model(windows());
    assert_eq!(
        m.gpu_line(),
        ("GPU".to_string(), "RTX 4070, 12 GB VRAM".to_string())
    );
    assert_eq!(
        m.memory_line(),
        ("Memory".to_string(), "32 GB RAM".to_string())
    );
    let mut i = windows();
    i.hardware.gpu = None;
    assert_eq!(model(i).gpu_line().1, "None detected");
}

#[test]
fn budget_text_cases() {
    let mut i = base();
    i.budget.gpu = 12 * GB; // free 10.5 GB
    assert_eq!(
        model(i).budget_text(),
        "Models up to ~10.5 GB run fully on your GPU."
    );
    let mut i = base();
    i.budget.gpu = 9_500_000_000; // free exactly 8.0
    assert_eq!(
        model(i).budget_text(),
        "Models up to ~8.0 GB run fully on your GPU."
    );
    let mut i = base();
    i.budget.gpu = GB;
    assert_eq!(
        model(i).budget_text(),
        "Your GPU has little free memory: models will run partly on the CPU."
    );
    let mut i = base();
    i.budget.gpu = 0;
    assert_eq!(
        model(i).budget_text(),
        "No supported GPU found: models run on the CPU, which is slower."
    );
    let mut i = base();
    i.hardware.gpu = None;
    assert_eq!(
        model(i).budget_text(),
        "No supported GPU found: models run on the CPU, which is slower."
    );
}

#[test]
fn runtime_text() {
    assert_eq!(model(base()).runtime_text(), "llama.cpp: Metal (built in)");
    let mut i = windows();
    i.backend.installed = None;
    assert_eq!(model(i).runtime_text(), "llama.cpp: not installed");
    assert_eq!(model(windows()).runtime_text(), "llama.cpp: CUDA");
    let mut i = windows();
    i.backend.installed = Some(Backend::Vulkan);
    assert_eq!(
        model(i).runtime_text(),
        "llama.cpp: Vulkan (recommended: CUDA)"
    );
}

#[test]
fn backend_labels_and_selection() {
    let mut m = model(windows());
    assert_eq!(m.backend_label(Backend::Cuda), "CUDA (recommended)");
    assert_eq!(m.backend_label(Backend::Vulkan), "Vulkan");
    assert_eq!(m.backend_label(Backend::Cpu), "CPU");
    assert_eq!(m.selected_backend(), Backend::Cuda);
    m.choose_backend(Backend::Cpu);
    assert_eq!(m.selected_backend(), Backend::Cpu);
    assert_eq!(m.chosen_backend, Some(Backend::Cpu));
}

#[test]
fn installed_note_and_reinstall_button_windows() {
    let mut m = model(windows());
    assert!(
        m.reinstall_button().is_none(),
        "selected == installed == recommended"
    );
    assert_eq!(m.installed_note(), None);
    m.choose_backend(Backend::Vulkan);
    assert_eq!(m.installed_note().as_deref(), Some("Installed: CUDA"));
    let b = m.reinstall_button().unwrap();
    assert_eq!((b.label.as_str(), b.enabled), ("Reinstall", true));
    assert!(!m.stop_to_reinstall_hint());
}

#[test]
fn reinstall_when_recommendation_or_gpu_changed() {
    let mut i = windows();
    i.backend.installed = Some(Backend::Vulkan); // installed != recommended
    assert!(model(i).reinstall_button().is_some());
    let mut i = windows();
    i.backend.installed_gpu = Some("GTX 1060".into()); // GPU swapped
    assert!(model(i).reinstall_button().is_some());
    let mut i = windows();
    i.backend.installed = Some(Backend::Vulkan);
    i.runtime = None;
    assert!(model(i).reinstall_button().is_none(), "needs a runtime");
    let mut i = base();
    i.backend = BackendInfo {
        recommended: Backend::Cpu,
        installed: Some(Backend::Vulkan),
        installed_gpu: None,
    };
    assert!(model(i).reinstall_button().is_none(), "never on macOS");
}

#[test]
fn reinstall_locked_while_busy_or_running() {
    let mut i = windows();
    i.backend.installed = Some(Backend::Vulkan);
    i.downloading = true;
    assert!(!model(i).reinstall_button().unwrap().enabled);
    let mut i = windows();
    i.backend.installed = Some(Backend::Vulkan);
    i.server = ready("a.gguf", None);
    let m = model(i);
    assert!(!m.reinstall_button().unwrap().enabled);
    assert!(m.stop_to_reinstall_hint());
}

// ---- recommended cards ----

#[test]
fn recommended_cards_content() {
    let cards = model(base()).recommended_cards();
    assert_eq!(cards.len(), 2);
    let c = &cards[0];
    assert_eq!(
        (c.name.as_str(), c.description.as_str()),
        ("Name a", "Desc a")
    );
    assert_eq!(c.note.as_deref(), Some("License note"));
    assert!(c.tag);
    let texts: Vec<_> = c.badges.iter().map(|b| (b.text.as_str(), b.tone)).collect();
    assert_eq!(
        texts,
        [
            ("5.0 GB", Tone::Neutral),
            ("Best quality", Tone::Accent),
            ("Fits your GPU", Tone::Success)
        ]
    );
    assert!(!cards[1].tag);
    assert_eq!(cards[1].note, None);
}

#[test]
fn recommended_card_actions() {
    // a.gguf is installed, b is not
    let cards = model(base()).recommended_cards();
    assert_eq!(cards[0].action, CardAction::Use { enabled: true });
    assert_eq!(
        cards[1].action,
        CardAction::Download {
            primary: false,
            enabled: true
        }
    );

    let mut i = base();
    i.installed.clear();
    let cards = model(i).recommended_cards();
    assert_eq!(
        cards[0].action,
        CardAction::Download {
            primary: true,
            enabled: true
        }
    );
    assert_eq!(
        cards[1].action,
        CardAction::Download {
            primary: false,
            enabled: true
        }
    );

    let mut i = base();
    i.runtime = None;
    assert_eq!(
        model(i).recommended_cards()[0].action,
        CardAction::Use { enabled: false }
    );

    let mut i = base();
    i.server = ready("a.gguf", None);
    assert_eq!(model(i).recommended_cards()[0].action, CardAction::InUse);
}

#[test]
fn recommended_download_locks_and_shows_progress_for_active_card() {
    let mut i = base();
    i.installed.clear();
    let mut m = model(i);
    assert!(!m.another_download_hint());
    m.start_download("a.gguf");
    let cards = m.recommended_cards();
    assert_eq!(cards[0].action, CardAction::Progress);
    assert_eq!(
        cards[1].action,
        CardAction::Download {
            primary: false,
            enabled: false
        }
    );
    assert!(m.another_download_hint());
}

#[test]
fn recommended_empty_when_none() {
    let mut i = base();
    i.recommended.clear();
    assert!(model(i).recommended_cards().is_empty());
    assert_eq!(
        EMPTY_RECOMMENDED,
        "No recommended models for this computer. Search below to find one."
    );
}

// ---- installed list + delete ----

#[test]
fn installed_rows_content() {
    let rows = model(base()).installed_rows();
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(
        (r.file.as_str(), r.name.as_str(), r.size.as_str()),
        ("a.gguf", "Name a", "5.0 GB")
    );
    assert_eq!(r.fit.text, "Fits your GPU");
    assert!(!r.in_use && r.use_enabled && !r.confirming && r.delete_enabled);
    assert_eq!(
        EMPTY_INSTALLED,
        "No models yet. Pick a recommended one above to get started."
    );
}

#[test]
fn installed_row_in_use_cannot_be_deleted() {
    let mut i = base();
    i.server = ready("a.gguf", None);
    let r = model(i).installed_rows().remove(0);
    assert!(r.in_use && !r.delete_enabled);
    let mut i = base();
    i.server = Status::Loading {
        model: "other.gguf".into(),
    };
    assert!(!model(i).installed_rows()[0].in_use);
}

#[test]
fn installed_use_needs_runtime() {
    let mut i = base();
    i.runtime = None;
    assert!(!model(i).installed_rows()[0].use_enabled);
}

#[test]
fn delete_is_two_step() {
    let mut m = model(base());
    m.ask_delete("a.gguf");
    assert!(m.installed_rows()[0].confirming);
    m.cancel_delete();
    assert!(!m.installed_rows()[0].confirming);
    assert_eq!(m.confirm_delete_click(), None, "nothing pending");
    m.ask_delete("a.gguf");
    assert_eq!(m.confirm_delete_click().as_deref(), Some("a.gguf"));
    assert!(
        !m.installed_rows()[0].confirming,
        "confirming clears the prompt"
    );
}

// ---- download lifecycle + progress text ----

#[test]
fn start_download_marks_busy_immediately() {
    let mut m = model(base());
    m.last = Some(prog("model", "x", 1, None));
    m.speed = 5.0;
    m.start_download("custom");
    assert!(m.busy());
    assert!(m.downloading_key("custom") && !m.downloading_key("a.gguf"));
    assert!(m.last.is_none());
    assert_eq!(m.speed, 0.0);
    assert!(!m.custom_enabled());
}

#[test]
fn downloading_key_requires_busy() {
    let mut m = model(base());
    m.active_key = Some("runtime".into());
    assert!(!m.downloading_key("runtime"));
}

#[test]
fn progress_text_variants() {
    let mut m = model(base());
    m.start_download("a.gguf");
    assert_eq!(m.progress_text(), "Starting download...");
    assert_eq!(m.progress_fraction(), None);
    m.last = Some(prog("model", "a.gguf", 450_000_000, None));
    assert_eq!(m.progress_text(), "450 MB downloaded");
    assert_eq!(m.progress_fraction(), None);
    m.last = Some(prog("model", "a.gguf", 2 * GB, Some(4 * GB)));
    assert_eq!(m.progress_text(), "2.0 GB of 4.0 GB");
    assert_eq!(m.progress_fraction(), Some(0.5));
    m.speed = 12_400_000.0;
    assert_eq!(m.progress_text(), "2.0 GB of 4.0 GB, 12 MB/s");
    m.last = Some(prog("model", "a.gguf", 5 * GB, Some(4 * GB)));
    assert_eq!(m.progress_fraction(), Some(1.0), "clamped");
    m.last = Some(prog("model", "a.gguf", 10, Some(0)));
    assert_eq!(m.progress_fraction(), None, "a zero total is indeterminate");
}

#[test]
fn on_progress_ignored_unless_downloading() {
    let mut m = model(base());
    m.on_progress(prog("model", "a", 1, None), 0.0);
    assert!(m.last.is_none() && m.active_key.is_none());
}

#[test]
fn on_progress_sets_key_when_missing() {
    let mut m = model(base());
    m.info.as_mut().unwrap().downloading = true;
    m.on_progress(prog("runtime", "llama.cpp", 1, None), 0.0);
    assert_eq!(m.active_key.as_deref(), Some("runtime"));
    let mut m = model(base());
    m.info.as_mut().unwrap().downloading = true;
    m.on_progress(prog("model", "q.gguf", 1, None), 0.0);
    assert_eq!(m.active_key.as_deref(), Some("q.gguf"));
    m.on_progress(prog("model", "other", 2, None), 10.0);
    assert_eq!(
        m.active_key.as_deref(),
        Some("q.gguf"),
        "an existing key is kept"
    );
}

#[test]
fn on_progress_speed_is_smoothed() {
    let mut m = model(base());
    m.start_download("a.gguf");
    m.on_progress(prog("model", "a.gguf", 0, Some(100 * GB)), 0.0);
    assert_eq!(m.speed, 0.0, "first sample has no rate");
    m.on_progress(prog("model", "a.gguf", 10_000_000, Some(100 * GB)), 1000.0);
    assert!((m.speed - 10_000_000.0).abs() < 1.0);
    m.on_progress(prog("model", "a.gguf", 30_000_000, Some(100 * GB)), 2000.0);
    assert!((m.speed - 13_000_000.0).abs() < 1.0, "0.7*10M + 0.3*20M");
    assert_eq!(m.last.as_ref().unwrap().downloaded, 30_000_000);
    // downloaded going backwards (a new file) takes no sample rate but re-bases it
    m.on_progress(prog("model", "b", 0, Some(100 * GB)), 3000.0);
    assert!((m.speed - 13_000_000.0).abs() < 1.0);
}

#[test]
fn refresh_clears_download_state_when_idle() {
    let mut m = model(base());
    m.start_download("a.gguf");
    m.last = Some(prog("model", "a.gguf", 1, None));
    let mut still = base();
    still.downloading = true;
    m.refresh_ok(still);
    assert!(
        m.active_key.is_some() && m.last.is_some(),
        "still downloading keeps the slot"
    );
    m.refresh_ok(base());
    assert!(m.active_key.is_none() && m.last.is_none());
}

// ---- Hugging Face search ----

fn repo(id: &str, downloads: u64, likes: u64) -> HfRepo {
    HfRepo {
        id: id.into(),
        downloads,
        likes,
    }
}

fn qrow(quant: &str, name: &str, size: u64, parts: u32, fit: Fit) -> QuantRow {
    QuantRow {
        file: QuantFile {
            quant: quant.into(),
            name: name.into(),
            size,
            parts,
        },
        fit,
    }
}

#[test]
fn search_hint_when_idle_or_blank() {
    let m = model(base());
    assert!(matches!(m.search_view(""), SearchView::Hint));
    assert!(
        matches!(m.search_view("qwen"), SearchView::Hint),
        "idle state shows the hint"
    );
}

#[test]
fn begin_search_blank_resets_to_idle() {
    let mut m = model(base());
    let s = m.begin_search("qwen").unwrap();
    m.finish_search(s, Ok(vec![repo("a/b", 1, 1)]));
    assert_eq!(m.begin_search(""), None);
    assert!(matches!(m.search, SearchState::Idle));
    assert_eq!(m.announcement, "");
}

#[test]
fn search_flow_results_and_announcement() {
    let mut m = model(base());
    let s = m.begin_search("qwen").unwrap();
    assert!(matches!(m.search_view("qwen"), SearchView::Searching));
    assert!(m.finish_search(
        s,
        Ok(vec![repo("a/qwen", 15_300, 1_234), repo("b/qwen", 999, 0)])
    ));
    assert_eq!(m.announcement, "2 models found");
    let SearchView::Results(rows) = m.search_view("qwen") else {
        panic!("expected results")
    };
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (
            rows[0].id.as_str(),
            rows[0].downloads.as_str(),
            rows[0].likes.as_str()
        ),
        ("a/qwen", "15.3K downloads", "1.2K likes")
    );
    assert_eq!(rows[1].downloads, "999 downloads");
    assert!(!rows[0].expanded && rows[0].files.is_none());
}

#[test]
fn search_no_results() {
    let mut m = model(base());
    let s = m.begin_search("zzzz").unwrap();
    m.finish_search(s, Ok(vec![]));
    assert_eq!(m.announcement, "No models found");
    let SearchView::NoResults(t) = m.search_view("zzzz") else {
        panic!()
    };
    assert_eq!(
        t,
        "No models found for \"zzzz\". Try a shorter or different name."
    );
}

#[test]
fn search_error_has_details() {
    let mut m = model(base());
    let s = m.begin_search("q").unwrap();
    m.finish_search(s, Err("timeout".into()));
    assert_eq!(m.announcement, "Couldn't reach Hugging Face");
    let SearchView::Error { details } = m.search_view("q") else {
        panic!()
    };
    assert_eq!(details.as_deref(), Some("timeout"));
    assert!(m.repos.is_empty());
    let s = m.begin_search("q").unwrap();
    m.finish_search(s, Err(String::new()));
    let SearchView::Error { details } = m.search_view("q") else {
        panic!()
    };
    assert_eq!(details, None);
}

#[test]
fn stale_search_results_are_dropped() {
    let mut m = model(base());
    let first = m.begin_search("qw").unwrap();
    let second = m.begin_search("qwen").unwrap();
    assert!(second > first);
    assert!(!m.finish_search(first, Ok(vec![repo("old/x", 1, 1)])));
    assert!(matches!(m.search, SearchState::Searching));
    assert!(m.finish_search(second, Ok(vec![repo("new/x", 1, 1)])));
    assert_eq!(m.repos[0].id, "new/x");
}

#[test]
fn clearing_the_query_invalidates_inflight_search() {
    let mut m = model(base());
    let s = m.begin_search("qwen").unwrap();
    assert_eq!(m.begin_search(""), None);
    assert!(!m.finish_search(s, Ok(vec![repo("a/b", 1, 1)])));
    assert!(matches!(m.search, SearchState::Idle));
}

#[test]
fn new_search_collapses_expanded_repo() {
    let mut m = model(base());
    let s = m.begin_search("q").unwrap();
    m.finish_search(s, Ok(vec![repo("a/b", 1, 1)]));
    m.toggle_repo("a/b");
    assert_eq!(m.expanded.as_deref(), Some("a/b"));
    m.begin_search("qw");
    assert_eq!(m.expanded, None);
}

fn with_repo(m: &mut ModelsModel) {
    let s = m.begin_search("q").unwrap();
    m.finish_search(s, Ok(vec![repo("a/b", 1, 1)]));
}

fn expanded_files(m: &ModelsModel) -> Option<FilesView> {
    let SearchView::Results(mut rows) = m.search_view("q") else {
        panic!()
    };
    rows.remove(0).files
}

#[test]
fn toggle_repo_fetches_once_then_caches() {
    let mut m = model(base());
    with_repo(&mut m);
    assert!(m.toggle_repo("a/b"), "first expand needs a fetch");
    assert!(matches!(expanded_files(&m), Some(FilesView::Loading)));
    m.set_files(
        "a/b",
        Ok(vec![qrow("Q4_K_M", "m-Q4_K_M.gguf", 2 * GB, 1, Fit::Full)]),
    );
    assert!(matches!(expanded_files(&m), Some(FilesView::Rows(_))));
    assert!(!m.toggle_repo("a/b"), "collapsing never fetches");
    assert!(expanded_files(&m).is_none());
    assert!(!m.toggle_repo("a/b"), "re-expanding uses the cache");
    assert!(matches!(expanded_files(&m), Some(FilesView::Rows(_))));
}

#[test]
fn toggle_repo_retries_after_error() {
    let mut m = model(base());
    with_repo(&mut m);
    assert!(m.toggle_repo("a/b"));
    m.set_files("a/b", Err("nope".into()));
    assert!(matches!(expanded_files(&m), Some(FilesView::Error)));
    m.toggle_repo("a/b");
    assert!(m.toggle_repo("a/b"), "an errored repo is fetched again");
}

#[test]
fn toggle_other_repo_switches_expansion() {
    let mut m = model(base());
    m.toggle_repo("a/b");
    m.toggle_repo("c/d");
    assert_eq!(m.expanded.as_deref(), Some("c/d"));
}

#[test]
fn repo_file_rows() {
    let mut m = model(base());
    with_repo(&mut m);
    m.toggle_repo("a/b");
    m.set_files(
        "a/b",
        Ok(vec![
            qrow("Q4_K_M", "m-Q4_K_M.gguf", 2 * GB, 1, Fit::Full),
            qrow("", "plain.gguf", 450_000_000, 3, Fit::Partial),
            qrow("F16", "f16.gguf", 40 * GB, 1, Fit::TooBig),
        ]),
    );
    let Some(FilesView::Rows(rows)) = expanded_files(&m) else {
        panic!()
    };
    assert_eq!(
        (rows[0].label.as_str(), rows[0].size.as_str()),
        ("Q4_K_M", "2.0 GB")
    );
    assert_eq!(rows[0].fit.text, "Fits your GPU");
    assert_eq!(rows[0].action, FileAction::Download { enabled: true });
    assert_eq!(
        (rows[1].label.as_str(), rows[1].size.as_str()),
        ("plain.gguf", "450 MB, 3 parts"),
        "no quant: file name"
    );
    assert_eq!(rows[2].fit.text, "Too big for this computer");
    assert_eq!(rows[2].action, FileAction::Download { enabled: false });
}

#[test]
fn repo_file_download_states() {
    let mut m = model(base());
    with_repo(&mut m);
    m.toggle_repo("a/b");
    m.set_files(
        "a/b",
        Ok(vec![
            qrow("Q4", "q4.gguf", GB, 1, Fit::Full),
            qrow("Q8", "q8.gguf", 2 * GB, 1, Fit::Full),
        ]),
    );
    m.start_download("q4.gguf");
    let Some(FilesView::Rows(rows)) = expanded_files(&m) else {
        panic!()
    };
    assert_eq!(rows[0].action, FileAction::Progress);
    assert_eq!(rows[1].action, FileAction::Download { enabled: false });
}

#[test]
fn repo_files_empty() {
    let mut m = model(base());
    with_repo(&mut m);
    m.toggle_repo("a/b");
    m.set_files("a/b", Ok(vec![]));
    assert!(matches!(expanded_files(&m), Some(FilesView::Empty)));
    assert_eq!(
        EMPTY_FILES,
        "No downloadable GGUF files in this repository."
    );
}

// ---- custom source ----

#[test]
fn custom_download_trims_and_locks() {
    let mut m = model(base());
    assert!(m.custom_enabled());
    assert_eq!(m.custom_download("   "), None);
    assert!(!m.busy(), "blank input does nothing");
    assert_eq!(
        m.custom_download("  owner/repo:Q4_K_M  ").as_deref(),
        Some("owner/repo:Q4_K_M")
    );
    assert!(m.busy() && m.downloading_key("custom") && !m.custom_enabled());
}

// ---- starting flag (double-click guard) ----

#[test]
fn starting_disables_start_buttons_until_refresh() {
    let mut m = model(base());
    assert!(m.begin_start());
    assert!(
        !m.begin_start(),
        "a second Use while one is starting is dropped"
    );
    assert!(!m.server_button().unwrap().enabled);
    assert_eq!(
        m.recommended_cards()[0].action,
        CardAction::Use { enabled: false }
    );
    assert!(!m.installed_rows()[0].use_enabled);
    m.refresh_ok(base());
    assert!(m.server_button().unwrap().enabled && m.installed_rows()[0].use_enabled);
    assert!(m.begin_start());
    m.refresh_failed();
    assert!(!m.starting, "a failed refresh also ends it");
}

// ---- view smoke ----

#[gpui::test]
fn models_view_opens_in_root(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let mut i = base();
    i.server = ready("a.gguf", Some(off(Some("Metal"), 33, 33)));
    i.fallback_note = Some("The CUDA build failed to start, switched to Vulkan.".into());
    let m = model(i);
    let window = cx.add_window(|window, cx| {
        let view = cx.new(|cx| ModelsView::new(m, window, cx));
        gpui_component::Root::new(view, window, cx)
    });
    cx.run_until_parked();
    window.update(cx, |_, _, _| ()).unwrap();
}

#[gpui::test]
fn models_view_renders_before_info_loads(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let window = cx.add_window(|window, cx| {
        let view = cx.new(|cx| ModelsView::new(ModelsModel::new(), window, cx));
        gpui_component::Root::new(view, window, cx)
    });
    cx.run_until_parked();
    window.update(cx, |_, _, _| ()).unwrap();
}
