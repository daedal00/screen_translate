# Screen Translate

Tray app: press `Ctrl/Cmd+Shift+T`, drag-select a screen region, OCR it, and translate it to English. Windows is the main target. Settings has a **Translation mode** (Popup by default, or Live) that `Ctrl/Cmd+Shift+T` runs.

**Live mode:** keeps re-reading a pinned region and draws the translation over each line as it changes. `Ctrl/Cmd+Shift+T` runs the mode chosen in Settings and `Ctrl/Cmd+Shift+L` runs the other one (Live, by default); the tray menu has a matching item for each. Drag-select a region to pin it, and press the live shortcut (or tray item) again to stop.

## Install

Download the installer for your system from the [latest release](https://github.com/daedal00/screen_translate/releases/latest).

**macOS 14+ (Apple Silicon and Intel):** open the `.dmg` and drag Screen Translate to Applications. The app isn't signed with an Apple Developer ID yet, so macOS blocks the first launch. To allow it, either:
- try to open the app once, then (within about an hour) open System Settings > Privacy & Security and click **Open Anyway** next to the Screen Translate message, or
- run `xattr -cr "/Applications/Screen Translate.app"` in Terminal.

On first use, grant Screen Recording permission when asked (System Settings > Privacy & Security > Screen Recording), then restart the app.

After updating on macOS, each new version counts as a new app: remove the old Screen Translate entry under Screen Recording (the `-` button) and grant it again, and click **Always Allow** if macOS asks about keychain access for your DeepL key.

**Windows 10/11:** run the `_x64-setup.exe`. If SmartScreen shows "Windows protected your PC", click **More info** > **Run anyway**.

**Updates:** from v0.3.1 the app updates itself. It checks for a new release at launch and every 6 hours; when one is out, the tray menu shows **Install Update vX and Restart**. You can also click **Check for Updates…** in the tray. Versions before v0.3.1 need one manual download.

## Development

Rust only; the toolchain is pinned by `rust-toolchain.toml`.

```
cargo run -p screen-translate        # run the app
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
packaging/macos/bundle.sh <version>  # universal .app, updater tarball and .dmg in target/bundle (macOS)
```

On Windows, build with `cargo build --release -p screen-translate`, then make the installer with NSIS: `makensis /DVERSION=<version> /DSRC=%CD%\target\release\screen-translate.exe /DOUTFILE=%CD%\target\bundle\setup.exe packaging\windows\installer.nsi` (absolute paths: makensis resolves them from the script directory).

Debug builds read a few environment variables for testing: `ST_SELFTEST`, `ST_LIVE_SELFTEST`, `ST_OPEN_SETTINGS` and `ST_UPDATE_*` (see the `crates/app/src` sources for what each does).

## Releasing

Bump `version` in `crates/app/Cargo.toml` (keep `crates/core/Cargo.toml` in sync) and merge to `main`. The Release workflow runs the tests, builds macOS and Windows installers into a draft release, and publishes `v<version>` once both platforms succeed. Merges that keep the version don't create a release. Use plain `MAJOR.MINOR.PATCH` versions.

Updates are signed with the updater key: the repo secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` must be set, and the public key embedded in `crates/core/src/update.rs` must match it. Losing the private key means installed apps can't verify new updates, so keep a backup.

If a platform build fails, the draft stays and later pushes at that version skip: re-run the failed jobs from the Actions tab, or delete the draft release and push again.

## Setting up translation (DeepL API key)

1. Create a free account at https://www.deepl.com/pro-api and choose the **DeepL API Free** plan (500,000 characters/month; DeepL asks for a card to verify identity but does not charge the Free plan).
2. In your DeepL account, open **API Keys** and copy the key. Free keys end in `:fx`.
3. Run the app, click the tray icon, choose **Settings…**, keep the provider on **DeepL**, paste the key into **DeepL API key**, pick a target language, and click **Save**.
4. Press `Ctrl/Cmd+Shift+T`, drag over some text, and the popup shows the translation.

The key is stored in the OS keychain (macOS Keychain / Windows Credential Manager), never in a file or log. **Remove key** in Settings deletes it. Unsigned macOS dev rebuilds may trigger a keychain access prompt each time.

## Local model mode

Pick "Local model" as the provider in Settings to translate with a local model instead of DeepL: no account or API key, and the captured text never leaves your machine. You need a few GB of free disk and memory. The app can install and run [llama.cpp](https://github.com/ggml-org/llama.cpp) for you:

1. In Settings, choose provider "Local model" on the **General** tab and press Save.
2. Open the **Models** tab. It shows your GPU (or Apple chip) and memory, and how large a model runs fully on your GPU.
3. If llama.cpp is missing, click **Install llama.cpp**. The app picks the right build for your GPU (CUDA for NVIDIA, Vulkan for AMD and Intel, CPU otherwise; Metal is built in on macOS). On Windows you can override it.
4. Download one of the **Recommended for you** models, or search Hugging Face for any GGUF model and pick a quantization (each shows whether it fits your GPU). A custom `owner/repo:quant` or https `.gguf` URL is under **Custom model**.
5. Click **Use** next to an installed model. When it is ready, the status shows how much of the model runs on the GPU, and the Server URL is filled in for you.
6. Optionally tick "Start the server when the app launches".

#### Recommended models

| Model | Size | Best for | Hardware |
|---|---|---|---|
| Gemma 4 12B (QAT) | 7.0 GB | Top translation quality | Macs with 16 GB, GPUs with 10 GB or more |
| Hy-MT2 7B | 4.6 GB | Translation-tuned, best for 8 GB GPUs | 8 GB GPUs, Macs with 16 GB |
| Hy-MT2 1.8B | 1.1 GB | Very fast, close to the 7B quality | Any GPU, or CPU only |
| Qwen3 8B | 5.0 GB | General-purpose, standard open license | 8 GB GPUs, Macs with 16 GB |

Chosen by the benchmark in [`bench/`](bench/) (COMET on FLORES-200 and screen-style text, Japanese, Korean, French, German and Spanish to English). Quality numbers should transfer to other machines; speed will not (it was measured on an M1 Pro with 16 GB).

Large downloads use several connections in parallel and are verified with SHA-256 when Hugging Face publishes a checksum.

Files live in the app data directory: `~/Library/Application Support/com.screentranslate.app/` on macOS and `%APPDATA%\com.screentranslate.app\` on Windows, in `llama/` (the runtime) and `models/`. The managed server listens only on `127.0.0.1:18080`. Port 18080 is reserved for it, so don't run your own server there. Local models are slower than DeepL; the first request after startup may be slow.

### Manual setup

To run your own server instead, any OpenAI-compatible one works. These steps use llama.cpp's `llama-server`; you need a few GB of free disk and memory.

#### 1. Install llama.cpp

**macOS** (Apple Silicon recommended; uses the GPU automatically), with [Homebrew](https://brew.sh):

```sh
brew install llama.cpp
```

**Windows 10/11**, in PowerShell:

```powershell
winget install llama.cpp
```

Close and reopen PowerShell afterwards so `llama-server` is on your PATH. For the best speed on an **NVIDIA** GPU, download the CUDA build from the [llama.cpp releases page](https://github.com/ggml-org/llama.cpp/releases) instead: get `llama-<version>-bin-win-cuda-<cuda version>-x64.zip` and the matching `cudart-llama-bin-win-cuda-<cuda version>-x64.zip`, unzip both into the same folder (for example `C:\llama.cpp`), and run `.\llama-server.exe` from that folder in the commands below. AMD and Intel GPUs: use the `vulkan` zip the same way.

Check it worked: `llama-server --version` prints a version.

#### 2. Pick a model

Pick the largest model that fits in your GPU's memory (VRAM), or in unified memory on a Mac (leave room for your other apps). Bigger models translate more naturally, especially Japanese, Chinese and Korean. All of these are Qwen 2.5 Instruct (strong at CJK) at Q4_K_M quantization:

| GPU / Mac memory | Model (`-hf` value) | Download | Notes |
|---|---|---|---|
| 4 GB | `Qwen/Qwen2.5-3B-Instruct-GGUF:Q4_K_M` | ~2 GB | Fast, rougher translations |
| 8 GB (or 16 GB Mac) | `Qwen/Qwen2.5-7B-Instruct-GGUF:Q4_K_M` | ~4.7 GB | Recommended starting point |
| 12-16 GB (or 32 GB Mac) | `Qwen/Qwen2.5-14B-Instruct-GGUF:Q4_K_M` | ~9 GB | Noticeably more natural |
| 24 GB+ (or 64 GB Mac) | `Qwen/Qwen2.5-32B-Instruct-GGUF:Q4_K_M` | ~20 GB | Best quality, slower |

Avoid reasoning models (Qwen3, DeepSeek-R1): they think before answering, which adds seconds per translation.

#### 3. Download and load the model

Start the server with the model you picked (same command on macOS and Windows):

```sh
llama-server -hf Qwen/Qwen2.5-7B-Instruct-GGUF:Q4_K_M -c 4096
```

- The first run downloads the model from Hugging Face (several GB; progress shows in the terminal) and caches it, in `~/.cache/huggingface/hub` on macOS or `%USERPROFILE%\.cache\huggingface\hub` on Windows. Later runs load it from the cache in seconds.
- `-c 4096` keeps memory use low; translations are short, so a bigger context isn't needed.
- It's ready when the terminal prints `listening on http://127.0.0.1:8080`. Opening that address in a browser shows llama.cpp's chat page, a quick way to check the model works.
- **Keep this terminal open** while you use Screen Translate. Press `Ctrl+C` to stop the server, and run the same command again next time.
- To switch models, stop the server and start it with a different `-hf` value. To use a `.gguf` file you downloaded yourself, run `llama-server -m path\to\model.gguf -c 4096` instead.

#### 4. Point Screen Translate at it

1. Open **Settings** from the tray icon.
2. Set **Provider** to **Local model (llama.cpp)**.
3. Leave **Server URL** at `http://127.0.0.1:8080` and **Model** empty (llama-server uses the model it loaded).
4. Save, then translate a region as usual. The first translation after the server starts can take several seconds while the model warms up.

**Ollama** also works: set the URL to `http://127.0.0.1:11434` and enter a model name such as `qwen2.5:7b`.

#### Troubleshooting

- **"Local model server is not running"**: llama-server isn't started, or is still loading. Check its terminal for the `listening` line.
- **Very slow translations**: the model doesn't fit in GPU memory. Pick a smaller one from the table. On Windows, make sure you use the CUDA or Vulkan build, not a CPU-only one.
- **Port 8080 already in use**: start the server with `--port 8081` and set the Server URL to `http://127.0.0.1:8081`.

If you point the URL at a non-local server, the text you capture is sent to that server; Settings shows a warning in that case.

## License

MIT, see [LICENSE](LICENSE).
