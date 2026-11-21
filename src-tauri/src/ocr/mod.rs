use image::RgbaImage;

use crate::capture::PhysicalRect;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[derive(Debug, Clone, PartialEq)]
pub struct TextLine {
    pub text: String,
    /// Pixels of the input image.
    pub bbox: PhysicalRect,
}

/// Recognize text lines in the engine's reading order.
pub fn recognize(img: &RgbaImage) -> Result<Vec<TextLine>, String> {
    #[cfg(target_os = "macos")]
    let result = macos::recognize(img);
    #[cfg(target_os = "windows")]
    let result = windows::recognize(img);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let result = {
        let _ = img;
        Err("OCR not supported on this platform".to_string())
    };

    result
}

/// How many chars of `text` belong to the scripts expected for OCR language `lang` (BCP-47).
/// Used to pick between per-language engines that each return something for the same image.
#[cfg_attr(not(windows), allow(dead_code))]
fn script_score(text: &str, lang: &str) -> usize {
    let primary = lang.split('-').next().unwrap_or("").to_ascii_lowercase();
    let in_script: fn(char) -> bool = match primary.as_str() {
        "ja" => |c| matches!(c, '\u{3040}'..='\u{30FF}' | '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}'),
        "zh" => |c| matches!(c, '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}'),
        "ko" => |c| matches!(c, '\u{1100}'..='\u{11FF}' | '\u{3130}'..='\u{318F}' | '\u{AC00}'..='\u{D7AF}'),
        "ru" | "uk" | "bg" | "sr" | "be" | "mk" | "kk" => |c| matches!(c, '\u{0400}'..='\u{04FF}'),
        "el" => |c| matches!(c, '\u{0370}'..='\u{03FF}'),
        "ar" | "fa" | "ur" => |c| matches!(c, '\u{0600}'..='\u{06FF}'),
        "he" => |c| matches!(c, '\u{0590}'..='\u{05FF}'),
        "th" => |c| matches!(c, '\u{0E00}'..='\u{0E7F}'),
        _ => |c| {
            c.is_ascii_alphabetic()
                || (matches!(c, '\u{00C0}'..='\u{024F}') && c != '\u{00D7}' && c != '\u{00F7}')
        },
    };
    text.chars().filter(|&c| in_script(c)).count()
}

/// Index of the highest score; ties go to the earliest (the profile engine comes first).
#[cfg_attr(not(windows), allow(dead_code))]
fn best_index(scores: &[usize]) -> Option<usize> {
    scores.iter().enumerate().fold(None, |best: Option<(usize, usize)>, (i, &s)| match best {
        Some((_, bs)) if bs >= s => best,
        _ => Some((i, s)),
    }).map(|(i, _)| i)
}

/// Trim each line, drop empty ones, join with '\n'.
pub fn join_lines(lines: &[TextLine]) -> String {
    lines
        .iter()
        .map(|l| l.text.trim())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether OCR can read text in `tag` (BCP-47, e.g. "ja", "zh-Hans"). Windows needs the
/// matching language pack; macOS Vision ships all languages.
pub fn language_available(tag: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        let _ = tag;
        true
    }
    #[cfg(target_os = "windows")]
    {
        windows::language_available(tag)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = tag;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn japanese_engine_beats_latin_garbage() {
        let text = "こんにちは世界";
        assert!(script_score(text, "ja") > script_score(text, "en-US"));
        assert_eq!(script_score(text, "en-US"), 0);
    }

    #[test]
    fn latin_engine_beats_cjk_garbage() {
        let text = "Hello wörld";
        assert!(script_score(text, "en-US") > script_score(text, "ja"));
        assert_eq!(script_score("Straße façade", "fr-FR"), 12);
    }

    #[test]
    fn scripts_per_language() {
        assert_eq!(script_score("漢字かな", "zh-Hans"), 2);
        assert_eq!(script_score("한국어 text", "ko"), 3);
        assert_eq!(script_score("Привет mir", "ru"), 6);
        assert_eq!(script_score("12 ×÷ !?", "en"), 0);
    }

    #[test]
    fn ties_go_to_first_and_empty_is_none() {
        assert_eq!(best_index(&[3, 3, 2]), Some(0));
        assert_eq!(best_index(&[1, 5, 5]), Some(1));
        assert_eq!(best_index(&[0, 0]), Some(0));
        assert_eq!(best_index(&[]), None);
    }
}
