import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const key = document.querySelector<HTMLInputElement>("#key")!;
const lang = document.querySelector<HTMLSelectElement>("#lang")!;
const mode = document.querySelector<HTMLSelectElement>("#mode")!;
const provider = document.querySelector<HTMLSelectElement>("#provider")!;
const url = document.querySelector<HTMLInputElement>("#url")!;
const model = document.querySelector<HTMLInputElement>("#model")!;
const status = document.querySelector<HTMLDivElement>("#status")!;
const deeplFields = document.querySelector<HTMLDivElement>("#deepl-fields")!;
const localFields = document.querySelector<HTMLDivElement>("#local-fields")!;
const remoteHint = document.querySelector<HTMLDivElement>("#remote-hint")!;
const remove = document.querySelector<HTMLButtonElement>("#remove")!;

const save = document.querySelector<HTMLButtonElement>("#save")!;

function syncProvider() {
  const local = provider.value === "local";
  deeplFields.hidden = local;
  remove.hidden = local;
  localFields.hidden = !local;
}
provider.addEventListener("change", syncProvider);

function isLocalHost(u: string) {
  try {
    return ["127.0.0.1", "localhost", "[::1]"].includes(new URL(u).hostname);
  } catch {
    return true;
  }
}
function syncRemote() {
  remoteHint.hidden = isLocalHost(url.value);
}
url.addEventListener("input", syncRemote);

invoke<{ has_key: boolean; target_lang: string; provider: string; local_url: string; local_model: string; mode: string }>("get_settings")
  .then((s) => {
    if (s.has_key) key.placeholder = "saved";
    lang.value = s.target_lang;
    mode.value = s.mode;
    provider.value = s.provider;
    url.value = s.local_url;
    model.value = s.local_model;
    syncProvider();
    syncRemote();
  })
  .catch((e) => {
    status.textContent = `Error: ${e}`;
    save.disabled = true;
  });

remove.addEventListener("click", async () => {
  try {
    await invoke("clear_api_key");
    key.placeholder = "";
    status.textContent = "Key removed.";
  } catch (e) {
    status.textContent = `Error: ${e}`;
  }
});

save.addEventListener("click", async () => {
  try {
    await invoke("save_settings", {
      key: key.value || null,
      targetLang: lang.value,
      provider: provider.value,
      localUrl: url.value,
      localModel: model.value,
      mode: mode.value,
    });
    if (key.value) key.placeholder = "saved";
    key.value = "";
    status.textContent = "Saved.";
  } catch (e) {
    status.textContent = `Error: ${e}`;
  }
});

// ---- Managed llama.cpp runtime, models and server ----
type ServerStatus = { state: "stopped" | "loading" | "ready" | "error"; model?: string; message?: string; log?: string[] };
type Info = {
  runtime: string | null;
  os: string;
  catalog: { id: string; label: string; source: string; size: number }[];
  installed: { file: string; size: number }[];
  server: ServerStatus;
  base_url: string;
  autostart: boolean;
  selected_model: string | null;
  downloading: boolean;
};
type Progress = { job: string; file: string; downloaded: number; total: number | null };

const $ = <T extends HTMLElement>(id: string) => document.querySelector<T>(id)!;
const rtStatus = $<HTMLDivElement>("#rt-status");
const rtPath = $<HTMLDivElement>("#rt-path");
const rtBackend = $<HTMLSelectElement>("#rt-backend");
const rtInstall = $<HTMLButtonElement>("#rt-install");
const rtHint = $<HTMLDivElement>("#rt-hint");
const installedBox = $<HTMLDivElement>("#installed");
const catalogSel = $<HTMLSelectElement>("#catalog");
const dlCatalog = $<HTMLButtonElement>("#dl-catalog");
const custom = $<HTMLInputElement>("#custom");
const dlCustom = $<HTMLButtonElement>("#dl-custom");
const progressBox = $<HTMLDivElement>("#progress-box");
const progress = $<HTMLProgressElement>("#progress");
const progressText = $<HTMLSpanElement>("#progress-text");
const cancel = $<HTMLButtonElement>("#cancel");
const srvStatus = $<HTMLDivElement>("#srv-status");
const srvLog = $<HTMLPreElement>("#srv-log");
const srvStop = $<HTMLButtonElement>("#srv-stop");
const autostart = $<HTMLInputElement>("#autostart");

let info: Info | null = null;
let confirmDelete: string | null = null;

