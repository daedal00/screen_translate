import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type Fit = "full" | "partial" | "toobig";
type Backend = "cpu" | "vulkan" | "cuda";
type Entry = { id: string; name: string; description: string; repo: string; file: string; size: number };
type Offload = { device: string | null; layers: number; total: number };
type ServerStatus = {
  state: "stopped" | "loading" | "ready" | "error";
  model?: string;
  offload?: Offload | null;
  message?: string;
  log?: string[];
};
type Info = {
  runtime: string | null;
  os: string;
  hardware: {
    gpu: { name: string; vendor: string; vram: number; driver: string | null; max_cuda: { major: number; minor: number } | null } | null;
    ram: number;
    unified: boolean;
  };
  budget: { gpu: number; total: number };
  recommended: { entry: Entry; fit: Fit; badges: string[] }[];
  catalog: (Entry & { fit: Fit })[];
  backend: { recommended: Backend; installed: Backend | null; installed_gpu: string | null };
  fallback_note: string | null;
  installed: { file: string; size: number; fit: Fit }[];
  server: ServerStatus;
  base_url: string;
  autostart: boolean;
  selected_model: string | null;
  downloading: boolean;
};
type Progress = { job: string; file: string; downloaded: number; total: number | null };
type Repo = { id: string; downloads: number; likes: number };
type RepoFile = { quant: string; name: string; size: number; parts: number; fit: Fit };

const OVERHEAD = 1.5e9;
const BACKEND_LABEL: Record<Backend, string> = { cuda: "CUDA", vulkan: "Vulkan", cpu: "CPU" };

type Hooks = { setStatus: (s: string) => void; reloadLocal: () => Promise<void> };
let hooks: Hooks;

// ---- DOM helpers ----
type Kid = Node | string | null | false | undefined;
function h<K extends keyof HTMLElementTagNameMap>(tag: K, attrs: Record<string, string | boolean | EventListener | null | undefined> = {}, ...kids: Kid[]) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (typeof v === "function") el.addEventListener(k.slice(2).toLowerCase(), v);
    else if (k === "class") el.className = v as string;
    else el.setAttribute(k, v === true ? "" : v);
  }
  for (const k of kids) if (k) el.append(k);
  return el;
}
const $ = <T extends HTMLElement>(sel: string) => document.querySelector<T>(sel)!;

/** Rebuilds a container but keeps keyboard focus on the same control (matched by data-k). */
function paint(box: HTMLElement, ...kids: Kid[]) {
  const k = document.activeElement instanceof HTMLElement && box.contains(document.activeElement) ? document.activeElement.dataset.k : undefined;
  box.replaceChildren(...(kids.filter(Boolean) as (Node | string)[]));
  if (k) box.querySelector<HTMLElement>(`[data-k="${CSS.escape(k)}"]`)?.focus();
}

function size(n: number) {
  return n >= 1e9 ? `${(n / 1e9).toFixed(1)} GB` : `${Math.round(n / 1e6)} MB`;
}
/** Memory amounts read better as whole numbers ("16 GB"). */
function memory(n: number) {
  return `${Number((n / 1e9).toFixed(1))} GB`;
}
const compact = new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 1 });

// ---- State ----
let info: Info | null = null;
let loadError = false;
let confirmDelete: string | null = null;
let chosenBackend: Backend | null = null;
/** Which card/row/runtime is downloading (its slot shows progress); "runtime", "custom" or a file name. */
let activeKey: string | null = null;
let last: Progress | null = null;
let speed = 0;
let sample: { at: number; bytes: number } | null = null;

type HfState = { state: "idle" | "searching" | "done" | "error"; repos: Repo[]; error: string };
const hf: HfState = { state: "idle", repos: [], error: "" };
let expanded: string | null = null;
const filesByRepo = new Map<string, RepoFile[] | "loading" | "error">();
let searchSeq = 0;
let debounce: ReturnType<typeof setTimeout> | undefined;

// ---- Progress slots (updated in place so events do not rebuild the page) ----
const slots = new Set<{ el: HTMLElement; update: () => void }>();

