//! The curated model catalog (data in `catalog.json`) and recommendations for the detected hardware.

use std::sync::OnceLock;

use super::hardware::{Budget, Fit, OVERHEAD, fit};

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Hugging Face repo, `owner/name`.
    pub repo: String,
    pub file: String,
    /// Download size in bytes.
    pub size: u64,
    /// Lowercase hex sha256 of the file, when known.
    pub sha256: Option<String>,
    /// Memory the model needs to run well, in bytes (a floor on `required`).
    pub min_vram: u64,
    pub tags: Vec<String>,
    pub prompt_template: String,
}

/// Parses a catalog JSON array.
pub fn parse(json: &str) -> Result<Vec<Entry>, String> {
    serde_json::from_str(json).map_err(|e| format!("Invalid model catalog: {e}"))
}

/// The bundled `catalog.json`.
pub fn entries() -> &'static [Entry] {
    static ENTRIES: OnceLock<Vec<Entry>> = OnceLock::new();
    ENTRIES.get_or_init(|| parse(include_str!("catalog.json")).expect("bundled catalog.json is valid"))
}

/// Bytes needed to run `e`: file size plus context overhead, at least `min_vram`.
pub fn required(e: &Entry) -> u64 {
    (e.size + OVERHEAD).max(e.min_vram)
}

#[derive(serde::Serialize)]
pub struct Recommendation {
    pub entry: Entry,
    pub fit: Fit,
    pub badges: Vec<String>,
}

/// Up to `limit` entries that fit `b`: Full before Partial, larger first within each. Badges:
/// "Best quality" on the largest returned, "Fastest" on the smallest when more than one is returned,
/// then "Fits your GPU", or "Partly on CPU, slower" (GPU present) / "Runs on CPU, slower" (none) for Partial.
pub fn recommend(entries: &[Entry], b: &Budget, limit: usize) -> Vec<Recommendation> {
    let mut fits: Vec<(Entry, Fit)> =
        entries.iter().map(|e| (e.clone(), fit(required(e), b))).filter(|(_, f)| *f != Fit::TooBig).collect();
    fits.sort_by_key(|(e, f)| (*f != Fit::Full, std::cmp::Reverse(e.size)));
    fits.truncate(limit);
    let (largest, smallest) = (
        fits.iter().map(|(e, _)| e.size).max().unwrap_or(0),
        fits.iter().map(|(e, _)| e.size).min().unwrap_or(0),
    );
    let many = fits.len() > 1;
    let mut best_done = false;
    let mut fast_done = false;
    fits.into_iter()
        .map(|(entry, fit)| {
            let mut badges = Vec::new();
            if entry.size == largest && !std::mem::replace(&mut best_done, true) {
                badges.push("Best quality".to_string());
            } else if many && entry.size == smallest && !std::mem::replace(&mut fast_done, true) {
                badges.push("Fastest".to_string());
            }
            badges.push(
                match (fit, b.gpu > 0) {
                    (Fit::Full, _) => "Fits your GPU",
                    (_, true) => "Partly on CPU, slower",
                    (_, false) => "Runs on CPU, slower",
                }
                .to_string(),
            );
            Recommendation { entry, fit, badges }
        })
        .collect()
}
