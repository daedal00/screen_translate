# Screen Translate

Tray app: press `Ctrl/Cmd+Shift+T`, drag-select a screen region, OCR it, and translate it to English. Windows is the main target.

```
npm install
npm run tauri dev
```

The DeepL API key is stored in the OS keychain. Unsigned macOS dev rebuilds may trigger a keychain access prompt each time.

## Free/offline mode

Pick "Ollama" as the provider in Settings to translate with a local model instead of DeepL (no account or API key). Install [Ollama](https://ollama.com), start it, and run `ollama pull qwen2.5:7b` (or set another model in Settings). Local models are slower than DeepL; the first request after startup loads the model.
