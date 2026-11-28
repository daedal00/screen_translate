/// What the main hotkey does: a one-shot popup or a pinned live region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Popup,
    Live,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hotkey {
    Main,
    Secondary,
}

pub const DEFAULT_MODE: Mode = Mode::Popup;

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Popup => "popup",
            Mode::Live => "live",
        }
    }

    /// Exact match only.
    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "popup" => Some(Mode::Popup),
            "live" => Some(Mode::Live),
            _ => None,
        }
    }

    /// Missing or unknown stored values fall back to the default.
    pub fn from_stored(s: Option<&str>) -> Mode {
        s.and_then(Mode::parse).unwrap_or(DEFAULT_MODE)
    }

    pub fn other(self) -> Mode {
        match self {
            Mode::Popup => Mode::Live,
            Mode::Live => Mode::Popup,
        }
    }

    /// Must match the options in `settings.html`.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Popup => "Popup (one-shot)",
            Mode::Live => "Live (pinned region)",
        }
    }
}

pub fn mode_for(hotkey: Hotkey, setting: Mode) -> Mode {
    match hotkey {
        Hotkey::Main => setting,
        Hotkey::Secondary => setting.other(),
    }
}

/// Tray item texts: (main, secondary).
pub fn tray_labels(setting: Mode) -> (String, String) {
    (
        format!("Translate region: {}", setting.label()),
        setting.other().label().to_owned(),
    )
}
