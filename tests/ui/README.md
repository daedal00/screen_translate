# Settings UI screenshot harness (mocked Tauri IPC)

Renders `settings.html` in a normal browser with `window.__TAURI_INTERNALS__` replaced by `mock-ipc.js`. No app code changes.

## Run

```bash
# needs playwright-cli on PATH (or PLAYWRIGHT_CLI=/path/to/playwright-cli)
node tests/ui/run.mjs                                   # all scenarios x light/dark x 720x680,560x520
node tests/ui/run.mjs --scenarios ready,error --themes dark --sizes 560x520
node tests/ui/run.mjs --tabs Models
```

Options: `--scenarios empty,downloading,ready,error,search,cpu,mac`, `--themes light,dark`, `--sizes WxH,...`, `--tabs General,Models`, `--port 1520`.

Output: `.rex-team/shots/<tab>-<scenario>-<theme>-<w>x<h>.png` (`<tab>` is `page` while the page has no role=tab elements). The script prints the shot list and any page/console errors (favicon 404 ignored).

Vite is started on port 1520 with `--strictPort` (reused if already up) and stopped on exit. It never touches port 1420.

## Manual use

Add `&delay=800` to hold `manager_status` back (skeleton visible); `run.mjs` uses it to write `models-skeleton-<theme>-720x680.png` and `models-skeleton-loaded-...` when the `empty` scenario and Models tab are included.

Scenario is the URL query: `http://localhost:1520/settings.html?scenario=ready`. The mock must be injected as a context init script (`run.mjs` does this); theme comes from `page.emulateMedia({ colorScheme })`.
In the page: `window.__emit(event, payload)` fires a Tauri event (e.g. `manager://progress`); `window.__ipcLog` lists every invoke.

## Limitations

- The `downloading` scenario emits progress for the 7B file (1.2 GB) on a 500 ms loop with a fixed value; `download_*`/`start_server` etc. resolve but do not change state (`manager_status` stays as the scenario defined it). The search scenario also answers any query containing "qwen" in other scenarios.
- Hardware fit math is approximated in the mock (OVERHEAD 1.5e9; macOS budget = 75% of unified memory); verify real numbers with cargo tests.
- Mock does not cover window/dialog plugins; unhandled commands log `mock-ipc: unhandled command` (reported as errors).
- Needs a Chromium that playwright-cli can launch; reduced motion is forced on so pulses do not blur shots.
