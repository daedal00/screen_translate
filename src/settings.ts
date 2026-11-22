import { invoke } from "@tauri-apps/api/core";
import { initModels } from "./models";

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

const msg = document.querySelector<HTMLDivElement>("#m-msg")!;
const footer = document.querySelector<HTMLElement>("footer")!;
let onModels = false;
/** General shows messages in the footer; Models shows them inline in its status card. */
function setStatus(s: string) {
  status.textContent = onModels ? "" : s;
  msg.textContent = onModels ? s : "";
}

// ---- Tabs ----
const tabs = [...document.querySelectorAll<HTMLButtonElement>('[role="tab"]')];
function selectTab(tab: HTMLButtonElement, focus = false) {
  for (const t of tabs) {
    const on = t === tab;
    t.setAttribute("aria-selected", String(on));
    t.tabIndex = on ? 0 : -1;
    document.getElementById(t.getAttribute("aria-controls")!)!.hidden = !on;
  }
  onModels = tab.id === "tab-models";
  footer.hidden = onModels;
  setStatus("");
  if (focus) tab.focus();
}
for (const t of tabs) {
  t.addEventListener("click", () => selectTab(t));
  t.addEventListener("keydown", (e) => {
    const i = tabs.indexOf(t);
    const next = { ArrowRight: tabs[(i + 1) % tabs.length], ArrowLeft: tabs[(i + tabs.length - 1) % tabs.length], Home: tabs[0], End: tabs[tabs.length - 1] }[e.key];
    if (!next) return;
    e.preventDefault();
    selectTab(next, true);
  });
}

// ---- General ----
function syncProvider() {
  const local = provider.value === "local";
  deeplFields.hidden = local;
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
    setStatus(`Error: ${e}`);
    save.disabled = true;
  });

remove.addEventListener("click", async () => {
  try {
    await invoke("clear_api_key");
    key.placeholder = "";
    setStatus("Key removed.");
  } catch (e) {
    setStatus(`Error: ${e}`);
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
    setStatus("Saved.");
  } catch (e) {
    setStatus(`Error: ${e}`);
  }
});

/** Reloads the saved URL/Model so a later Save cannot write stale values over what the backend stored. */
async function reloadLocal() {
  try {
    const s = await invoke<{ local_url: string; local_model: string }>("get_settings");
    url.value = s.local_url;
    model.value = s.local_model;
    syncRemote();
  } catch (e) {
    setStatus(`Error: ${e}`);
  }
}

initModels({ setStatus, reloadLocal });
