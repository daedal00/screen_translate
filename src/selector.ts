import { invoke } from "@tauri-apps/api/core";

const box = document.getElementById("box")!;
const dim = document.getElementById("dim")!;
let start: { x: number; y: number } | null = null;
let rect = { x: 0, y: 0, width: 0, height: 0 };

invoke<ArrayBuffer>("get_selection_image")
  .then((png) => {
    document.body.style.backgroundImage = `url(${URL.createObjectURL(new Blob([png], { type: "image/png" }))})`;
  })
  .catch((e) => {
    console.error(e);
    invoke("close_window");
  });

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
});

window.addEventListener("mouseup", () => {
  if (!start) return;
  start = null;
  if (rect.width < 3 || rect.height < 3) {
    box.style.display = "none";
    dim.style.display = "block";
    return;
  }
  invoke("submit_region", { rect }).catch((e) => {
    console.error(e);
    if (e === "selection is empty") {
      // Rust kept the screenshot; let the user drag again.
      box.style.display = "none";
      dim.style.display = "block";
    } else {
      invoke("close_window");
    }
  });
});

window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") invoke("close_window");
});
