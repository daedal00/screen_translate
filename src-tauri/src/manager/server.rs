//! The managed llama-server child process: arguments, status state machine, health polling.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::sync::mpsc;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::runtime::Backend;

pub const HOST: &str = "127.0.0.1";
/// Fixed and away from llama-server's default 8080, so a user's own server keeps working beside it.
pub const PORT: u16 = 18080;
pub const CONTEXT: u32 = 4096;
pub const LOG_LINES: usize = 20;
const MAX_LINE_BYTES: usize = 1000;
/// Longest chunk a reader buffers before treating it as a line (a stream without newlines stays bounded).
const MAX_READ_BYTES: u64 = 8192;
/// How long to wait for the output readers after the child is gone (a leaked pipe must not hold up the status).
const READER_GRACE: Duration = Duration::from_millis(500);
pub const LOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// `http://127.0.0.1:18080`, the URL the Local provider uses for the managed server.
pub fn base_url() -> String {
    format!("http://{HOST}:{PORT}")
}

/// GPU flags for `os` given the installed backend (None: unknown, e.g. a llama-server from PATH). The bundled
/// build defaults to `-ngl auto --fit on`, which fits the model against the device's free memory, so no flag
/// forces offloading. Only a recorded CPU install on Windows turns it off (`-ngl 0`); CUDA gets `-fa on`.
pub fn gpu_args(os: &str, backend: Option<Backend>) -> Vec<String> {
    match backend {
        Some(Backend::Cpu) if os == "windows" => vec!["-ngl".into(), "0".into()],
        Some(Backend::Cuda) => vec!["-fa".into(), "on".into()],
        _ => vec![],
    }
}

/// Extra flags for model `file`: Qwen3 (and 3.5) and Gemma 4 think by default, which is wasted time for translation.
pub fn model_args(file: &str) -> Vec<String> {
    let f = file.to_ascii_lowercase();
    if f.starts_with("qwen3") || f.starts_with("gemma-4") {
        vec!["--reasoning-budget".into(), "0".into()]
    } else {
        vec![]
    }
}

/// `["-m", <model>, "--host", HOST, "--port", PORT, "-c", CONTEXT, "-lv", "4"]` then `gpu`. Verbosity 4 is
/// needed: the bundled build only logs which device and how many layers it offloaded at that level.
pub fn server_args(model: &Path, gpu: &[String]) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        "-m".into(),
        model.into(),
        "--host".into(),
        HOST.into(),
        "--port".into(),
        PORT.to_string().into(),
        "-c".into(),
        CONTEXT.to_string().into(),
        "-lv".into(),
        "4".into(),
    ];
    args.extend(gpu.iter().map(OsString::from));
    args
}

/// Where the model was loaded, from llama-server's startup log.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Offload {
    /// GPU name, e.g. "Apple M1 Pro".
    pub device: Option<String>,
    pub layers: u32,
    pub total: u32,
}

/// Updates `acc` from one log line: `... using device MTL0 (Apple M1 Pro) (...)` sets the device,
/// `... offloaded 33/33 layers to GPU` sets the layer counts. Other lines change nothing.
pub fn parse_offload_line(line: &str, acc: &mut Option<Offload>) {
    let new = || Offload { device: None, layers: 0, total: 0 };
    if let Some(rest) = line.split_once("using device ").map(|(_, r)| r)
        && let Some((_, rest)) = rest.split_once(" (")
        && let Some((name, _)) = rest.split_once(')')
    {
        acc.get_or_insert_with(new).device = Some(name.to_string());
    } else if let Some(rest) = line.split_once("offloaded ").map(|(_, r)| r)
        && let Some((counts, tail)) = rest.split_once(' ')
        && tail.starts_with("layers to GPU")
        && let Some((layers, total)) = counts.split_once('/')
        && let (Ok(layers), Ok(total)) = (layers.parse(), total.parse())
    {
        let o = acc.get_or_insert_with(new);
        (o.layers, o.total) = (layers, total);
    }
}

/// How the child ended, as far as the log shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exit {
    pub code: Option<i32>,
    /// The log named a GPU device (`using device CUDA0 (...)`), so the GPU runtime did come up.
    pub device_seen: bool,
}

/// Windows exit code of a process whose DLL (the CUDA runtime) is missing.
const STATUS_DLL_NOT_FOUND: i32 = -1073741515;

