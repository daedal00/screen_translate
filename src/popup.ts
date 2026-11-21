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

function render(r: RegionResult) {
  current = r;
  $("translation").textContent = r.translation ?? "";
  $("translation").hidden = r.translation === null;
  $("error").textContent = r.error ?? "";
  $("error").hidden = r.error === null;
  $("original").textContent = r.original;
  $("source").textContent = r.detected_source ? `From ${r.detected_source}` : "";
  $("copy").hidden = r.translation === null;
  $("settings").hidden = !r.needs_settings;
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
// Arm blur-close only once the popup has actually had focus, or a stray early blur would close it.
const armBlur = () => window.addEventListener("blur", close);
if (document.hasFocus()) armBlur();
else window.addEventListener("focus", armBlur, { once: true });

// Listener first, then fetch: covers a result emitted before the page loaded.
listen<RegionResult>("result", (e) => render(e.payload)).then(() =>
  invoke<RegionResult | null>("get_last_result").then((r) => {
    if (r) render(r);
  }),
);
