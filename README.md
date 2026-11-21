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

## Free/offline mode

Pick "Ollama" as the provider in Settings to translate with a local model instead of DeepL (no account or API key). Install [Ollama](https://ollama.com), start it, and run `ollama pull qwen2.5:7b` (or set another model in Settings). Local models are slower than DeepL; the first request after startup loads the model.

## License

MIT, see [LICENSE](LICENSE).
