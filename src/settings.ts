import { invoke } from "@tauri-apps/api/core";

const key = document.querySelector<HTMLInputElement>("#key")!;
const lang = document.querySelector<HTMLSelectElement>("#lang")!;
const provider = document.querySelector<HTMLSelectElement>("#provider")!;
const model = document.querySelector<HTMLInputElement>("#model")!;
const status = document.querySelector<HTMLDivElement>("#status")!;
const deeplFields = document.querySelector<HTMLDivElement>("#deepl-fields")!;
const ollamaFields = document.querySelector<HTMLDivElement>("#ollama-fields")!;
const remove = document.querySelector<HTMLButtonElement>("#remove")!;

const save = document.querySelector<HTMLButtonElement>("#save")!;

function syncProvider() {
  const ollama = provider.value === "ollama";
  deeplFields.hidden = ollama;
  remove.hidden = ollama;
  ollamaFields.hidden = !ollama;
}
provider.addEventListener("change", syncProvider);

invoke<{ has_key: boolean; target_lang: string; provider: string; ollama_model: string }>("get_settings")
  .then((s) => {
    if (s.has_key) key.placeholder = "saved";
    lang.value = s.target_lang;
    provider.value = s.provider;
    model.value = s.ollama_model;
    syncProvider();
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
      ollamaModel: model.value,
    });
    if (key.value) key.placeholder = "saved";
    key.value = "";
    status.textContent = "Saved.";
  } catch (e) {
    status.textContent = `Error: ${e}`;
  }
});