/// True when a CUDA build exited while loading and CUDA itself looks to blame: no GPU device was ever named in
/// its log, or the exit was a missing DLL. Any other failure (bad GGUF, out of memory) keeps the CUDA build.
pub fn should_fallback_to_vulkan(os: &str, backend: Backend, status_before: &Status, exit: Option<&Exit>) -> bool {
    os == "windows"
        && backend == Backend::Cuda
        && matches!(status_before, Status::Loading { .. })
        && exit.is_some_and(|e| !e.device_seen || e.code == Some(STATUS_DLL_NOT_FOUND))
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Status {
    Stopped,
    Loading { model: String },
    /// `offload` is where the model was loaded, when the log said so.
    Ready { model: String, offload: Option<Offload> },
    /// `log` is raw child output (it can echo prompt text): send it to the UI, never to `log::` or `Debug` output.
    Error { message: String, log: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The child was spawned for `model` (a display name, e.g. the file name).
    Started { model: String },
    /// One /health poll: `Some(http status)` or `None` when the request failed (not listening yet).
    Health(Option<u16>),
    /// The child exited on its own (not via `Stopped`).
    Exited { code: Option<i32>, log: Vec<String> },
    /// Still not ready after `LOAD_TIMEOUT`.
    TimedOut { log: Vec<String> },
    /// The user (or app quit) stopped it.
    Stopped,
}

/// Pure transition function:
/// - `Started` from any state -> Loading.
/// - `Health(Some(200))` in Loading -> Ready; other Health results leave the state unchanged
///   (503 while loading, connection refused before it binds). Health in Stopped/Error is ignored.
/// - `Exited` in Loading/Ready -> Error "llama-server exited (code N)" / "llama-server exited" with the log;
///   ignored in Stopped/Error.
/// - `TimedOut` in Loading -> Error "llama-server did not become ready in time" with the log; ignored otherwise.
/// - `Stopped` from any state -> Stopped.
pub fn next(status: &Status, event: Event) -> Status {
    match (event, status) {
        (Event::Started { model }, _) => Status::Loading { model },
        (Event::Health(Some(200)), Status::Loading { model }) => Status::Ready { model: model.clone(), offload: None },
        (Event::Exited { code, log }, Status::Loading { .. } | Status::Ready { .. }) => Status::Error {
            message: match code {
                Some(c) => format!("llama-server exited (code {c})"),
                None => "llama-server exited".into(),
            },
            log,
        },
        (Event::TimedOut { log }, Status::Loading { .. }) => {
            Status::Error { message: "llama-server did not become ready in time".into(), log }
        }
        (Event::Stopped, _) => Status::Stopped,
        _ => status.clone(),
    }
}

/// True for a llama.cpp log line tagged info: `<timestamp> I <text>`.
fn is_info(line: &str) -> bool {
    let mut t = line.split_whitespace();
    matches!((t.next(), t.next()), (Some(ts), Some("I")) if ts.starts_with(|c: char| c.is_ascii_digit()))
}

/// The last `cap` lines of child output, in memory only (never written to the app log: it can echo prompts).
pub struct LogTail {
    cap: usize,
    lines: VecDeque<String>,
    /// Parsed from every pushed line, so it survives the tail dropping old lines.
    offload: Option<Offload>,
    /// Once set, info-level lines are not kept: at `-lv 4` every request logs dozens of them and would push the
    /// lines that explain a later crash out of the tail.
    ready: bool,
}

impl LogTail {
    pub fn new(cap: usize) -> Self {
        Self { cap, lines: VecDeque::new(), offload: None, ready: false }
    }

    /// Trims trailing whitespace; skips empty lines; keeps at most `MAX_LINE_BYTES` bytes of a line (cut on a char boundary).
    /// Every line is also scanned for offload info.
    pub fn push(&mut self, line: &str) {
        let line = line.trim_end();
        if line.is_empty() {
            return;
        }
        parse_offload_line(line, &mut self.offload);
        if self.ready && is_info(line) {
            return;
        }
        let line = &line[..line.floor_char_boundary(MAX_LINE_BYTES)];
        self.lines.push_back(line.to_string());
        while self.lines.len() > self.cap {
            self.lines.pop_front();
        }
    }

    /// From now on only lines that are not info-level are kept.
    pub fn mark_ready(&mut self) {
        self.ready = true;
    }

    pub fn offload(&self) -> Option<Offload> {
        self.offload.clone()
    }

    pub fn lines(&self) -> Vec<String> {
        self.lines.iter().cloned().collect()
    }
}

type OnChange = Arc<dyn Fn(Status) + Send + Sync>;

struct Inner {
    status: Status,
    child: Option<Child>,
    /// Bumped on every start and stop, so a monitor thread from an old run goes quiet.
    generation: u64,
    on_change: Option<OnChange>,
    /// How the last child ended on its own; set just before the Error status is notified.
    exit: Option<Exit>,
}

struct Core {
    inner: Mutex<Inner>,
    /// Serializes notifications so the last callback always reports the final status.
    notify: Mutex<()>,
}

/// Calls `on_change` with the status read now, never while holding the state lock.
fn notify(core: &Core) {
    let _order = core.notify.lock().unwrap();
    let (cb, status) = {
        let g = core.inner.lock().unwrap();
        (g.on_change.clone(), g.status.clone())
    };
    if let Some(cb) = cb {
        cb(status);
    }
}

/// Windows: a job object that kills its processes when the app exits for any reason, so a crash or
/// logoff cannot leave llama-server holding the port and GPU memory.
#[cfg(windows)]
struct Job(windows::Win32::Foundation::HANDLE);

#[cfg(windows)]
unsafe impl Send for Job {}
#[cfg(windows)]
unsafe impl Sync for Job {}

#[cfg(windows)]
impl Job {
    fn new() -> Option<Job> {
        use windows::Win32::System::JobObjects::*;
        use windows::core::PCWSTR;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        unsafe {
            let handle = CreateJobObjectW(None, PCWSTR::null()).ok()?;
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .ok()?;
            Some(Job(handle))
        }
    }

    fn assign(&self, child: &Child) {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::JobObjects::AssignProcessToJobObject;
        // Best effort: the child is still stopped explicitly on stop().
        let _ = unsafe { AssignProcessToJobObject(self.0, HANDLE(child.as_raw_handle())) };
    }
}

/// Handle to at most one managed llama-server. Cheap to share (`Arc` inside); `Send + Sync`.
pub struct Server {
    core: Arc<Core>,
    #[cfg(windows)]
    job: Option<Job>,
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

/// Applies `event` unless a newer run or a stop superseded `generation`; notifies on change.
/// A Loading -> Ready transition takes the offload info from `log`.
fn apply(core: &Core, generation: u64, log: &Mutex<LogTail>, event: Event) -> bool {
    {
        let mut g = core.inner.lock().unwrap();
        if g.generation != generation {
            return false;
        }
        let mut new = next(&g.status, event);
        if let (Status::Loading { .. }, Status::Ready { offload, .. }) = (&g.status, &mut new) {
            let mut tail = log.lock().unwrap();
            tail.mark_ready();
            *offload = tail.offload();
        }
        if new == g.status {
            return true;
        }
        g.status = new;
    }
    notify(core);
    true
}

/// Reads `reader` into `log` line by line (at most `MAX_READ_BYTES` per line) and signals `done` at EOF.
fn pump(reader: impl Read + Send + 'static, log: Arc<Mutex<LogTail>>, done: mpsc::Sender<()>) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match (&mut reader).take(MAX_READ_BYTES).read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => log.lock().unwrap().push(&String::from_utf8_lossy(&buf)),
            }
        }
        let _ = done.send(());
    });
}

