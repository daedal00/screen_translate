# Screen Translate

Tray app: press `Ctrl/Cmd+Shift+T`, drag-select a screen region, OCR it, and translate it to English. Windows is the main target. Settings has a **Translation mode** (Popup by default, or Live) that `Ctrl/Cmd+Shift+T` runs.

**Live mode:** keeps re-reading a pinned region and draws the translation over each line as it changes. `Ctrl/Cmd+Shift+T` runs the mode chosen in Settings and `Ctrl/Cmd+Shift+L` runs the other one (Live, by default); the tray menu has a matching item for each. Drag-select a region to pin it, and press the live shortcut (or tray item) again to stop.

## Install

Download the installer for your system from the [latest release](https://github.com/daedal00/screen_translate/releases/latest).

**macOS 13+ (Apple Silicon and Intel):** open the `.dmg` and drag Screen Translate to Applications. The app isn't signed with an Apple Developer ID yet, so macOS blocks the first launch. To allow it, either:
- try to open the app once, then (within about an hour) open System Settings > Privacy & Security and click **Open Anyway** next to the Screen Translate message, or
- run `xattr -cr "/Applications/Screen Translate.app"` in Terminal.

On first use, grant Screen Recording permission when asked (System Settings > Privacy & Security > Screen Recording), then restart the app.

After updating on macOS, each new version counts as a new app: remove the old Screen Translate entry under Screen Recording (the `-` button) and grant it again, and click **Always Allow** if macOS asks about keychain access for your DeepL key.

**Windows 10/11:** run the `_x64-setup.exe` (or the `.msi`). If SmartScreen shows "Windows protected your PC", click **More info** > **Run anyway**.

## Development

```
npm install
npm run tauri dev
```

## Releasing

Bump `version` in `src-tauri/tauri.conf.json` (keep `package.json` and `src-tauri/Cargo.toml` in sync) and merge to `main`. The Release workflow runs the tests, builds macOS and Windows installers into a draft release, and publishes `v<version>` once both platforms succeed. Merges that keep the version don't create a release. Use plain `MAJOR.MINOR.PATCH` versions (the Windows MSI rejects prerelease suffixes like `-beta.1`).

If a platform build fails, the draft stays and later pushes at that version skip: re-run the failed jobs from the Actions tab, or delete the draft release and push again.

## Setting up translation (DeepL API key)

1. Create a free account at https://www.deepl.com/pro-api and choose the **DeepL API Free** plan (500,000 characters/month; DeepL asks for a card to verify identity but does not charge the Free plan).
2. In your DeepL account, open **API Keys** and copy the key. Free keys end in `:fx`.
3. Run the app, click the tray icon, choose **Settings…**, keep the provider on **DeepL**, paste the key into **DeepL API key**, pick a target language, and click **Save**.
4. Press `Ctrl/Cmd+Shift+T`, drag over some text, and the popup shows the translation.

The key is stored in the OS keychain (macOS Keychain / Windows Credential Manager), never in a file or log. **Remove key** in Settings deletes it. Unsigned macOS dev rebuilds may trigger a keychain access prompt each time.

The transparent selector needs Tauri's `macOSPrivateApi`, which makes the app ineligible for the Mac App Store.

## Local model mode

Pick "Local model" as the provider in Settings to translate with a local model instead of DeepL (no account or API key). It talks to any OpenAI-compatible server; the default is [llama.cpp](https://github.com/ggml-org/llama.cpp):

```sh
brew install llama.cpp
llama-server -hf Qwen/Qwen2.5-7B-Instruct-GGUF:Q4_K_M
```

Leave Server URL at `http://127.0.0.1:8080` (without `/v1`; a trailing `/v1` is also accepted) and Model empty (llama-server uses the model it loaded). Ollama also works: set the URL to `http://127.0.0.1:11434` and enter a model name such as `qwen2.5:7b`. Local models are slower than DeepL; the first request after startup may be slow.

### Recommended models

Pick the largest model that fits in your GPU's memory (VRAM), or in unified memory on Apple Silicon. Bigger models translate more naturally, especially Japanese, Chinese and Korean. All of these are Qwen 2.5 Instruct (strong at CJK) at Q4_K_M quantization; swap the `-hf` value in the command above:

| GPU memory | Model (`-hf` value) | Download | Notes |
|---|---|---|---|
| 4 GB | `Qwen/Qwen2.5-3B-Instruct-GGUF:Q4_K_M` | ~2 GB | Fast, rougher translations |
| 8 GB (or 16 GB Mac) | `Qwen/Qwen2.5-7B-Instruct-GGUF:Q4_K_M` | ~4.7 GB | Default; good balance |
| 12-16 GB | `Qwen/Qwen2.5-14B-Instruct-GGUF:Q4_K_M` | ~9 GB | Noticeably more natural |
| 24 GB+ | `Qwen/Qwen2.5-32B-Instruct-GGUF:Q4_K_M` | ~20 GB | Best quality, slower |

On Windows with an NVIDIA GPU, download the CUDA build of llama.cpp from its [releases page](https://github.com/ggml-org/llama.cpp/releases) (or the Vulkan build for AMD/Intel GPUs) and run the same command with `-ngl 99` to put the whole model on the GPU, for example:

```sh
llama-server -hf Qwen/Qwen2.5-14B-Instruct-GGUF:Q4_K_M -ngl 99
```

Avoid reasoning models (Qwen3, DeepSeek-R1) for this app: they think before answering, which adds seconds per translation.

If you point the URL at a non-local server, the text you capture is sent to that server; Settings shows a warning in that case.

## License

MIT, see [LICENSE](LICENSE).
