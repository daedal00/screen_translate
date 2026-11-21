# Screen Translate

Tray app: press `Ctrl/Cmd+Shift+T`, drag-select a screen region, OCR it, and translate it to English. Windows is the main target.

```
npm install
npm run tauri dev
```

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