/// Waits (bounded) for `count` readers to finish, so the log is complete when the child's pipes close.
fn wait_readers(done: &mpsc::Receiver<()>, count: usize) {
    let deadline = Instant::now() + READER_GRACE;
    for _ in 0..count {
        if done.recv_timeout(deadline.saturating_duration_since(Instant::now())).is_err() {
            break;
        }
    }
}

impl Server {
    pub fn new() -> Self {
        let inner = Inner { status: Status::Stopped, child: None, generation: 0, on_change: None, exit: None };
        Self {
            core: Arc::new(Core { inner: Mutex::new(inner), notify: Mutex::new(()) }),
            #[cfg(windows)]
            job: Job::new(),
        }
    }

    pub fn status(&self) -> Status {
        self.core.inner.lock().unwrap().status.clone()
    }

    /// Spawns `exe args` (stdin null; stdout and stderr piped into the LogTail by reader threads; on
    /// windows with CREATE_NO_WINDOW and inside a kill-on-close job object), sets Loading, and starts a
    /// monitor thread that every `poll` checks `try_wait` (-> Exited) and GETs `health_url` with `client`
    /// (-> Health), and sends TimedOut after `load_timeout` in Loading. It stops once the state is
    /// Stopped/Error or a newer run started. `on_change` gets every status change (from any thread, one
    /// at a time, always with the status current at call time); it must not call `start` or `stop`.
    /// Err if a child is already running (Loading/Ready), `PORT` is already taken (an orphaned
    /// llama-server, say), or the spawn fails (status becomes Error in the last two cases).
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &self,
        exe: &Path,
        args: Vec<OsString>,
        model: String,
        health_url: String,
        client: reqwest::Client,
        poll: Duration,
        load_timeout: Duration,
        on_change: impl Fn(Status) + Send + Sync + 'static,
    ) -> Result<(), String> {
        let mut g = self.core.inner.lock().unwrap();
        if matches!(g.status, Status::Loading { .. } | Status::Ready { .. }) {
            return Err("llama-server is already running".into());
        }
        g.generation += 1;
        let generation = g.generation;
        g.exit = None;
        g.on_change = Some(Arc::new(on_change));

        let mut cmd = Command::new(exe);
        cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        let addr = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, PORT));
        let spawned = match std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)) {
            Ok(_) => Err(format!("Port {PORT} is already in use (a previous llama-server may still be running)")),
            Err(_) => cmd.spawn().map_err(|e| format!("Cannot start llama-server: {e}")),
        };
        let mut child = match spawned {
            Ok(c) => c,
            Err(message) => {
                g.status = Status::Error { message: message.clone(), log: Vec::new() };
                drop(g);
                notify(&self.core);
                return Err(message);
            }
        };
        #[cfg(windows)]
        if let Some(job) = &self.job {
            job.assign(&child);
        }
        let log = Arc::new(Mutex::new(LogTail::new(LOG_LINES)));
        let (done_tx, done_rx) = mpsc::channel();
        let mut readers = 0;
        if let Some(r) = child.stdout.take() {
            pump(r, log.clone(), done_tx.clone());
            readers += 1;
        }
        if let Some(r) = child.stderr.take() {
            pump(r, log.clone(), done_tx.clone());
            readers += 1;
        }
        g.child = Some(child);
        g.status = next(&g.status, Event::Started { model });
        drop(g);
        notify(&self.core);

        let core = self.core.clone();
        std::thread::spawn(move || monitor(core, generation, (done_rx, readers), log, health_url, client, poll, load_timeout));
        Ok(())
    }

    /// Counter bumped by every start and stop: lets a caller tell that the user acted since it captured it.
    pub fn generation(&self) -> u64 {
        self.core.inner.lock().unwrap().generation
    }

    /// How the child of the current run exited on its own, if it did.
    pub fn last_exit(&self) -> Option<Exit> {
        self.core.inner.lock().unwrap().exit.clone()
    }

    /// Kills and reaps the child if any, then Stopped (on_change of the current run is notified).
    /// Safe to call when nothing runs. Used on app quit too.
    pub fn stop(&self) {
        {
            let mut g = self.core.inner.lock().unwrap();
            g.generation += 1;
            if let Some(mut child) = g.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            if g.status == Status::Stopped {
                return;
            }
            g.status = Status::Stopped;
        }
        notify(&self.core);
    }
}

