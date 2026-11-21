use image::RgbaImage;
use objc2::{AnyThread, rc::{Retained, autoreleasepool}, runtime::AnyObject};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRequestTextRecognitionLevel,
};

use super::TextLine;
use crate::capture::PhysicalRect;

const LANGUAGES: [&str; 8] = ["en-US", "de-DE", "fr-FR", "es-ES", "ja-JP", "zh-Hans", "zh-Hant", "ko-KR"];

pub fn recognize(img: &RgbaImage) -> Result<Vec<TextLine>, String> {
    // Worker threads have no autorelease pool; without one Vision's autoreleased objects leak.
    autoreleasepool(|_| recognize_inner(img))
}

fn recognize_inner(img: &RgbaImage) -> Result<Vec<TextLine>, String> {
    let (w, h) = img.dimensions();
    let mut png = Vec::new();
    img.write_with_encoder(image::codecs::png::PngEncoder::new(&mut png))
        .map_err(|e| e.to_string())?;
    let data = NSData::with_bytes(&png);

    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
    request.setUsesLanguageCorrection(true);
    request.setAutomaticallyDetectsLanguage(true);
    let languages: Vec<Retained<NSString>> = LANGUAGES.iter().map(|l| NSString::from_str(l)).collect();
    request.setRecognitionLanguages(&NSArray::from_retained_slice(&languages));

    let options = NSDictionary::<NSString, AnyObject>::new();
    let handler = VNImageRequestHandler::initWithData_options(VNImageRequestHandler::alloc(), &data, &options);
    let requests = NSArray::from_retained_slice(&[Retained::into_super(Retained::into_super(request.clone()))]);
    handler
        .performRequests_error(&requests)
        .map_err(|e| format!("Vision OCR failed: {}", e.localizedDescription()))?;

    let mut lines = Vec::new();
    for obs in request.results().into_iter().flat_map(|r| r.to_vec()) {
        let Some(best) = obs.topCandidates(1).firstObject() else { continue };
        // Vision: normalized, origin bottom-left. Convert to top-left pixels, clamped.
        let b = unsafe { obs.boundingBox() };
        let (fw, fh) = (w as f64, h as f64);
        let x0 = (b.origin.x * fw).floor().clamp(0.0, fw);
        let x1 = ((b.origin.x + b.size.width) * fw).ceil().clamp(0.0, fw);
        let y0 = ((1.0 - b.origin.y - b.size.height) * fh).floor().clamp(0.0, fh);
        let y1 = ((1.0 - b.origin.y) * fh).ceil().clamp(0.0, fh);
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        lines.push(TextLine {
            text: best.string().to_string(),
            bbox: PhysicalRect { x: x0 as u32, y: y0 as u32, width: (x1 - x0) as u32, height: (y1 - y0) as u32 },
        });
    }
    Ok(lines)
}
