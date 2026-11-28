//! Settings window, Models tab. `ModelsModel` is the pure state and its derived view data;
//! `ModelsView` renders it and emits `ModelsCommand`s for the app to run on the core manager.

use std::cell::Cell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Context, Div, ElementId, Entity, EventEmitter, Hsla, IntoElement,
    ParentElement, Render, SharedString, Styled, Subscription, Task, Window, div, prelude::*, px,
};
use gpui_component::{
    ActiveTheme, Disableable, Selectable, Sizable,
    button::{Button as UiButton, ButtonVariants},
    checkbox::Checkbox,
    input::{Input, InputEvent, InputState},
    progress::Progress as UiProgress,
};
use screen_translate_core::manager::{
    ManagerInfo, QuantRow,
    hardware::{Fit, OVERHEAD},
    models::HfRepo,
    runtime::Backend,
    server::Status,
};

// ---- formatting ----

/// "5.0 GB" from 1 GB up, else whole MB.
pub fn size(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.1} GB", n as f64 / 1e9)
    } else {
        format!("{} MB", (n as f64 / 1e6).round() as u64)
    }
}

/// Memory amounts read better as whole numbers: "16 GB", "17.2 GB".
pub fn memory(n: u64) -> String {
    let v: f64 = format!("{:.1}", n as f64 / 1e9).parse().unwrap_or(0.0);
    format!("{v} GB")
}

/// `Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 1 })`.
pub fn compact(n: u64) -> String {
    const UNITS: [(f64, &str); 4] = [(1e3, "K"), (1e6, "M"), (1e9, "B"), (1e12, "T")];
    let mut value = n as f64;
    let mut suffix = "";
    for (i, (unit, name)) in UNITS.iter().enumerate() {
        if (n as f64) < *unit {
            break;
        }
        value = n as f64 / unit;
        suffix = name;
        // 999_999 reads "1M", not "1000K".
        if (value * 10.0).round() / 10.0 >= 1000.0 && i + 1 < UNITS.len() {
            value = n as f64 / UNITS[i + 1].0;
            suffix = UNITS[i + 1].1;
        }
    }
    let v = (value * 10.0).round() / 10.0;
    format!("{v}{suffix}")
}

pub const EMPTY_RECOMMENDED: &str =
    "No recommended models for this computer. Search below to find one.";
pub const EMPTY_INSTALLED: &str = "No models yet. Pick a recommended one above to get started.";
pub const EMPTY_FILES: &str = "No downloadable GGUF files in this repository.";
pub const MSG_DOWNLOADED: &str = "Model downloaded.";
pub const MSG_DELETED: &str = "Deleted.";
pub const MSG_STOPPED: &str = "Stopped.";
pub const MSG_SAVED: &str = "Saved.";
pub const MSG_INSTALLED: &str = "llama.cpp installed.";
pub const MSG_VULKAN: &str = "Switched to Vulkan.";
pub const MSG_STARTING: &str = "Starting...";
const SEARCH_HINT: &str =
    "Search for a model by name, for example qwen or llama. Only GGUF models are listed.";
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(350);

// ---- view data ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Accent,
    Success,
    Warning,
    Danger,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Badge {
    pub text: String,
    pub tone: Tone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    pub label: String,
    pub enabled: bool,
}

/// Overall download progress, as the manager reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub job: String,
    pub file: String,
    pub downloaded: u64,
    pub total: Option<u64>,
}