function progressSlot() {
  const fill = h("i");
  const bar = h("div", { class: "bar", role: "progressbar", "aria-label": "Download progress", "aria-valuemin": "0", "aria-valuemax": "100" }, fill);
  const text = h("span", { class: "small muted", "aria-live": "off" });
  const cancel = h("button", { class: "btn small", type: "button", "data-k": "cancel-dl", "aria-label": "Cancel download", onclick: () => invoke("cancel_download").catch((e) => hooks.setStatus(String(e))) }, "Cancel");
  const el = h("div", { class: "progress" }, bar, h("div", { class: "row" }, text, h("span", { class: "spacer" }), cancel));
  const update = () => {
    const p = last;
    if (p?.total) {
      bar.classList.remove("indeterminate");
      fill.style.width = `${Math.min(100, (p.downloaded / p.total) * 100)}%`;
      bar.setAttribute("aria-valuenow", String(Math.floor((p.downloaded / p.total) * 100)));
      bar.setAttribute("aria-valuetext", `${size(p.downloaded)} of ${size(p.total)}`);
      text.textContent = `${size(p.downloaded)} of ${size(p.total)}${speed > 0 ? `, ${Math.round(speed / 1e6)} MB/s` : ""}`;
    } else {
      bar.classList.add("indeterminate");
      bar.removeAttribute("aria-valuenow");
      bar.removeAttribute("aria-valuetext");
      text.textContent = p ? `${size(p.downloaded)} downloaded` : "Starting download...";
    }
  };
  slots.add({ el, update });
  update();
  return el;
}

function paintProgress() {
  for (const s of slots) {
    if (!s.el.isConnected) slots.delete(s);
    else s.update();
  }
}

const busy = () => !!info?.downloading;
const downloadingKey = (key: string) => busy() && activeKey === key;
const running = () => info?.server.state === "loading" || info?.server.state === "ready";

// ---- Fit badges ----
function fitBadge(fit: Fit, tooBig = "Too big") {
  const cpuOnly = (info?.budget.gpu ?? 0) <= 0;
  if (fit === "full") return h("span", { class: "badge success" }, "Fits your GPU");
  if (fit === "partial") return h("span", { class: "badge warning" }, cpuOnly ? "Runs on CPU, slower" : "Partly on CPU, slower");
  return h("span", { class: "badge" }, tooBig);
}
function textBadge(t: string) {
  const cls = /^(best quality|fastest)/i.test(t) ? "accent" : /^fits/i.test(t) ? "success" : /^(partly|runs on cpu)/i.test(t) ? "warning" : "";
  return h("span", { class: `badge ${cls}` }, t);
}

/** Short, polite announcement for screen readers (the result list itself is not a live region). */
function announce(msg: string) {
  $("#hf-live").textContent = msg;
}

const modelName = (file: string | undefined) => (file && info?.catalog.find((c) => c.file === file)?.name) || file || "";

// ---- Actions ----
async function run(cmd: () => Promise<unknown>, done: string) {
  hooks.setStatus("");
  try {
    await cmd();
    hooks.setStatus(done);
  } catch (e) {
    hooks.setStatus(String(e));
  }
  await refresh();
}

/** Marks a download as running right away so buttons lock and the slot shows before the first progress event. */
function download(key: string, cmd: () => Promise<unknown>, done: string) {
  activeKey = key;
  last = null;
  speed = 0;
  sample = null;
  if (info) info.downloading = true;
  renderAll();
  return run(cmd, done);
}

async function useModel(file: string) {
  await run(async () => {
    if (running()) await invoke("stop_server");
    await invoke("start_server", { file });
  }, "Starting...");
  await hooks.reloadLocal();
}

const installRuntime = (backend: Backend | null) =>
  download("runtime", () => invoke("install_runtime", { backend }), "llama.cpp installed.");

