//! UI-agnostic core of Screen Translate. Must not depend on tauri (see `scripts/check-core-no-tauri.sh`).

pub mod capture;
pub mod live;
pub mod manager;
pub mod mode;
pub mod ocr;
pub mod paths;
pub mod pipeline;
pub mod popup;
pub mod settings;
pub mod store;
pub mod translate;
pub mod update;