function size(n: number) {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)} GB`;
  return `${Math.round(n / 1e6)} MB`;
}

function showProgress(p: Progress | null) {
  progressBox.hidden = p === null && !info?.downloading;
  if (p === null) {
    if (!progressBox.hidden) {
      progress.removeAttribute("value");
      progressText.textContent = "Downloading...";
    }
    return;
  }
  const what = p.job === "runtime" ? "llama.cpp" : p.file;
  if (p.total) {
    progress.max = p.total;
    progress.value = p.downloaded;
    progressText.textContent = `Downloading ${what}: ${size(p.downloaded)} of ${size(p.total)} (${Math.floor((p.downloaded / p.total) * 100)}%)`;
  } else {
    progress.removeAttribute("value");
    progressText.textContent = `Downloading ${what}: ${size(p.downloaded)}`;
  }
}

function render(i: Info) {
  info = i;
  const running = i.server.state === "loading" || i.server.state === "ready";
  const busy = i.downloading;

  rtStatus.textContent = i.runtime ? "Installed" : "Not installed";
  rtPath.textContent = i.runtime ?? "";
  rtBackend.hidden = i.os !== "windows";
  rtInstall.textContent = i.runtime ? "Reinstall llama.cpp" : "Install llama.cpp";
  rtInstall.disabled = busy || running;
  rtHint.hidden = !running;

  installedBox.replaceChildren(
    ...i.installed.map((m) => {
      const row = document.createElement("div");
      row.className = "row";
      const name = document.createElement("span");
      name.className = "name";
      name.textContent = `${m.file} (${size(m.size)})`;
      const start = document.createElement("button");
      start.textContent = "Start";
      start.disabled = running || !i.runtime;
      start.addEventListener("click", async () => {
        await run(() => invoke("start_server", { file: m.file }), "Starting...");
        await reloadLocal();
      });
      const del = document.createElement("button");
      del.textContent = confirmDelete === m.file ? "Confirm delete" : "Delete";
      del.disabled = running && i.server.model === m.file;
      del.addEventListener("click", () => {
        if (confirmDelete !== m.file) {
          confirmDelete = m.file;
          render(i);
          return;
        }
        confirmDelete = null;
        run(() => invoke("delete_model", { file: m.file }), "Deleted.");
      });
      row.append(name, start, del);
      return row;
    }),
  );
  if (i.installed.length === 0) installedBox.textContent = "No models installed.";

  if (catalogSel.options.length === 0) {
    for (const c of i.catalog) {
      const o = document.createElement("option");
      o.value = c.source;
      o.textContent = `${c.label} (${size(c.size)})`;
      catalogSel.append(o);
    }
  }
  dlCatalog.disabled = busy;
  dlCustom.disabled = busy;

  const s = i.server;
  srvStatus.textContent =
    s.state === "stopped"
      ? "Stopped"
      : s.state === "loading"
        ? `Loading ${s.model ?? ""}...`
        : s.state === "ready"
          ? `Ready (${s.model ?? ""}) at ${i.base_url}`
          : `Error: ${s.message ?? ""}`;
  const lines = s.state === "error" ? (s.log ?? []) : [];
  srvLog.hidden = lines.length === 0;
  srvLog.textContent = lines.join("\n");
  srvStop.hidden = !running;
  autostart.checked = i.autostart;
  progressBox.hidden = !busy;
  if (busy && !progressText.textContent) showProgress(null);
}

async function refresh() {
  try {
    render(await invoke<Info>("manager_status"));
  } catch (e) {
    status.textContent = `Error: ${e}`;
  }
}

async function run(cmd: () => Promise<unknown>, done: string) {
  status.textContent = "";
  try {
    await cmd();
    status.textContent = done;
  } catch (e) {
    status.textContent = `Error: ${e}`;
  }
  await refresh();
}

/** Reloads the saved URL/Model so a later Save cannot write stale values over what the backend stored. */
async function reloadLocal() {
  try {
    const s = await invoke<{ local_url: string; local_model: string }>("get_settings");
    url.value = s.local_url;
    model.value = s.local_model;
    syncRemote();
  } catch (e) {
    status.textContent = `Error: ${e}`;
  }
}

/** Marks a download as running right away so the buttons lock before the first progress event. */
function download(cmd: () => Promise<unknown>, done: string) {
  if (info) info.downloading = true;
  progressBox.hidden = false;
  progress.removeAttribute("value");
  progressText.textContent = "Starting download...";
  dlCatalog.disabled = dlCustom.disabled = rtInstall.disabled = true;
  return run(cmd, done);
}

rtInstall.addEventListener("click", () =>
  download(() => invoke("install_runtime", { backend: rtBackend.hidden ? "cpu" : rtBackend.value }), "llama.cpp installed."),
);
dlCatalog.addEventListener("click", () => download(() => invoke("download_model", { source: catalogSel.value }), "Model downloaded."));
dlCustom.addEventListener("click", () => {
  const source = custom.value.trim();
  if (!source) return;
  download(() => invoke("download_model", { source }), "Model downloaded.").then(() => {
    if (status.textContent === "Model downloaded.") custom.value = "";
  });
});
cancel.addEventListener("click", () => invoke("cancel_download").catch((e) => (status.textContent = `Error: ${e}`)));
srvStop.addEventListener("click", () => run(() => invoke("stop_server"), "Stopped."));
autostart.addEventListener("change", () => run(() => invoke("set_autostart", { enabled: autostart.checked }), "Saved."));

listen<Progress>("manager://progress", (e) => {
  if (info?.downloading) showProgress(e.payload);
});
listen<ServerStatus>("manager://status", async (e) => {
  await refresh();
  if (e.payload.state === "ready") await reloadLocal();
});
refresh();
