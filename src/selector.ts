import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const box = document.getElementById("box")!;
const dim = document.getElementById("dim")!;
const size = document.getElementById("size")!;
let start: { x: number; y: number } | null = null;
let rect = { x: 0, y: 0, width: 0, height: 0 };

function reset() {
  start = null;
  box.style.display = "none";
  dim.style.display = "block";
  // Replay the dim fade-in.
  dim.style.animation = "none";
  void dim.offsetWidth;
  dim.style.animation = "";
}

// The window is created once and reused: Rust emits `reset` before each show.
listen("reset", reset).catch(console.error);

window.addEventListener("mousedown", (e) => {
  if (e.button !== 0) return;
  rect = { x: e.clientX, y: e.clientY, width: 0, height: 0 };
  start = { x: e.clientX, y: e.clientY };
  dim.style.display = "none";
  box.style.display = "block";
});

window.addEventListener("mousemove", (e) => {
  if (!start) return;
  rect = {
    x: Math.min(start.x, e.clientX),
    y: Math.min(start.y, e.clientY),
    width: Math.abs(e.clientX - start.x),
    height: Math.abs(e.clientY - start.y),
  };
  Object.assign(box.style, {
    left: `${rect.x}px`,
    top: `${rect.y}px`,
    width: `${rect.width}px`,
    height: `${rect.height}px`,
  });
  size.textContent = `${Math.round(rect.width)} × ${Math.round(rect.height)}`;
  // Flip the label inside the box when there is no room below it.
  size.classList.toggle("inside", rect.y + rect.height + 28 > window.innerHeight);
});

window.addEventListener("mouseup", () => {
  if (!start) return;
  start = null;
  if (rect.width < 3 || rect.height < 3) return reset();
  reset();
  invoke("submit_region", { rect }).catch((e) => {
    console.error(e);
    if (e === "selection is empty") reset();
    else invoke("close_window");
  });
});

window.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  reset();
  invoke("close_window");
});