// ---- Sections ----
function renderStatus() {
  const box = $("#m-status");
  if (!info) {
    paint(box, loadError ? h("div", { class: "alert danger" }, "Couldn't load the model manager. Close and reopen Settings to try again.") : h("div", { class: "row" }, h("div", { class: "skeleton line" }), h("span", { class: "spacer" }), h("div", { class: "skeleton control" })), h("div", { class: "skeleton line short" }));
    return;
  }
  const i = info;
  const s = i.server;
  const [cls, label] = ({ stopped: ["", "Stopped"], loading: ["warning loading", "Loading"], ready: ["success", "Ready"], error: ["danger", "Error"] } as const)[s.state];
  const model = s.model ?? i.selected_model ?? undefined;
  let offload = "";
  if (s.state === "ready" && s.offload) {
    const o = s.offload;
    offload = o.device === null ? "Running on CPU" : o.layers >= o.total ? `Running on GPU: ${o.layers}/${o.total} layers` : `Partly on GPU: ${o.layers}/${o.total} layers`;
  }
  const cudaOnCpu = i.backend.installed === "cuda" && s.state === "ready" && (!s.offload || s.offload.device === null);
  const startFile = i.selected_model ?? i.installed[0]?.file;

  const control = !i.runtime
    ? null
    : running()
      ? h("button", { class: "btn", type: "button", "data-k": "server-ctl", onclick: () => run(() => invoke("stop_server"), "Stopped.") }, "Stop")
      : h("button", { class: "btn primary", type: "button", "data-k": "server-ctl", disabled: !startFile, onclick: () => useModel(startFile!) }, s.state === "error" ? "Try again" : "Start");

  paint(
    box,
    h("div", { class: "row" },
      h("span", { class: `pill ${cls}` }, label),
      h("span", { class: "status-name truncate" }, s.state === "stopped" && !model ? "No model running" : modelName(model)),
      offload && h("span", { class: "small muted" }, offload),
      h("span", { class: "spacer" }),
      !startFile && !running() && i.runtime && h("span", { class: "small muted" }, busy() ? "Downloading..." : "Download a model first."),
      control,
    ),
    !i.runtime &&
      h("div", { class: "alert warning row" },
        h("span", {}, "llama.cpp is not installed. It runs the models on your computer."),
        h("span", { class: "spacer" }),
        downloadingKey("runtime") ? progressSlot() : h("button", { class: "btn primary small", type: "button", "data-k": "install-rt", disabled: busy(), onclick: () => installRuntime(i.os === "windows" ? (chosenBackend ?? i.backend.recommended) : null) }, "Install llama.cpp"),
      ),
    i.fallback_note && h("div", { class: "alert info small" }, i.fallback_note),
    cudaOnCpu &&
      h("div", { class: "alert warning row" },
        h("span", {}, "The CUDA build is running on the CPU."),
        h("span", { class: "spacer" }),
        h("button", { class: "btn small", type: "button", "data-k": "vulkan", disabled: busy(), onclick: () => download("runtime", async () => { await invoke("stop_server"); await invoke("install_runtime", { backend: "vulkan" }); }, "Switched to Vulkan.") }, "Switch to Vulkan"),
      ),
    s.state === "error" &&
      h("div", { class: "alert danger" }, s.message || "The server stopped unexpectedly.",
        s.log?.length ? h("details", { class: "details-gap" }, h("summary", {}, "Show details"), h("pre", { class: "log" }, s.log.join("\n"))) : null),
    h("label", { class: "check" },
      h("input", { type: "checkbox", "data-k": "autostart", ...(i.autostart ? { checked: true } : {}), onchange: (e: Event) => run(() => invoke("set_autostart", { enabled: (e.target as HTMLInputElement).checked }), "Saved.") }),
      "Start the server when the app launches"),
  );
  // `checked` as an attribute only sets the default; keep the live state in sync too.
  box.querySelector<HTMLInputElement>("input[type=checkbox]")!.checked = i.autostart;
}

function skeleton(lines = 2) {
  return h("div", {}, ...Array.from({ length: lines }, () => h("div", { class: "skeleton line" })));
}

