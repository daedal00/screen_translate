import { invoke } from "@tauri-apps/api/core";

const key = document.querySelector<HTMLInputElement>("#key")!;
const lang = document.querySelector<HTMLSelectElement>("#lang")!;
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

invoke<{ has_key: boolean; target_lang: string; provider: string; local_url: string; local_model: string }>("get_settings")
  .then((s) => {
    if (s.has_key) key.placeholder = "saved";
    lang.value = s.target_lang;
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
    });
    if (key.value) key.placeholder = "saved";
    key.value = "";
    status.textContent = "Saved.";
  } catch (e) {
    status.textContent = `Error: ${e}`;
  }
});