pub enum RepoFiles {
    Loading,
    Error,
    Loaded(Vec<QuantRow>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchState {
    Idle,
    Searching,
    Done,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardAction {
    Progress,
    InUse,
    Use { enabled: bool },
    Download { primary: bool, enabled: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub id: String,
    pub file: String,
    pub name: String,
    pub description: String,
    pub note: Option<String>,
    pub tag: bool,
    pub badges: Vec<Badge>,
    pub action: CardAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledItem {
    pub file: String,
    pub name: String,
    pub size: String,
    pub fit: Badge,
    pub in_use: bool,
    pub use_enabled: bool,
    pub confirming: bool,
    pub delete_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileAction {
    Progress,
    Download { enabled: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    pub name: String,
    pub label: String,
    pub size: String,
    pub fit: Badge,
    pub action: FileAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesView {
    Loading,
    Error,
    Empty,
    Rows(Vec<FileRow>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRow {
    pub id: String,
    pub downloads: String,
    pub likes: String,
    pub expanded: bool,
    pub files: Option<FilesView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchView {
    Hint,
    Searching,
    Error { details: Option<String> },
    NoResults(String),
    Results(Vec<RepoRow>),
}

// ---- model ----

pub struct ModelsModel {
    pub info: Option<ManagerInfo>,
    pub load_error: bool,
    pub confirm_delete: Option<String>,
    pub chosen_backend: Option<Backend>,
    /// Which card/row/runtime is downloading (its slot shows progress): "runtime", "custom" or a file name.
    pub active_key: Option<String>,
    pub last: Option<Progress>,
    pub speed: f64,
    pub search: SearchState,
    pub search_error: String,
    pub repos: Vec<HfRepo>,
    pub expanded: Option<String>,
    pub files_by_repo: HashMap<String, RepoFiles>,
    /// A Use/Start was sent and its refresh has not landed yet: the buttons that start a server stay off.
    pub starting: bool,
    /// Short, polite text for screen readers (the result list is not a live region).
    pub announcement: String,
    /// (time in ms, bytes) of the previous progress event, for the speed estimate.
    sample: Option<(f64, u64)>,
    search_seq: u64,
}

impl Default for ModelsModel {
    fn default() -> Self {
        Self::new()
    }
}

fn backend_name(b: Backend) -> &'static str {
    match b {
        Backend::Cuda => "CUDA",
        Backend::Vulkan => "Vulkan",
        Backend::Cpu => "CPU",
    }
}

impl ModelsModel {
    pub fn new() -> Self {
        Self {
            info: None,
            load_error: false,
            confirm_delete: None,
            chosen_backend: None,
            active_key: None,
            last: None,
            speed: 0.0,
            search: SearchState::Idle,
            search_error: String::new(),
            repos: Vec::new(),
            expanded: None,
            files_by_repo: HashMap::new(),
            starting: false,
            announcement: String::new(),
            sample: None,
            search_seq: 0,
        }
    }

    // ---- state ----

    pub fn refresh_ok(&mut self, info: ManagerInfo) {
        let downloading = info.downloading;
        self.info = Some(info);
        self.load_error = false;
        self.starting = false;
        if !downloading {
            self.active_key = None;
            self.last = None;
        }
    }

    pub fn refresh_failed(&mut self) {
        self.starting = false;
        if self.info.is_none() {
            self.load_error = true;
        }
    }

    /// Marks a download as running right away so buttons lock and the slot shows before the first progress event.
    pub fn start_download(&mut self, key: &str) {
        self.active_key = Some(key.into());
        self.last = None;
        self.speed = 0.0;
        self.sample = None;
        if let Some(i) = self.info.as_mut() {
            i.downloading = true;
        }
    }

    /// `now_ms` is any monotonic clock in milliseconds.
    pub fn on_progress(&mut self, p: Progress, now_ms: f64) {
        if !self.busy() {
            return;
        }
        if let Some((at, bytes)) = self.sample
            && now_ms > at
            && p.downloaded >= bytes
        {
            let inst = (p.downloaded - bytes) as f64 / (now_ms - at) * 1000.0;
            self.speed = if self.speed != 0.0 {
                self.speed * 0.7 + inst * 0.3
            } else {
                inst
            };
        }
        self.sample = Some((now_ms, p.downloaded));
        if self.active_key.is_none() {
            self.active_key = Some(if p.job == "runtime" {
                "runtime".into()
            } else {
                p.file.clone()
            });
        }
        self.last = Some(p);
    }

    pub fn busy(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.downloading)
    }

    pub fn running(&self) -> bool {
        self.info
            .as_ref()
            .is_some_and(|i| matches!(i.server, Status::Loading { .. } | Status::Ready { .. }))
    }

    pub fn downloading_key(&self, key: &str) -> bool {
        self.busy() && self.active_key.as_deref() == Some(key)
    }

    /// The model a Loading or Ready server runs.
    fn running_model(&self) -> Option<&str> {
        match &self.info.as_ref()?.server {
            Status::Loading { model } | Status::Ready { model, .. } => Some(model),
            _ => None,
        }
    }

    fn runtime_installed(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.runtime.is_some())
    }

    fn os(&self) -> &str {
        self.info.as_ref().map_or("", |i| i.os)
    }

    fn is_windows(&self) -> bool {
        self.os() == "windows"
    }

    fn recommended_backend(&self) -> Backend {
        self.info
            .as_ref()
            .map_or(Backend::Cpu, |i| i.backend.recommended)
    }

    fn installed_backend(&self) -> Option<Backend> {
        self.info.as_ref().and_then(|i| i.backend.installed)
    }

    // ---- status card ----

    pub fn status_pill(&self) -> Option<(Tone, &'static str)> {
        Some(match self.info.as_ref()?.server {
            Status::Stopped => (Tone::Neutral, "Stopped"),
            Status::Loading { .. } => (Tone::Warning, "Loading"),
            Status::Ready { .. } => (Tone::Success, "Ready"),
            Status::Error { .. } => (Tone::Danger, "Error"),
        })
    }

    pub fn status_name(&self) -> String {
        let Some(i) = &self.info else {
            return String::new();
        };
        let model = match &i.server {
            Status::Loading { model } | Status::Ready { model, .. } => Some(model.as_str()),
            _ => i.selected_model.as_deref(),
        };
        if i.server == Status::Stopped && model.is_none() {
            return "No model running".into();
        }
        self.model_name(model)
    }

    pub fn offload_text(&self) -> String {
        let Some(Status::Ready {
            offload: Some(o), ..
        }) = self.info.as_ref().map(|i| &i.server)
        else {
            return String::new();
        };
        match o.device {
            None => "Running on CPU".into(),
            Some(_) if o.layers >= o.total => {
                format!("Running on GPU: {}/{} layers", o.layers, o.total)
            }
            Some(_) => format!("Partly on GPU: {}/{} layers", o.layers, o.total),
        }
    }

    /// The model Start launches: the selected one, else the first installed.
    fn start_file(&self) -> Option<String> {
        let i = self.info.as_ref()?;
        i.selected_model
            .clone()
            .or_else(|| i.installed.first().map(|m| m.model.file.clone()))
    }

    /// The model Start launches, for the view.
    pub fn start_target(&self) -> Option<String> {
        self.start_file()
    }

    pub fn server_button(&self) -> Option<Button> {
        let i = self.info.as_ref()?;
        i.runtime.as_ref()?;
        Some(if self.running() {
            Button {
                label: "Stop".into(),
                enabled: true,
            }
        } else {
            Button {
                label: if matches!(i.server, Status::Error { .. }) {
                    "Try again"
                } else {
                    "Start"
                }
                .into(),
                enabled: self.start_file().is_some() && !self.starting,
            }
        })
    }

    pub fn status_hint(&self) -> Option<&'static str> {
        if !self.runtime_installed() || self.start_file().is_some() || self.running() {
            return None;
        }
        Some(if self.busy() {
            "Downloading..."
        } else {
            "Download a model first."
        })
    }

    pub fn install_button(&self) -> Option<Button> {
        let i = self.info.as_ref()?;
        i.runtime.is_none().then(|| Button {
            label: "Install llama.cpp".into(),
            enabled: !self.busy(),
        })
    }

    /// The build to install: only Windows has a choice.
    pub fn install_backend_arg(&self) -> Option<Backend> {
        self.is_windows().then(|| self.selected_backend())
    }

    pub fn cuda_on_cpu(&self) -> bool {
        let Some(i) = &self.info else { return false };
        i.backend.installed == Some(Backend::Cuda)
            && matches!(&i.server, Status::Ready { offload, .. } if offload.as_ref().is_none_or(|o| o.device.is_none()))
    }

    pub fn switch_vulkan_enabled(&self) -> bool {
        !self.busy()
    }

    pub fn error_message(&self) -> Option<String> {
        match &self.info.as_ref()?.server {
            Status::Error { message, .. } => Some(if message.is_empty() {
                "The server stopped unexpectedly.".into()
            } else {
                message.clone()
            }),
            _ => None,
        }
    }

    pub fn error_log(&self) -> Option<String> {
        match &self.info.as_ref()?.server {
            Status::Error { log, .. } if !log.is_empty() => Some(log.join("\n")),
            _ => None,
        }
    }

    pub fn autostart(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.autostart)
    }

    /// Using a model while a server runs stops that server first.
    /// Marks a Use/Start as sent; true when the caller should send it (false while one is already starting).
    pub fn begin_start(&mut self) -> bool {
        !std::mem::replace(&mut self.starting, true)
    }

    pub fn use_model_stops_first(&self) -> bool {
        self.running()
    }

    // ---- hardware ----

    pub fn gpu_line(&self) -> (String, String) {
        let Some(i) = &self.info else {
            return ("GPU".into(), String::new());
        };
        let h = &i.hardware;
        let label = if h.unified { "Chip" } else { "GPU" };
        let value = match &h.gpu {
            None => "None detected".into(),
            Some(g) if h.unified => g.name.clone(),
            Some(g) => format!("{}, {} VRAM", g.name, memory(g.vram)),
        };
        (label.into(), value)
    }

    pub fn memory_line(&self) -> (String, String) {
        let Some(i) = &self.info else {
            return ("Memory".into(), String::new());
        };
        let h = &i.hardware;
        let value = if h.unified {
            format!("{} unified", memory(h.ram))
        } else {
            format!("{} RAM", memory(h.ram))
        };
        ("Memory".into(), value)
    }

    pub fn budget_text(&self) -> String {
        let Some(i) = &self.info else {
            return String::new();
        };
        let free = i.budget.gpu as f64 - OVERHEAD as f64;
        if i.hardware.gpu.is_none() || i.budget.gpu == 0 {
            "No supported GPU found: models run on the CPU, which is slower.".into()
        } else if free > 0.0 {
            format!(
                "Models up to ~{:.1} GB run fully on your GPU.",
                (free / 1e8).floor() / 10.0
            )
        } else {
            "Your GPU has little free memory: models will run partly on the CPU.".into()
        }
    }

    pub fn runtime_text(&self) -> String {
        let rec = self.recommended_backend();
        if self.os() == "macos" {
            return "llama.cpp: Metal (built in)".into();
        }
        match self.installed_backend() {
            None => "llama.cpp: not installed".into(),
            Some(b) if b == rec => format!("llama.cpp: {}", backend_name(b)),
            Some(b) => format!(
                "llama.cpp: {} (recommended: {})",
                backend_name(b),
                backend_name(rec)
            ),
        }
    }

    pub fn selected_backend(&self) -> Backend {
        self.chosen_backend
            .unwrap_or_else(|| self.recommended_backend())
    }

    pub fn backend_label(&self, b: Backend) -> String {
        if b == self.recommended_backend() {
            format!("{} (recommended)", backend_name(b))
        } else {
            backend_name(b).into()
        }
    }

    pub fn choose_backend(&mut self, b: Backend) {
        self.chosen_backend = Some(b);
    }

    pub fn installed_note(&self) -> Option<String> {
        let inst = self.installed_backend()?;
        (self.is_windows() && inst != self.selected_backend())
            .then(|| format!("Installed: {}", backend_name(inst)))
    }

    pub fn reinstall_button(&self) -> Option<Button> {
        let i = self.info.as_ref()?;
        i.runtime.as_ref()?;
        if self.os() == "macos" {
            return None;
        }
        let inst = i.backend.installed;
        let gpu_changed = inst.is_some()
            && i.backend
                .installed_gpu
                .as_ref()
                .is_some_and(|g| Some(g) != i.hardware.gpu.as_ref().map(|g| &g.name));
        let needs = inst.is_some() && (inst != Some(i.backend.recommended) || gpu_changed);
        (needs || (self.is_windows() && Some(self.selected_backend()) != inst)).then(|| Button {
            label: "Reinstall".into(),
            enabled: !self.busy() && !self.running(),
        })
    }

    pub fn stop_to_reinstall_hint(&self) -> bool {
        self.running() && self.reinstall_button().is_some()
    }

    // ---- cards and rows ----

    pub fn fit_badge(&self, fit: Fit, too_big: &str) -> Badge {
        let cpu_only = self.info.as_ref().is_none_or(|i| i.budget.gpu == 0);
        match fit {
            Fit::Full => Badge {
                text: "Fits your GPU".into(),
                tone: Tone::Success,
            },
            Fit::Partial => Badge {
                text: if cpu_only {
                    "Runs on CPU, slower"
                } else {
                    "Partly on CPU, slower"
                }
                .into(),
                tone: Tone::Warning,
            },
            Fit::TooBig => Badge {
                text: too_big.into(),
                tone: Tone::Neutral,
            },
        }
    }

    pub fn text_badge(&self, text: &str) -> Badge {
        let t = text.to_lowercase();
        let tone = if t.starts_with("best quality") || t.starts_with("fastest") {
            Tone::Accent
        } else if t.starts_with("fits") {
            Tone::Success
        } else if t.starts_with("partly") || t.starts_with("runs on cpu") {
            Tone::Warning
        } else {
            Tone::Neutral
        };
        Badge {
            text: text.into(),
            tone,
        }
    }

    /// The catalog name of `file`, else the file name itself.
    pub fn model_name(&self, file: Option<&str>) -> String {
        let Some(file) = file else {
            return String::new();
        };
        self.info
            .as_ref()
            .and_then(|i| i.catalog.iter().find(|c| c.entry.file == file))
            .map_or_else(|| file.into(), |c| c.entry.name.clone())
    }

    pub fn recommended_cards(&self) -> Vec<Card> {
        let Some(i) = &self.info else {
            return Vec::new();
        };
        i.recommended
            .iter()
            .enumerate()
            .map(|(idx, r)| {
                let e = &r.entry;
                let installed = i.installed.iter().any(|m| m.model.file == e.file);
                let in_use = self.running_model() == Some(e.file.as_str());
                let action = if self.downloading_key(&e.file) {
                    CardAction::Progress
                } else if in_use {
                    CardAction::InUse
                } else if installed {
                    CardAction::Use {
                        enabled: i.runtime.is_some() && !self.starting,
                    }
                } else {
                    CardAction::Download {
                        primary: idx == 0,
                        enabled: !self.busy(),
                    }
                };
                let mut badges = vec![Badge {
                    text: size(e.size),
                    tone: Tone::Neutral,
                }];
                badges.extend(r.badges.iter().map(|b| self.text_badge(b)));
                Card {
                    id: e.id.clone(),
                    file: e.file.clone(),
                    name: e.name.clone(),
                    description: e.description.clone(),
                    note: e.note.clone(),
                    tag: idx == 0,
                    badges,
                    action,
                }
            })
            .collect()
    }

    pub fn another_download_hint(&self) -> bool {
        self.busy()
    }

    pub fn installed_rows(&self) -> Vec<InstalledItem> {
        let Some(i) = &self.info else {
            return Vec::new();
        };
        i.installed
            .iter()
            .map(|m| {
                let in_use = self.running_model() == Some(m.model.file.as_str());
                InstalledItem {
                    file: m.model.file.clone(),
                    name: self.model_name(Some(&m.model.file)),
                    size: size(m.model.size),
                    fit: self.fit_badge(m.fit, "Too big"),
                    in_use,
                    use_enabled: i.runtime.is_some() && !self.starting,
                    confirming: self.confirm_delete.as_deref() == Some(m.model.file.as_str()),
                    delete_enabled: !in_use,
                }
            })
            .collect()
    }

    pub fn ask_delete(&mut self, file: &str) {
        self.confirm_delete = Some(file.into());
    }

    pub fn cancel_delete(&mut self) {
        self.confirm_delete = None;
    }

    /// The file to delete, and the prompt is cleared.
    pub fn confirm_delete_click(&mut self) -> Option<String> {
        self.confirm_delete.take()
    }

    // ---- progress ----

    pub fn progress_text(&self) -> String {
        match &self.last {
            Some(p) => match p.total.filter(|t| *t > 0) {
                Some(t) => {
                    let rate = if self.speed > 0.0 {
                        format!(", {} MB/s", (self.speed / 1e6).round() as u64)
                    } else {
                        String::new()
                    };
                    format!("{} of {}{rate}", size(p.downloaded), size(t))
                }
                None => format!("{} downloaded", size(p.downloaded)),
            },
            None => "Starting download...".into(),
        }
    }

    /// 0..=1, or None while the total is unknown.
    pub fn progress_fraction(&self) -> Option<f32> {
        let p = self.last.as_ref()?;
        let t = p.total.filter(|t| *t > 0)?;
        Some((p.downloaded as f64 / t as f64).min(1.0) as f32)
    }

    // ---- Hugging Face search ----

    /// Starts a search for `query`; None (and idle) for a blank one. A newer call makes older results stale.
    pub fn begin_search(&mut self, query: &str) -> Option<u64> {
        self.search_seq += 1;
        if query.trim().is_empty() {
            self.search = SearchState::Idle;
            self.announcement.clear();
            return None;
        }
        self.search = SearchState::Searching;
        self.expanded = None;
        Some(self.search_seq)
    }

    /// False when `seq` is stale and the result was dropped.
    pub fn finish_search(&mut self, seq: u64, result: Result<Vec<HfRepo>, String>) -> bool {
        if seq != self.search_seq {
            return false;
        }
        match result {
            Ok(repos) => {
                self.announcement = if repos.is_empty() {
                    "No models found".into()
                } else {
                    format!("{} models found", repos.len())
                };
                self.search = SearchState::Done;
                self.repos = repos;
                self.search_error.clear();
            }
            Err(e) => {
                self.search = SearchState::Error;
                self.repos.clear();
                self.search_error = e;
                self.announcement = "Couldn't reach Hugging Face".into();
            }
        }
        true
    }

    pub fn search_view(&self, query: &str) -> SearchView {
        let q = query.trim();
        if self.search == SearchState::Idle || q.is_empty() {
            return SearchView::Hint;
        }
        match self.search {
            SearchState::Idle => SearchView::Hint,
            SearchState::Searching => SearchView::Searching,
            SearchState::Error => SearchView::Error {
                details: (!self.search_error.is_empty()).then(|| self.search_error.clone()),
            },
            SearchState::Done if self.repos.is_empty() => SearchView::NoResults(format!(
                "No models found for \"{q}\". Try a shorter or different name."
            )),
            SearchState::Done => SearchView::Results(
                self.repos
                    .iter()
                    .map(|r| {
                        let expanded = self.expanded.as_deref() == Some(r.id.as_str());
                        RepoRow {
                            id: r.id.clone(),
                            downloads: format!("{} downloads", compact(r.downloads)),
                            likes: format!("{} likes", compact(r.likes)),
                            expanded,
                            files: expanded.then(|| self.files_view(&r.id)),
                        }
                    })
                    .collect(),
            ),
        }
    }

    fn files_view(&self, repo: &str) -> FilesView {
        match self.files_by_repo.get(repo) {
            None | Some(RepoFiles::Loading) => FilesView::Loading,
            Some(RepoFiles::Error) => FilesView::Error,
            Some(RepoFiles::Loaded(rows)) if rows.is_empty() => FilesView::Empty,
            Some(RepoFiles::Loaded(rows)) => FilesView::Rows(
                rows.iter()
                    .map(|r| {
                        let f = &r.file;
                        let parts = if f.parts > 1 {
                            format!(", {} parts", f.parts)
                        } else {
                            String::new()
                        };
                        FileRow {
                            name: f.name.clone(),
                            label: if f.quant.is_empty() {
                                f.name.clone()
                            } else {
                                f.quant.clone()
                            },
                            size: format!("{}{parts}", size(f.size)),
                            fit: self.fit_badge(r.fit, "Too big for this computer"),
                            action: if self.downloading_key(&f.name) {
                                FileAction::Progress
                            } else {
                                FileAction::Download {
                                    enabled: !self.busy() && r.fit != Fit::TooBig,
                                }
                            },
                        }
                    })
                    .collect(),
            ),
        }
    }

    /// Expands or collapses `repo`; true when its files must be fetched (they are now Loading).
    pub fn toggle_repo(&mut self, repo: &str) -> bool {
        self.expanded = if self.expanded.as_deref() == Some(repo) {
            None
        } else {
            Some(repo.into())
        };
        if self.expanded.is_some()
            && !matches!(self.files_by_repo.get(repo), Some(RepoFiles::Loaded(_)))
        {
            self.files_by_repo.insert(repo.into(), RepoFiles::Loading);
            return true;
        }
        false
    }

    pub fn set_files(&mut self, repo: &str, result: Result<Vec<QuantRow>, String>) {
        self.files_by_repo.insert(
            repo.into(),
            match result {
                Ok(rows) => RepoFiles::Loaded(rows),
                Err(_) => RepoFiles::Error,
            },
        );
    }

    // ---- custom source ----

    /// The trimmed source to download; None for a blank one.
    pub fn custom_download(&mut self, source: &str) -> Option<String> {
        let source = source.trim();
        if source.is_empty() {
            return None;
        }
        self.start_download("custom");
        Some(source.into())
    }

    pub fn custom_enabled(&self) -> bool {
        !self.busy()
    }
}

// ---- commands ----

/// What the view asks the app to do on the core manager.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelsCommand {
    Use { file: String, stop_first: bool },
    Stop,
    Install(Option<Backend>),
    SwitchVulkan,
    DownloadCatalog(String),
    DownloadModel(String),
    Cancel,
    Delete(String),
    SetAutostart(bool),
    Search { seq: u64, query: String },
    Files(String),
}

impl ModelsCommand {
    /// The footer message when the command succeeds; None for commands that report nothing.
    pub fn done_message(&self) -> Option<&'static str> {
        Some(match self {
            Self::Use { .. } => MSG_STARTING,
            Self::Stop => MSG_STOPPED,
            Self::Install(_) => MSG_INSTALLED,
            Self::SwitchVulkan => MSG_VULKAN,
            Self::DownloadCatalog(_) | Self::DownloadModel(_) => MSG_DOWNLOADED,
            Self::Delete(_) => MSG_DELETED,
            Self::SetAutostart(_) => MSG_SAVED,
            Self::Cancel | Self::Search { .. } | Self::Files(_) => return None,
        })
    }
}

// ---- view ----

#[derive(Clone, Copy)]
struct Palette {
    fg: Hsla,
    muted_fg: Hsla,
    muted: Hsla,
    border: Hsla,
    card: Hsla,
    primary: Hsla,
    success: Hsla,
    warning: Hsla,
    danger: Hsla,
}

impl Palette {
    fn of(cx: &App) -> Self {
        let t = cx.theme();
        Self {
            fg: t.foreground,
            muted_fg: t.muted_foreground,
            muted: t.muted,
            border: t.border,
            card: t.group_box,
            primary: t.primary,
            success: t.success,
            warning: t.warning,
            danger: t.danger,
        }
    }

    /// (text, background) of a tone.
    fn tone(&self, tone: Tone) -> (Hsla, Hsla) {
        match tone {
            Tone::Neutral => (self.muted_fg, self.muted),
            Tone::Accent => (self.primary, self.primary.opacity(0.15)),
            Tone::Success => (self.success, self.success.opacity(0.15)),
            Tone::Warning => (self.warning, self.warning.opacity(0.18)),
            Tone::Danger => (self.danger, self.danger.opacity(0.15)),
        }
    }
}

fn card(p: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .p(px(16.0))
        .rounded(px(12.0))
        .border_1()
        .border_color(p.border)
        .bg(p.card)
}

fn row() -> Div {
    div().flex().items_center().gap(px(8.0))
}

fn small(p: &Palette, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.0))
        .text_color(p.muted_fg)
        .child(text.into())
}

fn empty(p: &Palette, text: impl Into<SharedString>) -> Div {
    div()
        .py(px(12.0))
        .text_center()
        .text_color(p.muted_fg)
        .child(text.into())
}

fn alert(p: &Palette, tone: Tone) -> Div {
    let (fg, bg) = p.tone(tone);
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .px(px(12.0))
        .py(px(8.0))
        .rounded(px(8.0))
        .text_size(px(12.0))
        .text_color(fg)
        .bg(bg)
}

fn badge(p: &Palette, b: &Badge) -> Div {
    let (fg, bg) = p.tone(b.tone);
    div()
        .px(px(8.0))
        .py(px(1.0))
        .rounded_full()
        .text_size(px(11.0))
        .text_color(fg)
        .bg(bg)
        .child(b.text.clone())
}

fn pill(p: &Palette, tone: Tone, text: &str) -> Div {
    let (fg, bg) = p.tone(tone);
    row()
        .gap(px(6.0))
        .px(px(8.0))
        .py(px(1.0))
        .rounded_full()
        .text_size(px(11.0))
        .text_color(fg)
        .bg(bg)
        .child(div().size(px(7.0)).rounded_full().bg(fg))
        .child(text.to_string())
}

fn heading(text: &'static str) -> Div {
    div()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_size(px(15.0))
        .child(text)
}

fn skeleton(p: &Palette, w: f32) -> Div {
    div().h(px(14.0)).w(px(w)).rounded(px(4.0)).bg(p.muted)
}

fn button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> UiButton {
    UiButton::new(id).label(label).small()
}

fn id(parts: &[&str]) -> SharedString {
    parts.join(":").into()
}

pub struct ModelsView {
    model: ModelsModel,
    query: Entity<InputState>,
    custom: Entity<InputState>,
    debounce: Option<Task<()>>,
    started: Instant,
    /// A custom download is running: its source is cleared from the field once it ends.
    custom_pending: bool,
    /// Set while rendering when a progress slot was drawn; a download with no slot gets one in the status card.
    slot_used: Cell<bool>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<ModelsCommand> for ModelsView {}

impl ModelsView {
    pub fn new(model: ModelsModel, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search Hugging Face for GGUF models, e.g. qwen")
        });
        let custom = cx.new(|cx| {
            InputState::new(window, cx).placeholder("owner/repo:quant or https URL to a .gguf")
        });
        let subs = vec![
            cx.subscribe_in(&query, window, |this, _, e: &InputEvent, window, cx| {
                if matches!(e, InputEvent::Change) {
                    this.schedule_search(window, cx);
                }
            }),
        ];
        Self {
            model,
            query,
            custom,
            debounce: None,
            started: Instant::now(),
            custom_pending: false,
            slot_used: Cell::new(false),
            _subs: subs,
        }
    }

    pub fn model(&self) -> &ModelsModel {
        &self.model
    }

    /// The result of `manager::status`.
    pub fn refreshed(
        &mut self,
        result: Result<ManagerInfo, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(info) => self.model.refresh_ok(info),
            Err(_) => self.model.refresh_failed(),
        }
        if self.custom_pending && !self.model.busy() {
            self.custom_pending = false;
            self.custom.update(cx, |c, cx| c.set_value("", window, cx));
        }
        cx.notify();
    }