function renderHardware() {
  const box = $("#m-hw");
  if (!info) {
    paint(box, h("h2", { class: "section-title" }, "Your hardware"), h("div", { class: "skeleton line" }), h("div", { class: "skeleton line" }), h("div", { class: "skeleton line short" }), h("div", { class: "skeleton control" }));
    return;
  }
  const i = info;
  const { gpu, ram, unified } = i.hardware;
  const free = i.budget.gpu - OVERHEAD;
  const budget =
    !gpu || i.budget.gpu <= 0
      ? "No supported GPU found: models run on the CPU, which is slower."
      : free > 0
        ? `Models up to ~${(Math.floor(free / 1e8) / 10).toFixed(1)} GB run fully on your GPU.`
        : "Your GPU has little free memory: models will run partly on the CPU.";
  const rec = i.backend.recommended;
  const inst = i.backend.installed;
  const win = i.os === "windows";
  const mac = i.os === "macos";
  const gpuChanged = !!inst && !!i.backend.installed_gpu && i.backend.installed_gpu !== (gpu?.name ?? null);
  const needsReinstall = !!inst && (inst !== rec || gpuChanged);
  const selected = chosenBackend ?? rec;

  const recLabel = (b: Backend) => BACKEND_LABEL[b] + (b === rec ? " (recommended)" : "");
  const runtimeText = mac
    ? "llama.cpp: Metal (built in)"
    : !inst
      ? "llama.cpp: not installed"
      : `llama.cpp: ${BACKEND_LABEL[inst]}${inst === rec ? "" : ` (recommended: ${BACKEND_LABEL[rec]})`}`;
  const select = win
    ? h("select", { "data-k": "backend", "aria-label": "llama.cpp build", onchange: (e: Event) => { chosenBackend = (e.target as HTMLSelectElement).value as Backend; renderHardware(); } },
        ...(["cuda", "vulkan", "cpu"] as Backend[]).map((b) => h("option", { value: b, ...(b === selected ? { selected: true } : {}) }, recLabel(b))))
    : null;
  const reinstall = i.runtime && !mac && (needsReinstall || (win && selected !== inst))
    ? h("button", { class: "btn small", type: "button", "data-k": "reinstall", disabled: busy() || running(), onclick: () => installRuntime(win ? selected : null) }, "Reinstall")
    : null;

  paint(
    box,
    h("h2", { class: "section-title" }, "Your hardware"),
    h("dl", { class: "kv" },
      h("dt", {}, unified ? "Chip" : "GPU"),
      h("dd", {}, gpu ? (unified ? gpu.name : `${gpu.name}, ${memory(gpu.vram)} VRAM`) : "None detected"),
      h("dt", {}, "Memory"),
      h("dd", {}, unified ? `${memory(ram)} unified` : `${memory(ram)} RAM`),
    ),
    h("p", { class: "flush" }, budget),
    h("div", { class: "row" },
      h("span", { class: "muted" }, win ? "llama.cpp build" : runtimeText),
      select,
      win && inst && inst !== selected && h("span", { class: "small muted" }, `Installed: ${BACKEND_LABEL[inst]}`),
      reinstall,
      downloadingKey("runtime") && i.runtime ? progressSlot() : null,
    ),
    running() && reinstall && h("div", { class: "hint" }, "Stop the server to reinstall."),
  );
}

function modelButton(file: string, cls = "btn small") {
  return h("button", { class: cls, type: "button", "data-k": `use-${file}`, disabled: !info!.runtime, onclick: () => useModel(file) }, "Use");
}

function renderRecommended() {
  const box = $("#m-rec");
  if (!info) {
    paint(box, h("div", { class: "rec-grid" }, ...[0, 1, 2].map(() => h("div", { class: "skeleton block" }))));
    return;
  }
  const i = info;
  if (i.recommended.length === 0) {
    paint(box, h("div", { class: "empty" }, "No recommended models for this computer. Search below to find one."));
    return;
  }
  paint(
    box,
    h("div", { class: "rec-grid" },
      ...i.recommended.map(({ entry: e, badges }, idx) => {
        const installed = i.installed.some((m) => m.file === e.file);
        const inUse = running() && i.server.model === e.file;
        return h("div", { class: "model-card" },
          h("div", { class: "card-title" }, h("h3", {}, e.name), idx === 0 && h("span", { class: "tag" }, "Recommended")),
          h("div", { class: "desc" }, e.description),
          h("div", { class: "badges" }, h("span", { class: "badge" }, size(e.size)), ...badges.map(textBadge)),
          h("div", { class: "action" },
            downloadingKey(e.file)
              ? progressSlot()
              : inUse
                ? h("span", { class: "pill success" }, "In use")
                : installed
                  ? modelButton(e.file, "btn")
                  : h("button", { class: `btn${idx === 0 ? " primary" : ""}`, type: "button", "data-k": `dl-${e.id}`, disabled: busy(), onclick: () => download(e.file, () => invoke("download_catalog", { id: e.id }), "Model downloaded.") }, "Download")),
        );
      }),
    ),
    busy() && h("div", { class: "hint" }, "Another download is in progress."),
  );
}

