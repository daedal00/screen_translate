import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface RegionResult {
  original: string;
  translation: string | null;
  detected_source: string | null;
  error: string | null;
  needs_settings: boolean;
}

const $ = (id: string) => document.getElementById(id)!;
const close = () => invoke("close_window");
let current: RegionResult | null = null;

// Restart a CSS animation on `el`.
function replay(el: HTMLElement, cls: string) {
  el.classList.remove(cls);
  void el.offsetWidth;
  el.classList.add(cls);
}

// The popup window is created once and reused: `pending` resets it before each show (and on hide,
// which also drops the previous content), `result` fills it in.
function pending() {
  current = null;
  const t = $("translation");
  t.textContent = "Translating…";
  t.classList.add("pending");
  t.hidden = false;
  $("error").hidden = true;
  $("original").textContent = "";
  $("source").textContent = "";
  $("copy").hidden = true;
  $("copy").textContent = "Copy";
  $("settings").hidden = true;
  replay($("content"), "enter");
  replay($("bar"), "enter");
}

function render(r: RegionResult) {
  current = r;
  const t = $("translation");
  t.classList.remove("pending");
  t.textContent = r.translation ?? "";
  t.hidden = r.translation === null;
  $("error").textContent = r.error ?? "";
  $("error").hidden = r.error === null;
  $("original").textContent = r.original;
  $("source").textContent = r.detected_source ? `From ${r.detected_source}` : "";
  $("copy").hidden = r.translation === null;
  $("settings").hidden = !r.needs_settings;
  replay($("content"), "fade");
}

$("copy").addEventListener("click", async () => {
  if (current?.translation == null) return;
  try {
    await navigator.clipboard.writeText(current.translation);
    $("copy").textContent = "Copied";
  } catch (e) {
    console.error(e);
    $("copy").textContent = "Copy failed";
  }
});
$("settings").addEventListener("click", () => invoke("open_settings_window"));
$("close").addEventListener("click", close);
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") close();
});
// Blur-close is handled in Rust (armed after the popup first gets focus).
listen("pending", pending);
// Listener first, then fetch: covers a result emitted before the page loaded.
listen<RegionResult>("result", (e) => render(e.payload)).then(() =>
  invoke<RegionResult | null>("get_last_result").then((r) => {
    if (r) render(r);
  }),
);
