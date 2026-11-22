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
    /// Optional muted line shown under the description (e.g. a license caveat).
    #[serde(default)]
    pub note: Option<String>,
}

/// The entry whose file name is exactly `file`.
pub fn by_file<'a>(entries: &'a [Entry], file: &str) -> Option<&'a Entry> {
    entries.iter().find(|e| e.file == file)
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

/// Up to `limit` entries that fit `b`. With a GPU: Full before Partial, then catalog order (the order is the
/// quality rank). CPU only: smallest first. Badges: "Best quality" on the returned entry earliest in the catalog,
/// "Fastest" on the smallest returned when more than one is returned and it is not the best one,
/// then "Fits your GPU", or "Partly on CPU, slower" (GPU present) / "Runs on CPU, slower" (none) for Partial.
pub fn recommend(entries: &[Entry], b: &Budget, limit: usize) -> Vec<Recommendation> {
    let mut fits: Vec<(usize, Entry, Fit)> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (i, e.clone(), fit(required(e), b)))
        .filter(|(_, _, f)| *f != Fit::TooBig)
        .collect();
    if b.gpu == 0 {
        fits.sort_by_key(|(_, e, _)| e.size); // CPU only: smaller is faster
    } else {
        fits.sort_by_key(|(i, _, f)| (*f != Fit::Full, *i));
    }
    fits.truncate(limit);
    let best = fits.iter().map(|(i, _, _)| *i).min();
    let smallest = fits.iter().map(|(_, e, _)| e.size).min().unwrap_or(0);
    let many = fits.len() > 1;
    let mut fast_done = false;
    fits.into_iter()
        .map(|(i, entry, fit)| {
            let mut badges = Vec::new();
            if Some(i) == best {
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
