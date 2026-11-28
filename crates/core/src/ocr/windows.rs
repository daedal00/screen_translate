use image::RgbaImage;
use windows::{
    Globalization::Language,
    Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap},
    Media::Ocr::{OcrEngine, OcrResult},
    Storage::Streams::DataWriter,
    core::HSTRING,
};

use super::{TextLine, best_index, script_score};
use crate::capture::PhysicalRect;

const NO_PACK: &str = "no Windows OCR language pack installed; add one in Settings > Time & language > Language & region";

fn err(e: windows::core::Error) -> String {
    format!("Windows OCR failed: {e}")
}

pub fn language_available(tag: &str) -> bool {
    let want = tag.to_ascii_lowercase();
    let aliases: &[&str] = match want.as_str() {
        "zh-hans" => &["zh-hans", "zh-cn", "zh-sg"],
        "zh-hant" => &["zh-hant", "zh-tw", "zh-hk", "zh-mo"],
        _ => &[],
    };
    let Ok(langs) = OcrEngine::AvailableRecognizerLanguages() else {
        return false;
    };
    langs
        .into_iter()
        .filter_map(|l| l.LanguageTag().ok())
        .any(|t| {
            let t = t.to_string().to_ascii_lowercase();
            if aliases.is_empty() {
                t.split('-').next() == want.split('-').next()
            } else {
                aliases
                    .iter()
                    .any(|a| t == *a || t.starts_with(&format!("{a}-")))
            }
        })
}

/// Windows OCR is per-language with no auto-detect, so run the user-profile engine (first) and
/// one engine per other installed recognizer language, then keep the result with the most
/// characters in that engine's expected script. Ties go to the profile engine.
pub fn recognize(img: &RgbaImage) -> Result<Vec<TextLine>, String> {
    recognize_hinted(img, &mut None)
}

/// Like `recognize`, but first tries the engine for `*hint` (the language that produced text last
/// time); it wins when it finds text in its own script, else every engine runs. `*hint` is
/// updated to the winning language.
pub fn recognize_hinted(
    img: &RgbaImage,
    hint: &mut Option<String>,
) -> Result<Vec<TextLine>, String> {
    let (w, h) = img.dimensions();
    // OcrEngine rejects bitmaps over MaxImageDimension; shrink (aspect preserved) and map back.
    let max = OcrEngine::MaxImageDimension().map_err(err)?;
    let resized;
    let small = if w.max(h) > max {
        let k = max as f64 / w.max(h) as f64;
        let (nw, nh) = (
            ((w as f64 * k).round() as u32).max(1),
            ((h as f64 * k).round() as u32).max(1),
        );
        resized = image::imageops::resize(img, nw, nh, image::imageops::FilterType::Triangle);
        &resized
    } else {
        img
    };
    let (sw, sh) = small.dimensions();
    // OcrEngine accepts Bgra8 or Gray8, not Rgba8.
    let mut bgra = small.as_raw().clone();
    bgra.chunks_exact_mut(4).for_each(|p| p.swap(0, 2));
    let writer = DataWriter::new().map_err(err)?;
    writer.WriteBytes(&bgra).map_err(err)?;
    let buffer = writer.DetachBuffer().map_err(err)?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        sw as i32,
        sh as i32,
    )
    .map_err(err)?;

    if let Some(tag) = hint.as_deref()
        && let Ok(lang) = Language::CreateLanguage(&HSTRING::from(tag))
        && let Ok(engine) = OcrEngine::TryCreateFromLanguage(&lang)
        && let Ok((result, score)) = run_engine(&engine, &bitmap)
        && mostly_in_script(&result, score)
    {
        return to_lines(result, (w, h), (sw, sh));
    }

    let mut engines = Vec::new();
    let mut profile_tag = None;
    if let Ok(engine) = OcrEngine::TryCreateFromUserProfileLanguages() {
        profile_tag = engine
            .RecognizerLanguage()
            .and_then(|l| l.LanguageTag())
            .ok()
            .map(|t| t.to_string());
        engines.push(engine);
    }
    for lang in OcrEngine::AvailableRecognizerLanguages().map_err(err)? {
        let tag = lang
            .LanguageTag()
            .map(|t| t.to_string())
            .unwrap_or_default();
        if profile_tag.as_deref() == Some(tag.as_str()) || hint.as_deref() == Some(tag.as_str()) {
            continue;
        }
        if let Ok(engine) = OcrEngine::TryCreateFromLanguage(&lang) {
            engines.push(engine);
        }
    }
    if engines.is_empty() {
        return Err(NO_PACK.to_string());
    }

    let mut results = Vec::new();
    let mut scores = Vec::new();
    let mut tags = Vec::new();
    let mut last_err = String::new();
    for engine in engines {
        match run_engine(&engine, &bitmap) {
            Ok((result, score)) => {
                tags.push(
                    engine
                        .RecognizerLanguage()
                        .and_then(|l| l.LanguageTag())
                        .map(|t| t.to_string())
                        .ok(),
                );
                results.push(result);
                scores.push(score);
            }
            Err(e) => {
                log::warn!("OCR engine failed, skipping: {e}");
                last_err = err(e);
            }
        }
    }
    let best = best_index(&scores).ok_or(last_err)?;
    *hint = if scores[best] > 0 {
        tags.swap_remove(best)
    } else {
        None
    };
    to_lines(results.swap_remove(best), (w, h), (sw, sh))
}