#[allow(clippy::too_many_arguments)]
fn monitor(
    core: Arc<Core>,
    generation: u64,
    readers: (mpsc::Receiver<()>, usize),
    log: Arc<Mutex<LogTail>>,
    health_url: String,
    client: reqwest::Client,
    poll: Duration,
    load_timeout: Duration,
) {
    let deadline = Instant::now() + load_timeout;
    loop {
        std::thread::sleep(poll);
        let loading = {
            let mut g = core.inner.lock().unwrap();
            if g.generation != generation {
                return;
            }
            let loading = match &g.status {
                Status::Loading { .. } => true,
                Status::Ready { .. } => false,
                _ => return,
            };
            let exited = g.child.as_mut().and_then(|c| c.try_wait().ok().flatten());
            if let Some(st) = exited {
                g.child = None;
                drop(g);
                wait_readers(&readers.0, readers.1);
                let (lines, device_seen) = {
                    let tail = log.lock().unwrap();
                    (tail.lines(), tail.offload().is_some_and(|o| o.device.is_some()))
                };
                {
                    let mut g = core.inner.lock().unwrap();
                    if g.generation == generation {
                        g.exit = Some(Exit { code: st.code(), device_seen });
                    }
                }
                apply(&core, generation, &log, Event::Exited { code: st.code(), log: lines });
                return;
            }
            loading
        };
        if !loading {
            continue;
        }
        let code = tauri::async_runtime::block_on(async { client.get(&health_url).timeout(Duration::from_secs(2)).send().await })
            .ok()
            .map(|r| r.status().as_u16());
        if !apply(&core, generation, &log, Event::Health(code)) {
            return;
        }
        if Instant::now() >= deadline {
            // One guard: only this run's child, and only while it is still Loading.
            let child = {
                let mut g = core.inner.lock().unwrap();
                if g.generation != generation || !matches!(g.status, Status::Loading { .. }) {
                    continue;
                }
                g.child.take()
            };
            if let Some(mut child) = child {
                let _ = child.kill();
                let _ = child.wait();
            }
            wait_readers(&readers.0, readers.1);
            let lines = log.lock().unwrap().lines();
            apply(&core, generation, &log, Event::TimedOut { log: lines });
            return;
        }
    }
}
