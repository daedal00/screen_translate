// Usage: node tests/ui/run.mjs [--scenarios empty,ready] [--themes light,dark] [--sizes 720x680,560x520] [--tabs auto|General,Models]
// Starts Vite on :1520 (never kills anything), drives playwright-cli, writes .rex-team/shots/<tab>-<scenario>-<theme>-<w>x<h>.png
import { spawn, spawnSync } from "node:child_process";
import { readFileSync, writeFileSync, mkdirSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const arg = (n, d) => { const i = process.argv.indexOf(`--${n}`); return i > 0 ? process.argv[i + 1] : d; };
const scenarios = arg("scenarios", "empty,downloading,ready,error,search,cpu,mac").split(",");
const themes = arg("themes", "light,dark").split(",");
const sizes = arg("sizes", "720x680,560x520").split(",");
const tabs = arg("tabs", "General,Models").split(",");
const port = arg("port", "1520");
const outDir = resolve(root, ".rex-team/shots");
const pw = process.env.PLAYWRIGHT_CLI || "playwright-cli";
mkdirSync(outDir, { recursive: true });

const mock = readFileSync(join(here, "mock-ipc.js"), "utf8");
const catalog = readFileSync(join(root, "src-tauri/src/manager/catalog.json"), "utf8");
const initScript = `window.__CATALOG__ = ${catalog};\n${mock}`;
const cfg = { firstRepo: "Qwen/Qwen2.5-7B-Instruct-GGUF", scenarios, themes, sizes, tabs, port, outDir, initScript };

// Executed by `playwright-cli run-code` as `async page => ...`.
const body = `async page => {
  const cfg = ${JSON.stringify(cfg)};
  const errors = [];
  page.on("pageerror", e => errors.push("pageerror: " + e.message));
  page.on("console", m => { if ((m.type() === "error" && !/favicon/.test(m.location().url)) || /mock-ipc/.test(m.text())) errors.push(m.type() + ": " + m.text()); });
  await page.context().addInitScript(cfg.initScript);
  const out = [];
  for (const scenario of cfg.scenarios) for (const theme of cfg.themes) for (const size of cfg.sizes) {
    const [w, h] = size.split("x").map(Number);
    await page.setViewportSize({ width: w, height: h });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
    await page.goto("http://localhost:" + cfg.port + "/settings.html?scenario=" + scenario, { waitUntil: "load" });
    await page.waitForTimeout(700);
    const tablist = page.getByRole("tab");
    const hasTabs = await tablist.count() > 0;
    for (const tab of (hasTabs ? cfg.tabs : ["page"])) {
      if (hasTabs) await page.getByRole("tab", { name: tab }).click();
      await page.waitForTimeout(500);
      if (scenario === "search" && tab === "Models") {
        const box = page.getByRole("searchbox").or(page.getByPlaceholder(/search/i)).first();
        if (await box.count()) {
          await box.fill("qwen");
          await page.waitForTimeout(1200);
          const row = page.getByText(cfg.firstRepo).first();
          if (await row.count()) { await row.click(); await page.waitForTimeout(800); }
          else out.push("SKIP: no repo row found after search (" + theme + " " + size + ")");
        } else out.push("SKIP: no search box found (" + theme + " " + size + ")");
      }
      const file = cfg.outDir + "/" + tab.toLowerCase() + "-" + scenario + "-" + theme + "-" + size + ".png";
      await page.screenshot({ path: file });
      out.push(file);
    }
  }
  // Skeleton pair: manager_status is held back (&delay) so loading and loaded geometry can be compared.
  if (cfg.scenarios.includes("empty") && cfg.tabs.includes("Models")) for (const theme of cfg.themes) {
    await page.setViewportSize({ width: 720, height: 680 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
    await page.goto("http://localhost:" + cfg.port + "/settings.html?scenario=empty&delay=800", { waitUntil: "domcontentloaded" });
    await page.getByRole("tab", { name: "Models" }).click();
    await page.waitForTimeout(150);
    for (const name of ["skeleton", "skeleton-loaded"]) {
      const file = cfg.outDir + "/models-" + name + "-" + theme + "-720x680.png";
      await page.screenshot({ path: file });
      out.push(file);
      await page.waitForTimeout(1100);
    }
  }
  return JSON.stringify({ shots: out, errors }, null, 1);
}`;

const dir = mkdtempSync(join(tmpdir(), "uishots-"));
const script = join(dir, "shots.js");
writeFileSync(script, body);

const probe = await fetch(`http://localhost:${port}/settings.html`).then((r) => r.ok).catch(() => false);
let vite;
if (!probe) {
  vite = spawn("npx", ["vite", "--port", port, "--strictPort"], { cwd: root, stdio: "ignore" });
  for (let i = 0; i < 60 && !(await fetch(`http://localhost:${port}/settings.html`).then((r) => r.ok).catch(() => false)); i++)
    await new Promise((r) => setTimeout(r, 500));
}
let code = 0;
try {
  spawnSync(pw, ["open"], { stdio: "inherit" });
  const r = spawnSync(pw, ["run-code", "--filename=" + script], { stdio: "inherit" });
  code = r.status ?? 1;
} finally {
  spawnSync(pw, ["close"], { stdio: "ignore" });
  vite?.kill();
}
process.exit(code);