/// A hinted engine's result is kept only when at least half its non-space characters are in its
/// script; a wrong-script engine emits a few stray letters, which must not pin the hint.
fn mostly_in_script(result: &OcrResult, score: usize) -> bool {
    let total = result
        .Text()
        .map(|t| t.to_string().chars().filter(|c| c.is_alphabetic()).count())
        .unwrap_or(0);
    score > 0 && score * 2 >= total
}

/// One engine's result and its score: characters in the engine's expected script.
fn run_engine(
    engine: &OcrEngine,
    bitmap: &SoftwareBitmap,
) -> Result<(OcrResult, usize), windows::core::Error> {
    let tag = engine.RecognizerLanguage()?.LanguageTag()?.to_string();
    let result = engine.RecognizeAsync(bitmap)?.join()?;
    let score = script_score(&result.Text()?.to_string(), &tag);
    Ok((result, score))
}

/// Line boxes of `result`, scaled from the (possibly shrunk) `small` bitmap back to `full`.
fn to_lines(
    result: OcrResult,
    (w, h): (u32, u32),
    (sw, sh): (u32, u32),
) -> Result<Vec<TextLine>, String> {
    let (fx, fy) = (w as f32 / sw as f32, h as f32 / sh as f32);
    let (fw, fh) = (w as f32, h as f32);
    let mut lines = Vec::new();
    for line in result.Lines().map_err(err)? {
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, 0f32, 0f32);
        for word in line.Words().map_err(err)? {
            let r = word.BoundingRect().map_err(err)?;
            x0 = x0.min(r.X);
            y0 = y0.min(r.Y);
            x1 = x1.max(r.X + r.Width);
            y1 = y1.max(r.Y + r.Height);
        }
        let (x0, y0) = (
            (x0 * fx).floor().clamp(0.0, fw),
            (y0 * fy).floor().clamp(0.0, fh),
        );
        let (x1, y1) = (
            (x1 * fx).ceil().clamp(0.0, fw),
            (y1 * fy).ceil().clamp(0.0, fh),
        );
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        lines.push(TextLine {
            text: line.Text().map_err(err)?.to_string(),
            bbox: PhysicalRect {
                x: x0 as u32,
                y: y0 as u32,
                width: (x1 - x0) as u32,
                height: (y1 - y0) as u32,
            },
        });
    }
    Ok(lines)
}
