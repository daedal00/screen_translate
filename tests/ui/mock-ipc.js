// Injected before app code (page init script). Replaces Tauri IPC with scenario data.
// Scenario from ?scenario=... in the page URL. window.__CATALOG__ is prepended by run.mjs.
(() => {
  const q = new URLSearchParams(location.search);
  const scenario = q.get("scenario") || "empty";
  const delay = Number(q.get("delay")) || 0; // ms to hold manager_status back so the skeleton is visible
  const GB = 1e9, OVERHEAD = 1.5e9;
  const catalog = (window.__CATALOG__ || []).slice();
  const fitOf = (size, b, minVram = 0) => {
    const need = Math.max(size + OVERHEAD, minVram);
    return b.gpu > 0 && need <= b.gpu ? "full" : need <= b.total ? "partial" : "toobig";
  };
  const HW = {
    gpu: { name: "NVIDIA GeForce RTX 3070", vendor: "nvidia", vram: 8 * GB, driver: "551.86", max_cuda: { major: 12, minor: 4 } },
  };
  const S = {
    runtime: "b6100", os: "windows", hardware: { gpu: HW.gpu, ram: 32 * GB, unified: false },
    backend: { recommended: "cuda", installed: "cuda", installed_gpu: "NVIDIA GeForce RTX 3070" },
    fallback_note: null, installed: [], server: { state: "stopped" }, base_url: "http://127.0.0.1:8080/v1",
    autostart: false, selected_model: null, downloading: false,
  };
  if (scenario === "cpu") {
    S.hardware = { gpu: null, ram: 16 * GB, unified: false };
    S.runtime = null; S.backend = { recommended: "cpu", installed: null, installed_gpu: null };
  } else if (scenario === "mac") {
    S.os = "macos";
    S.hardware = { gpu: { name: "Apple M1 Pro", vendor: "apple", vram: 16 * GB, driver: null, max_cuda: null }, ram: 16 * GB, unified: true };
    S.backend = { recommended: "cpu", installed: "cpu", installed_gpu: "Apple M1 Pro" };
  }
  const budget = () => {
    const h = S.hardware;
    if (h.unified) { const t = Math.floor(h.ram * 0.65); return { gpu: t, total: t }; }
    const v = h.gpu ? h.gpu.vram : 0;
    return { gpu: v, total: v + Math.floor(h.ram / 2) };
  };
  const first = catalog.find((c) => c.id === "hy-mt2-7b") || catalog[0];
  if (scenario === "downloading") S.downloading = true;
  if (scenario === "ready" || scenario === "error") {
    S.installed = [{ file: first.file, size: first.size }, { file: catalog[0].file, size: catalog[0].size }];
    if (catalog[0].file === first.file) S.installed.length = 1;
    S.selected_model = first.file;
  }
  if (scenario === "ready")
    S.server = { state: "ready", model: first.file, offload: { device: "NVIDIA GeForce RTX 3070", layers: 29, total: 29 } };
  if (scenario === "error")
    S.server = { state: "error", message: "The model could not be loaded. Not enough memory.",
      log: Array.from({ length: 10 }, (_, i) => `llama_model_load: line ${i + 1}: ggml_cuda_init: failed to allocate buffer of size ${(i + 1) * 211} MiB`) };

  function managerStatus() {
    const b = budget();
    const wrap = (e) => ({ ...e, fit: fitOf(e.size, b, e.min_vram) });
    const rank = { full: 0, partial: 1, toobig: 2 };
    const cat = catalog.map(wrap); // catalog order is the quality rank
    const fits = cat.filter((e) => e.fit !== "toobig").sort((a, c) => (b.gpu > 0 ? rank[a.fit] - rank[c.fit] || cat.indexOf(a) - cat.indexOf(c) : a.size - c.size)).slice(0, 3);
    const best = fits.reduce((m, e) => Math.min(m, cat.indexOf(e)), Infinity), smallest = Math.min(...fits.map((e) => e.size));
    let fd = false;
    const recommended = fits.map((e) => {
      const badges = [];
      if (cat.indexOf(e) === best) badges.push("Best quality");
      else if (fits.length > 1 && e.size === smallest && !fd) { fd = true; badges.push("Fastest"); }
      badges.push(e.fit === "full" ? "Fits your GPU" : b.gpu > 0 ? "Partly on CPU, slower" : "Runs on CPU, slower");
      const { fit, ...entry } = e;
      return { entry, fit, badges };
    });
    return { ...S, budget: b, recommended, catalog: cat };
  }

  const repos = ["Qwen/Qwen2.5-7B-Instruct-GGUF", "bartowski/Qwen2.5-7B-Instruct-GGUF", "Qwen/Qwen2.5-3B-Instruct-GGUF",
    "unsloth/Qwen3-8B-GGUF", "bartowski/Qwen2.5-14B-Instruct-GGUF", "lmstudio-community/Qwen2.5-Coder-7B-Instruct-GGUF"]
    .map((id, i) => ({ id, downloads: [1_240_000, 860_000, 310_000, 98_500, 45_200, 1_900][i], likes: [412, 150, 80, 33, 12, 2][i] }));
  const quants = [["Q2_K", 3.0], ["Q3_K_M", 3.8], ["Q4_K_M", 4.68], ["Q5_K_M", 5.44], ["Q6_K", 6.25], ["Q8_0", 8.1], ["F16", 15.2], ["Q4_K_M-split", 20.4]];
  const hfFiles = () => {
    const b = budget();
    return quants.map(([quant, g], i) => ({ quant, name: `Qwen2.5-7B-Instruct-${quant}.gguf`, size: g * GB, parts: i === 7 ? 3 : 1, fit: fitOf(g * GB, b) }));
  };

  // ---- Tauri internals ----
  const cbs = new Map(); let nextId = 1; const listeners = new Map();
  function emit(event, payload) {
    for (const [eid, l] of listeners) if (l.event === event) { const f = window[`_${l.handler}`]; f && f({ event, id: eid, payload }); }
  }
  window.__emit = emit;
  const log = (window.__ipcLog = []);
  async function invoke(cmd, args = {}) {
    log.push({ cmd, args });
    switch (cmd) {
      case "plugin:event|listen": { const id = nextId++; listeners.set(id, { event: args.event, handler: args.handler }); return id; }
      case "plugin:event|unlisten": listeners.delete(args.eventId); return;
      case "plugin:event|emit": case "plugin:event|emit_to": return;
      case "get_settings":
        return { has_key: false, target_lang: "EN-US", provider: "local", local_url: "http://127.0.0.1:8080/v1", local_model: first.file, mode: "popup" };
      case "save_settings": case "clear_api_key": case "set_autostart": case "stop_server": case "cancel_download":
      case "delete_model": case "start_server": case "install_runtime": case "download_catalog": case "download_model": return;
      case "manager_status": if (delay) await new Promise((r) => setTimeout(r, delay)); return managerStatus();
      case "hf_search": await new Promise((r) => setTimeout(r, 250));
        if (scenario !== "search" && !String(args.query).includes("qwen")) return [];
        return repos;
      case "hf_files": await new Promise((r) => setTimeout(r, 200)); return hfFiles();
      default: console.warn("mock-ipc: unhandled command", cmd, args); return null;
    }
  }
  window.__TAURI_INTERNALS__ = {
    invoke,
    transformCallback(cb, once) {
      const id = nextId++;
      window[`_${id}`] = (...a) => { if (once) delete window[`_${id}`]; return cb && cb(...a); };
      return id;
    },
    unregisterCallback(id) { delete window[`_${id}`]; },
    convertFileSrc: (p) => p,
    metadata: { currentWindow: { label: "settings" }, currentWebview: { windowLabel: "settings", label: "settings" }, windows: [], webviews: [] },
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: (event, id) => listeners.delete(id) };

  // Progress ticker for the downloading scenario (7B file, 1.2 GB of 4.7 GB).
  if (scenario === "downloading") {
    const total = first.size; let done = 1.2 * GB;
    const tick = () => { emit("manager://progress", { job: "model", file: first.file, downloaded: done, total }); };
    setTimeout(tick, 150); setInterval(tick, 500);
  }
})();
