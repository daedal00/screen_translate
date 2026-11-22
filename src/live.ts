import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// Mirrors OverlayLine in src-tauri/src/live.rs (logical px relative to the overlay window).
interface OverlayLine {
  x: number;
  y: number;
  width: number;
  height: number;
  text: string;
  font_px: number;
}

const lines = document.getElementById("lines")!;
const error = document.getElementById("error")!;

function clear() {
  lines.replaceChildren();
  error.hidden = true;
}

function render(items: OverlayLine[]) {
  clear();
  lines.append(
    ...items.map((l) => {
      const div = document.createElement("div");
      div.className = "line";
      div.textContent = l.text;
      Object.assign(div.style, {
        left: `${l.x}px`,
        top: `${l.y}px`,
        width: `${l.width}px`,
        height: `${l.height}px`,
        fontSize: `${l.font_px}px`,
        lineHeight: `${l.height}px`,
      });
      return div;
    }),
  );
}

// The overlay window is created once and reused: "lines" replaces everything (an "error" may follow
// it), "clear" empties it. Listener first, then fetch: covers lines emitted before the page loaded.
Promise.all([
  listen<OverlayLine[]>("lines", (e) => render(e.payload)),
  listen<string>("error", (e) => {
    error.textContent = e.payload;
    error.hidden = false;
  }),
  listen("clear", clear),
]).then(() => invoke<OverlayLine[]>("get_live_lines").then(render));