function renderInstalled() {
  const box = $("#m-installed");
  if (!info) {
    paint(box, skeleton(2));
    return;
  }
  const i = info;
  if (i.installed.length === 0) {
    paint(box, h("div", { class: "empty" }, "No models yet. Pick a recommended one above to get started."));
    return;
  }
  paint(
    box,
    !i.runtime && h("div", { class: "hint" }, "Install llama.cpp to use these models."),
    ...i.installed.map((m) => {
      const inUse = running() && i.server.model === m.file;
      const confirming = confirmDelete === m.file;
      return h("div", { class: "item" },
        h("span", { class: "name" }, modelName(m.file)),
        h("span", { class: "small muted" }, size(m.size)),
        fitBadge(m.fit),
        inUse && h("span", { class: "pill success" }, "In use"),
        !inUse && modelButton(m.file),
        inUse && h("span", { class: "small muted" }, "Stop the server to delete."),
        confirming
          ? h("span", { class: "row" },
              h("span", { class: "small muted" }, "Delete?"),
              h("button", { class: "btn danger-quiet small", type: "button", "data-k": `del-${m.file}`, onclick: () => { confirmDelete = null; run(() => invoke("delete_model", { file: m.file }), "Deleted."); } }, "Confirm"),
              h("button", { class: "btn small", type: "button", "data-k": `cancel-del-${m.file}`, onclick: () => { confirmDelete = null; renderInstalled(); } }, "Cancel"))
          : h("button", { class: "btn danger-quiet small", type: "button", "data-k": `del-${m.file}`, disabled: inUse, onclick: () => { confirmDelete = m.file; renderInstalled(); } }, "Delete"),
      );
    }),
  );
}

function renderSearch() {
  const box = $("#hf-results");
  const q = $<HTMLInputElement>("#hf-query").value.trim();
  if (hf.state === "idle" || !q) {
    paint(box, h("div", { class: "empty" }, "Search for a model by name, for example qwen or llama. Only GGUF models are listed."));
    return;
  }
  if (hf.state === "searching") {
    paint(box, h("div", { class: "searching" }, h("span", { class: "spinner" }), "Searching..."));
    return;
  }
  if (hf.state === "error") {
    paint(box, h("div", { class: "alert danger" }, "Couldn't reach Hugging Face. Check your internet connection and try again.",
      hf.error ? h("details", { class: "details-gap" }, h("summary", {}, "Show details"), h("pre", { class: "log" }, hf.error)) : null));
    return;
  }
  if (hf.repos.length === 0) {
    paint(box, h("div", { class: "empty" }, `No models found for "${q}". Try a shorter or different name.`));
    return;
  }
  paint(
    box,
    ...hf.repos.map((r) =>
      h("div", { class: "repo" },
        h("button", { class: "repo-head", type: "button", "data-k": `repo-${r.id}`, "aria-expanded": String(expanded === r.id), "aria-controls": `files-${r.id}`, onclick: () => toggleRepo(r.id) },
          h("span", { class: "name" }, r.id),
          h("span", { class: "small muted" }, `${compact.format(r.downloads)} downloads`),
          h("span", { class: "small muted" }, `${compact.format(r.likes)} likes`)),
        expanded === r.id && h("div", { class: "repo-files", id: `files-${r.id}` }, ...repoFiles(r.id)),
      ),
    ),
  );
}

function repoFiles(repo: string): Kid[] {
  const files = filesByRepo.get(repo);
  if (files === "loading" || !files && files !== "error") return [h("div", { class: "searching" }, h("span", { class: "spinner" }), "Loading files...")];
  if (files === "error") return [h("div", { class: "alert danger small" }, "Couldn't load the files for this model. Try again later.")];
  if (files.length === 0) return [h("div", { class: "empty" }, "No downloadable GGUF files in this repository.")];
  return files.map((f) =>
    h("div", { class: "item" },
      h("span", { class: "name mono" }, f.quant || f.name),
      h("span", { class: "small muted col-size" }, `${size(f.size)}${f.parts > 1 ? `, ${f.parts} parts` : ""}`),
      h("span", { class: "col-fit" }, fitBadge(f.fit, "Too big for this computer")),
      downloadingKey(f.name)
        ? progressSlot()
        : h("button", { class: "btn small", type: "button", "data-k": `dl-q-${f.name}`, disabled: busy() || f.fit === "toobig", onclick: () => download(f.name, () => invoke("download_model", { source: `${repo}:${f.name}` }), "Model downloaded.") }, "Download"),
    ),
  );
}

