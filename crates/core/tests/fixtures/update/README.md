# Updater fixtures

Throwaway keys (empty password, test only) and sigs, generated with the locally installed Tauri CLI:

```
npx --yes @tauri-apps/cli@2.12.6 signer generate --ci -p "" -w throwaway.key      # also writes throwaway.key.pub (renamed throwaway.pub)
npx --yes @tauri-apps/cli@2.12.6 signer generate --ci -p "" -w other.key          # renamed other.pub
printf 'screen translate updater test payload\n' > payload.bin
npx --yes @tauri-apps/cli@2.12.6 signer sign -f throwaway.key -p "" --app-version 9.9.9 payload.bin   # payload.bin.sig
cp payload.bin payload-noversion.bin
npx --yes @tauri-apps/cli@2.12.6 signer sign -f throwaway.key -p "" payload-noversion.bin             # payload-noversion.bin.sig (no version in trusted comment)
```

Real v0.3.1 assets (renamed on download):

```
gh release download v0.3.1 -R daedal00/screen_translate -p latest.json -p '*.app.tar.gz.sig'
mv latest.json latest-0.3.1.json
mv Screen.Translate_universal.app.tar.gz.sig Screen.Translate_universal.app.tar.gz.sig-0.3.1
```

`*.pub` files are base64 of the minisign public key (the format of the `pubkey` config string).
`*.sig` files are base64 of the minisign signature (the format of the latest.json `signature`).