    pub fn progress(&mut self, p: Progress, cx: &mut Context<Self>) {
        let now = self.started.elapsed().as_secs_f64() * 1000.0;
        self.model.on_progress(p, now);
        cx.notify();
    }

    pub fn searched(
        &mut self,
        seq: u64,
        result: Result<Vec<HfRepo>, String>,
        cx: &mut Context<Self>,
    ) {
        if self.model.finish_search(seq, result) {
            cx.notify();
        }
    }

    pub fn files_loaded(
        &mut self,
        repo: &str,
        result: Result<Vec<QuantRow>, String>,
        cx: &mut Context<Self>,
    ) {
        self.model.set_files(repo, result);
        cx.notify();
    }

    fn schedule_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.debounce = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            this.update_in(cx, |this, _, cx| this.run_search(cx)).ok();
        }));
    }

    fn run_search(&mut self, cx: &mut Context<Self>) {
        let q = self.query.read(cx).value().trim().to_string();
        if let Some(seq) = self.model.begin_search(&q) {
            cx.emit(ModelsCommand::Search { seq, query: q });
        }
        cx.notify();
    }

    fn use_model(&mut self, file: String, cx: &mut Context<Self>) {
        let stop_first = self.model.use_model_stops_first();
        if self.model.begin_start() {
            cx.emit(ModelsCommand::Use { stop_first, file });
            cx.notify();
        }
    }

    fn download(&mut self, key: &str, command: ModelsCommand, cx: &mut Context<Self>) {
        self.model.start_download(key);
        cx.emit(command);
        cx.notify();
    }

    fn install(&mut self, backend: Option<Backend>, cx: &mut Context<Self>) {
        self.download("runtime", ModelsCommand::Install(backend), cx);
    }

    fn toggle_repo(&mut self, repo: String, cx: &mut Context<Self>) {
        if self.model.toggle_repo(&repo) {
            cx.emit(ModelsCommand::Files(repo));
        }
        cx.notify();
    }

    /// The progress bar, text and Cancel of the running download.
    fn progress_slot(&self, p: &Palette, cx: &mut Context<Self>) -> Div {
        self.slot_used.set(true);
        let bar = match self.model.progress_fraction() {
            Some(f) => UiProgress::new("dl-progress").value(f * 100.0),
            None => UiProgress::new("dl-progress").loading(true),
        };
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(bar)
            .child(
                row()
                    .child(div().flex_1().child(small(p, self.model.progress_text())))
                    .child(
                        button("dl-cancel", "Cancel")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ModelsCommand::Cancel))),
                    ),
            )
    }

    fn status_card(&self, p: &Palette, cx: &mut Context<Self>) -> Div {
        let m = &self.model;
        let Some(info) = &m.info else {
            return card(p).child(if m.load_error {
                alert(p, Tone::Danger).child(
                    "Couldn't load the model manager. Close and reopen Settings to try again.",
                )
            } else {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(skeleton(p, 220.0))
                    .child(skeleton(p, 120.0))
            });
        };
        let mut head = row();
        if let Some((tone, text)) = m.status_pill() {
            head = head.child(pill(p, tone, text));
        }
        head = head
            .child(
                div()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(m.status_name()),
            )
            .child(small(p, m.offload_text()))
            .child(div().flex_1());
        if let Some(hint) = m.status_hint() {
            head = head.child(small(p, hint));
        }
        if let Some(b) = m.server_button() {
            let start = m.start_target();
            let running = m.running();
            let mut btn = button("server-ctl", b.label).disabled(!b.enabled);
            if !running {
                btn = btn.primary();
            }
            head = head.child(btn.on_click(cx.listener(move |this, _, _, cx| {
                if running {
                    cx.emit(ModelsCommand::Stop);
                } else if let Some(file) = start.clone() {
                    this.use_model(file, cx);
                }
            })));
        }
        let mut c = card(p).child(head);
        if let Some(b) = m.install_button() {
            let mut a =
                alert(p, Tone::Warning).child(
                    row()
                        .child(div().flex_1().child(
                            "llama.cpp is not installed. It runs the models on your computer.",
                        ))
                        .child(if m.downloading_key("runtime") {
                            div().w(px(220.0)).child(self.progress_slot(p, cx))
                        } else {
                            div().child(
                                button("install-rt", b.label)
                                    .primary()
                                    .disabled(!b.enabled)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let arg = this.model.install_backend_arg();
                                        this.install(arg, cx);
                                    })),
                            )
                        }),
                );
            if m.os() == "windows" {
                a = a.child(self.backend_picker(p, cx));
            }
            c = c.child(a);
        }
        if let Some(note) = &info.fallback_note {
            c = c.child(alert(p, Tone::Neutral).child(note.clone()));
        }
        if m.cuda_on_cpu() {
            c = c.child(
                alert(p, Tone::Warning).child(
                    row()
                        .child(
                            div()
                                .flex_1()
                                .child("The CUDA build is running on the CPU."),
                        )
                        .child(
                            button("vulkan", "Switch to Vulkan")
                                .disabled(!m.switch_vulkan_enabled())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.download("runtime", ModelsCommand::SwitchVulkan, cx);
                                })),
                        ),
                ),
            );
        }
        if let Some(msg) = m.error_message() {
            let mut a = alert(p, Tone::Danger).child(msg);
            if let Some(log) = m.error_log() {
                a = a.child(
                    div()
                        .max_h(px(160.0))
                        .overflow_hidden()
                        .font_family("Menlo")
                        .text_size(px(11.0))
                        .child(log),
                );
            }
            c = c.child(a);
        }
        c = c.child(
            Checkbox::new("autostart")
                .label("Start the server when the app launches")
                .checked(m.autostart())
                .on_click(cx.listener(|this, v: &bool, _, cx| {
                    if let Some(i) = this.model.info.as_mut() {
                        i.autostart = *v;
                    }
                    cx.emit(ModelsCommand::SetAutostart(*v));
                    cx.notify();
                })),
        );
        c
    }

    /// Windows only: which llama.cpp build to install.
    fn backend_picker(&self, p: &Palette, cx: &mut Context<Self>) -> Div {
        let selected = self.model.selected_backend();
        let mut r = row().child(small(p, "Build"));
        for b in [Backend::Cuda, Backend::Vulkan, Backend::Cpu] {
            r = r.child(
                button(
                    id(&["backend", backend_name(b)]),
                    self.model.backend_label(b),
                )
                .selected(b == selected)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.model.choose_backend(b);
                    cx.notify();
                })),
            );
        }
        r
    }

    fn hardware_card(&self, p: &Palette, cx: &mut Context<Self>) -> Div {
        let m = &self.model;
        let mut c = card(p).child(heading("Your hardware"));
        if m.info.is_none() {
            return c
                .child(skeleton(p, 260.0))
                .child(skeleton(p, 200.0))
                .child(skeleton(p, 320.0));
        }
        let (gl, gv) = m.gpu_line();
        let (ml, mv) = m.memory_line();
        let kv = |k: String, v: String| {
            row()
                .child(div().w(px(70.0)).text_color(p.muted_fg).child(k))
                .child(div().child(v))
        };
        c = c
            .child(div().flex().flex_col().child(kv(gl, gv)).child(kv(ml, mv)))
            .child(div().child(m.budget_text()));
        let win = m.is_windows();
        let mut r = row().child(div().text_color(p.muted_fg).child(if win {
            "llama.cpp build".to_string()
        } else {
            m.runtime_text()
        }));
        if win {
            r = r.child(self.backend_picker(p, cx));
        }
        if let Some(note) = m.installed_note() {
            r = r.child(small(p, note));
        }
        if let Some(b) = m.reinstall_button() {
            r = r.child(
                button("reinstall", b.label)
                    .disabled(!b.enabled)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let arg = this.model.install_backend_arg();
                        this.install(arg, cx);
                    })),
            );
        }
        if m.downloading_key("runtime") && m.runtime_installed() {
            r = r.child(div().flex_1().child(self.progress_slot(p, cx)));
        }
        c = c.child(r);
        if m.stop_to_reinstall_hint() {
            c = c.child(small(p, "Stop the server to reinstall."));
        }
        c
    }

    fn recommended(&self, p: &Palette, cx: &mut Context<Self>) -> Div {
        let m = &self.model;
        let body = if m.info.is_none() {
            let mut g = row().items_start();
            for _ in 0..3 {
                g = g.child(div().flex_1().h(px(150.0)).rounded(px(10.0)).bg(p.muted));
            }
            div().child(g)
        } else {
            let cards = m.recommended_cards();
            if cards.is_empty() {
                div().child(empty(p, EMPTY_RECOMMENDED))
            } else {
                let mut grid = div().flex().flex_wrap().gap(px(12.0));
                for c in cards {
                    grid = grid.child(self.model_card(p, c, cx));
                }
                let mut d = div().flex().flex_col().gap(px(8.0)).child(grid);
                if m.another_download_hint() {
                    d = d.child(small(p, "Another download is in progress."));
                }
                d
            }
        };
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(heading("Recommended for you"))
            .child(body)
    }

    fn model_card(&self, p: &Palette, c: Card, cx: &mut Context<Self>) -> Div {
        let mut title = row().flex_wrap().child(
            div()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(c.name.clone()),
        );
        if c.tag {
            title = title.child(badge(
                p,
                &Badge {
                    text: "Recommended".into(),
                    tone: Tone::Accent,
                },
            ));
        }
        let mut badges = div().flex().flex_wrap().gap(px(4.0));
        for b in &c.badges {
            badges = badges.child(badge(p, b));
        }
        let (file, cid) = (c.file.clone(), c.id.clone());
        let action: AnyElement = match c.action {
            CardAction::Progress => self.progress_slot(p, cx).into_any_element(),
            CardAction::InUse => pill(p, Tone::Success, "In use").into_any_element(),
            CardAction::Use { enabled } => button(id(&["use", &c.file]), "Use")
                .disabled(!enabled)
                .on_click(cx.listener(move |this, _, _, cx| this.use_model(file.clone(), cx)))
                .into_any_element(),
            CardAction::Download { primary, enabled } => {
                let mut b = button(id(&["dl", &c.id]), "Download").disabled(!enabled);
                if primary {
                    b = b.primary();
                }
                b.on_click(cx.listener(move |this, _, _, cx| {
                    this.download(&file, ModelsCommand::DownloadCatalog(cid.clone()), cx);
                }))
                .into_any_element()
            }
        };
        let mut d = div()
            .w(px(200.0))
            .flex_grow(1.0)
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(12.0))
            .rounded(px(10.0))
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .child(title)
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.0))
                    .text_color(p.muted_fg)
                    .child(c.description),
            );
        if let Some(note) = c.note {
            d = d.child(
                div()
                    .text_size(px(12.0))
                    .text_color(p.muted_fg)
                    .italic()
                    .child(note),
            );
        }
        d.child(badges).child(div().child(action))
    }

    fn installed(&self, p: &Palette, cx: &mut Context<Self>) -> Div {
        let m = &self.model;
        let mut list = div().flex().flex_col().gap(px(8.0));
        if m.info.is_none() {
            list = list.child(skeleton(p, 300.0)).child(skeleton(p, 240.0));
        } else if m.installed_rows().is_empty() {
            list = list.child(empty(p, EMPTY_INSTALLED));
        } else {
            if !m.runtime_installed() {
                list = list.child(small(p, "Install llama.cpp to use these models."));
            }
            for r in m.installed_rows() {
                list = list.child(self.installed_row(p, r, cx));
            }
        }
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(heading("Installed"))
            .child(list)
    }

    fn installed_row(&self, p: &Palette, r: InstalledItem, cx: &mut Context<Self>) -> Div {
        let file = r.file.clone();
        let mut d = row()
            .px(px(12.0))
            .py(px(8.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .child(div().flex_1().child(r.name))
            .child(small(p, r.size))
            .child(badge(p, &r.fit));
        if r.in_use {
            d = d
                .child(pill(p, Tone::Success, "In use"))
                .child(small(p, "Stop the server to delete."));
        } else {
            let f = file.clone();
            d = d.child(
                button(id(&["use", &file]), "Use")
                    .disabled(!r.use_enabled)
                    .on_click(cx.listener(move |this, _, _, cx| this.use_model(f.clone(), cx))),
            );
        }
        if r.confirming {
            let f = file.clone();
            d = d
                .child(small(p, "Delete?"))
                .child(
                    button(id(&["del", &file]), "Confirm")
                        .danger()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let _ = &f;
                            if let Some(file) = this.model.confirm_delete_click() {
                                cx.emit(ModelsCommand::Delete(file));
                            }
                            cx.notify();
                        })),
                )
                .child(
                    button(id(&["cancel-del", &file]), "Cancel").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.model.cancel_delete();
                            cx.notify();
                        },
                    )),
                );
        } else {
            let f = file.clone();
            d = d.child(
                button(id(&["del", &file]), "Delete")
                    .danger()
                    .disabled(!r.delete_enabled)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.ask_delete(&f);
                        cx.notify();
                    })),
            );
        }
        d
    }

    fn search(&self, p: &Palette, cx: &mut Context<Self>) -> Div {
        let query = self.query.read(cx).value().to_string();
        let body: AnyElement = match self.model.search_view(&query) {
            SearchView::Hint => empty(p, SEARCH_HINT).into_any_element(),
            SearchView::Searching => empty(p, "Searching...").into_any_element(),
            SearchView::Error { details } => {
                let mut a = alert(p, Tone::Danger).child(
                    "Couldn't reach Hugging Face. Check your internet connection and try again.",
                );
                if let Some(d) = details {
                    a = a.child(div().font_family("Menlo").text_size(px(11.0)).child(d));
                }
                a.into_any_element()
            }
            SearchView::NoResults(t) => empty(p, t).into_any_element(),
            SearchView::Results(rows) => {
                let mut list = div().flex().flex_col().gap(px(8.0));
                for r in rows {
                    list = list.child(self.repo_row(p, r, cx));
                }
                list.into_any_element()
            }
        };
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(heading("Search Hugging Face"))
            .child(Input::new(&self.query).cleanable(true))
            .child(body)
    }

    fn repo_row(&self, p: &Palette, r: RepoRow, cx: &mut Context<Self>) -> Div {
        let repo = r.id.clone();
        let head = row()
            .id(id(&["repo", &r.id]))
            .cursor_pointer()
            .px(px(12.0))
            .py(px(8.0))
            .hover(|s| s.bg(p.muted))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_repo(repo.clone(), cx)))
            .child(
                div()
                    .flex_1()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(r.id.clone()),
            )
            .child(small(p, r.downloads))
            .child(small(p, r.likes));
        let mut d = div()
            .flex()
            .flex_col()
            .rounded(px(8.0))
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .child(head);
        if let Some(files) = r.files {
            let mut body = div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .px(px(12.0))
                .pb(px(8.0));
            match files {
                FilesView::Loading => body = body.child(small(p, "Loading files...")),
                FilesView::Error => {
                    body = body.child(
                        alert(p, Tone::Danger)
                            .child("Couldn't load the files for this model. Try again later."),
                    );
                }
                FilesView::Empty => body = body.child(small(p, EMPTY_FILES)),
                FilesView::Rows(rows) => {
                    for f in rows {
                        body = body.child(self.file_row(p, &r.id, f, cx));
                    }
                }
            }
            d = d.child(body);
        }
        d
    }

    fn file_row(&self, p: &Palette, repo: &str, f: FileRow, cx: &mut Context<Self>) -> Div {
        let source = format!("{repo}:{}", f.name);
        let name = f.name.clone();
        let action: AnyElement = match f.action {
            FileAction::Progress => div()
                .w(px(220.0))
                .child(self.progress_slot(p, cx))
                .into_any_element(),
            FileAction::Download { enabled } => button(id(&["dl-q", &f.name]), "Download")
                .disabled(!enabled)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.download(&name, ModelsCommand::DownloadModel(source.clone()), cx);
                }))
                .into_any_element(),
        };
        row()
            .child(div().flex_1().font_family("Menlo").child(f.label))
            .child(small(p, f.size))
            .child(badge(p, &f.fit))
            .child(action)
    }

    fn custom_source(&self, p: &Palette, cx: &mut Context<Self>) -> Div {
        let mut d = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Custom model"),
            )
            .child(
                row()
                    .child(div().flex_1().min_w_0().child(Input::new(&self.custom)))
                    .child(
                        button("dl-custom", "Download")
                            .disabled(!self.model.custom_enabled())
                            .on_click(cx.listener(|this, _, _, cx| {
                                let source = this.custom.read(cx).value().to_string();
                                if let Some(source) = this.model.custom_download(&source) {
                                    this.custom_pending = true;
                                    cx.emit(ModelsCommand::DownloadModel(source));
                                }
                                cx.notify();
                            })),
                    ),
            );
        if self.model.downloading_key("custom") {
            d = d.child(self.progress_slot(p, cx));
        }
        d
    }
}

impl Render for ModelsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::of(cx);
        self.slot_used.set(false);
        // Sections that can hold the progress slot first, so the status card knows whether it must show one.
        let hardware = self.hardware_card(&p, cx);
        let recommended = self.recommended(&p, cx);
        let installed = self.installed(&p, cx);
        let search = self.search(&p, cx);
        let custom = self.custom_source(&p, cx);
        let mut status = self.status_card(&p, cx);
        if self.model.busy() && !self.slot_used.get() {
            // A download whose key matches no slot (split parts, a refresh mid-start) still needs progress and Cancel.
            status = status.child(self.progress_slot(&p, cx));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .text_color(p.fg)
            .child(status)
            .child(hardware)
            .child(recommended)
            .child(installed)
            .child(card(&p).child(search).child(custom))
    }
}
