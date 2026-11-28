# Screen Translate 0.4.0

The app UI is rebuilt on GPUI (replacing Tauri). Updates are delivered the same way as before: the app checks the signed `latest.json` on GitHub releases.

## Updating

- **Windows, installed from the MSI:** the new installer removes the old MSI install first, which shows an admin (UAC) prompt. Approve it. If you decline, the update stops and your current install stays as it is (start the app yourself afterwards); you can also uninstall the MSI from Settings > Apps before updating.
- **Windows, installed from the setup.exe:** updates in place.
- **macOS:** the build is ad-hoc signed and the signature changes with each build, so macOS may ask for Screen Recording permission again after updating. Allow it again in System Settings > Privacy & Security > Screen & System Audio Recording if capture stops working.

Settings and downloaded models are kept.