async function toggleRepo(repo: string) {
  expanded = expanded === repo ? null : repo;
  if (expanded && !(filesByRepo.get(repo) instanceof Array)) {
    filesByRepo.set(repo, "loading");
    renderSearch();
    try {
      filesByRepo.set(repo, await invoke<RepoFile[]>("hf_files", { repo }));
    } catch {
      filesByRepo.set(repo, "error");
    }
  }
  renderSearch();
}

async function search(query: string) {
  const seq = ++searchSeq;
  if (!query) {
    hf.state = "idle";
    announce("");
    renderSearch();
    return;
  }
  hf.state = "searching";
  expanded = null;
  renderSearch();
  try {
    const repos = await invoke<Repo[]>("hf_search", { query });
    if (seq !== searchSeq) return;
    Object.assign(hf, { state: "done", repos, error: "" });
    announce(repos.length ? `${repos.length} models found` : "No models found");
  } catch (e) {
    if (seq !== searchSeq) return;
    Object.assign(hf, { state: "error", repos: [], error: String(e) });
    announce("Couldn't reach Hugging Face");
  }
  renderSearch();
}

function renderAll() {
  renderStatus();
  renderHardware();
  renderRecommended();
  renderInstalled();
  renderSearch();
  const custom = $<HTMLButtonElement>("#dl-custom");
  const row = custom.parentElement!;
  custom.disabled = busy();
  row.parentElement!.querySelector(".progress")?.remove();
  if (downloadingKey("custom")) row.after(progressSlot());
  // A download whose key matches no slot (split parts, a refresh mid-start) still needs visible progress and Cancel.
  if (busy() && ![...slots].some((s) => s.el.isConnected)) $("#m-status").append(progressSlot());
  paintProgress();
}

async function refresh() {
  try {
    info = await invoke<Info>("manager_status");
    loadError = false;
    if (!info.downloading) {
      activeKey = null;
      last = null;
    }
  } catch (e) {
    if (!info) loadError = true;
    hooks.setStatus(String(e));
  }
  renderAll();
}

export function initModels(h2: Hooks) {
  hooks = h2;
  const query = $<HTMLInputElement>("#hf-query");
  query.addEventListener("input", () => {
    clearTimeout(debounce);
    debounce = setTimeout(() => search(query.value.trim()), 350);
  });
  query.addEventListener("keydown", (e) => {
    if (e.key !== "Escape" || !query.value) return;
    e.preventDefault();
    clearTimeout(debounce);
    query.value = "";
    search("");
  });
  const custom = $<HTMLInputElement>("#custom");
  $<HTMLButtonElement>("#dl-custom").addEventListener("click", () => {
    const source = custom.value.trim();
    if (!source) return;
    download("custom", () => invoke("download_model", { source }), "Model downloaded.").then(() => {
      if (!busy()) custom.value = "";
    });
  });

  listen<Progress>("manager://progress", (e) => {
    if (!info?.downloading) return;
    const p = e.payload;
    const now = performance.now();
    if (sample && now > sample.at && p.downloaded >= sample.bytes) {
      const inst = ((p.downloaded - sample.bytes) / (now - sample.at)) * 1000;
      speed = speed ? speed * 0.7 + inst * 0.3 : inst;
    }
    sample = { at: now, bytes: p.downloaded };
    last = p;
    if (!activeKey) {
      activeKey = p.job === "runtime" ? "runtime" : p.file;
      renderAll();
    } else paintProgress();
  });
  listen<ServerStatus>("manager://status", async (e) => {
    await refresh();
    if (e.payload.state === "ready") await hooks.reloadLocal();
  });
  renderAll();
  refresh();
}
