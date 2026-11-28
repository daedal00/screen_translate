//! UI-agnostic orchestration: popup result state, live session state, translation with cache, and the live loop.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use image::RgbaImage;

use crate::capture::{self, LogicalRect};
use crate::live::{self, ChangeGate, Fingerprint, OverlayLine};
use crate::manager::{self, ManagerState};
use crate::ocr::{self, TextLine};
use crate::settings::{self, SettingsStore};
use crate::translate::{self, Translation, TranslationCache, deepl::DeepL, local::Local};

/// Cells that must change (see `live::distance`) before a live frame counts as new; stable for 2 ticks first.
const LIVE_THRESHOLD: u32 = 4;
const LIVE_SETTLE: u32 = 2;
/// Longest pause after repeated live translation failures.
const LIVE_BACKOFF_MAX: Duration = Duration::from_secs(30);
/// In-flight translations per fire (DeepL Free rate-limits bursts).
const LIVE_CONCURRENCY: usize = 4;
/// Returned (while the selector is still up, so the user can drag again) when the dragged rect maps to no pixels.
pub const SELECTION_EMPTY: &str = "selection is empty";
pub const NO_KEY: &str = "Add your DeepL API key in Settings";

/// The monitor the selector currently covers.
#[derive(Clone, Copy)]
pub struct Target {
    pub scale: f64,
    /// Monitor origin and size in physical pixels (virtual desktop), for placing the popup.
    pub pos: (i32, i32),
    pub size: (u32, u32),
    /// Monitor origin in xcap's units: physical pixels on Windows, points on macOS.
    pub origin: (i32, i32),
    /// xcap monitor id: the CGDirectDisplayID on macOS, the HMONITOR value on Windows (both equal gpui's `DisplayId`).
    pub display_id: u32,
}

impl Target {
    /// The monitor containing `point` (xcap units, see `origin`), else the primary monitor.
    pub fn at_point(point: (i32, i32)) -> Result<Self, String> {
        let monitor = match xcap::Monitor::from_point(point.0, point.1) {
            Ok(m) => m,
            Err(_) => xcap::Monitor::all()
                .map_err(|e| format!("cannot list monitors: {e}"))?
                .into_iter()
                .find(|m| m.is_primary().unwrap_or(false))
                .ok_or("no monitor found")?,
        };
        let err = |e: xcap::XCapError| e.to_string();
        let origin = (monitor.x().map_err(err)?, monitor.y().map_err(err)?);
        let (w, h) = (
            monitor.width().map_err(err)?,
            monitor.height().map_err(err)?,
        );
        let scale = f64::from(monitor.scale_factor().map_err(err)?);
        // xcap reports points on macOS and physical pixels on Windows; `pos`/`size` are always physical.
        let to_px = |v: f64| {
            if cfg!(target_os = "macos") {
                (v * scale).round() as i32
            } else {
                v as i32
            }
        };
        Ok(Self {
            scale,
            pos: (to_px(origin.0 as f64), to_px(origin.1 as f64)),
            size: (to_px(w as f64) as u32, to_px(h as f64) as u32),
            origin,
            display_id: monitor.id().map_err(err)?,
        })
    }

    /// `SELECTION_EMPTY` when `rect` (selector-local logical px) maps to no pixels of this monitor.
    pub fn check(&self, rect: LogicalRect) -> Result<(), String> {
        let (logical_w, logical_h) = (
            self.size.0 as f64 / self.scale,
            self.size.1 as f64 / self.scale,
        );
        if capture::to_physical(rect, 1.0, logical_w.ceil() as u32, logical_h.ceil() as u32)
            .is_none()
        {
            return Err(SELECTION_EMPTY.into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct RegionResult {
    pub original: String,
    pub translation: Option<String>,
    pub detected_source: Option<String>,
    pub error: Option<String>,
    /// The popup offers a Settings button.
    pub needs_settings: bool,
}

impl RegionResult {
    pub fn failed(original: String, error: impl Into<String>) -> Self {
        Self {
            original,
            translation: None,
            detected_source: None,
            error: Some(error.into()),
            needs_settings: false,
        }
    }
}

/// Latest result for the popup. The generation lives under the same lock as the result, so a slow
/// earlier request can't overwrite a newer one (bump+clear and compare+store are each atomic).
#[derive(Default)]
pub struct Latest(Mutex<LatestInner>);

#[derive(Default)]
struct LatestInner {
    generation: u64,
    result: Option<RegionResult>,
}

impl Latest {
    /// Start a new request: bump the generation and drop the previous result.
    pub fn begin(&self) -> u64 {
        let mut l = self.0.lock().unwrap_or_else(|e| e.into_inner());
        l.generation += 1;
        l.result = None;
        l.generation
    }

    pub fn is_current(&self, generation: u64) -> bool {
        self.0.lock().is_ok_and(|l| l.generation == generation)
    }

    /// Store `result` unless a newer request (or an invalidation) superseded `generation`.
    pub fn finish(&self, generation: u64, result: RegionResult) -> bool {
        self.finish_with(generation, result, |_| {})
    }

    /// Like `finish`, and runs `emit` with the stored result while the lock is held, so a concurrent
    /// `invalidate` can never land between the store and the emit. `emit` must not call `Latest` methods.
    pub fn finish_with(
        &self,
        generation: u64,
        result: RegionResult,
        emit: impl FnOnce(&RegionResult),
    ) -> bool {
        let Ok(mut l) = self.0.lock() else {
            return false;
        };
        let current = l.generation == generation;
        if current {
            emit(&result);
            l.result = Some(result);
        }
        current
    }

    /// Drop the content (screen content is private) and stop an in-flight request from being stored.
    pub fn invalidate(&self) {
        self.begin();
    }

    pub fn get(&self) -> Option<RegionResult> {
        self.0.lock().ok()?.result.clone()
    }
}

/// The running live session, if any. `show` only takes effect while the session is still current,
/// so a stopped or restarted session can never update after `stop`.
#[derive(Default)]
pub struct Live(Mutex<LiveInner>);

#[derive(Default)]
struct LiveInner {
    generation: u64,
    running: bool,
    /// What the overlay currently shows, so a freshly loaded overlay can fetch it.
    lines: Vec<OverlayLine>,
    /// The error shown with `lines`, if any.
    error: Option<String>,
}

impl Live {
    /// Claim a new session.
    pub fn start(&self) -> u64 {
        let mut l = self.0.lock().unwrap_or_else(|e| e.into_inner());
        l.generation += 1;
        l.running = true;
        l.lines.clear();
        l.error = None;
        l.generation
    }

    /// End the session. False when none was running.
    pub fn stop(&self) -> bool {
        self.stop_with(|| {})
    }

    /// Like `stop`, and runs `f` (e.g. emitting "clear") under the lock when a session was running,
    /// so no `show_with` callback can run after it. `f` must not call `Live` methods.
    pub fn stop_with(&self, f: impl FnOnce()) -> bool {
        let Ok(mut l) = self.0.lock() else {
            return false;
        };
        if !l.running {
            return false;
        }
        l.running = false;
        l.generation += 1;
        l.lines.clear();
        l.error = None;
        f();
        true
    }

    pub fn running(&self) -> bool {
        self.0.lock().is_ok_and(|l| l.running)
    }

    pub fn is_current(&self, session: u64) -> bool {
        self.0
            .lock()
            .is_ok_and(|l| l.running && l.generation == session)
    }

    /// Replace what the overlay shows unless `session` was stopped or replaced.
    pub fn show(&self, session: u64, lines: Vec<OverlayLine>) -> bool {
        self.show_with(session, lines, |_| {})
    }

    /// Like `show`, and runs `f` with the new lines under the lock when the session is current.
    /// `f` must not call `Live` methods.
    pub fn show_with(
        &self,
        session: u64,
        lines: Vec<OverlayLine>,
        f: impl FnOnce(&[OverlayLine]),
    ) -> bool {
        self.show_with_error(session, lines, None, f)
    }

    /// Like `show_with`, and records `error` (cleared by `None`) so a late overlay can fetch it.
    pub fn show_with_error(
        &self,
        session: u64,
        lines: Vec<OverlayLine>,
        error: Option<String>,
        f: impl FnOnce(&[OverlayLine]),
    ) -> bool {
        let Ok(mut l) = self.0.lock() else {
            return false;
        };
        let current = l.running && l.generation == session;
        if current {
            l.lines = lines;
            l.error = error;
            f(&l.lines);
        }
        current
    }

    /// The error the overlay should currently show, if any.
    pub fn error(&self) -> Option<String> {
        self.0.lock().ok()?.error.clone()
    }

    pub fn lines(&self) -> Vec<OverlayLine> {
        self.0.lock().map(|l| l.lines.clone()).unwrap_or_default()
    }
}

/// Translates through the cache and the configured provider.
pub struct Translator {
    settings: Arc<SettingsStore>,
    manager: Arc<ManagerState>,
    deepl: reqwest::Client,
    local: reqwest::Client,
    cache: Mutex<TranslationCache>,
    key_loader: Box<dyn Fn() -> Result<Option<String>, String> + Send + Sync>,
    /// The DeepL key, read from the keychain on first need. `None` = not loaded yet; `Some(None)` = no key.
    key: Mutex<Option<Option<String>>>,
    deepl_base: Option<String>,
}

/// DeepL rejects a request with more `text` entries than this.
const DEEPL_MAX_TEXTS: usize = 50;

/// What one translation needs from the settings, read once so a batch uses a single snapshot.
struct Plan {
    provider: String,
    url: String,
    model: String,
    target: String,
    template: translate::local::Template,
    cache_target: String,
}

/// Outcome of one live fire. `error` is set when some lines (or all) failed; `lines` holds the rest.
struct Fire {
    lines: Vec<OverlayLine>,
    hits: usize,
    misses: usize,
    error: Option<String>,
}

impl Translator {
    pub fn new(settings: Arc<SettingsStore>, manager: Arc<ManagerState>) -> Self {
        Self::with_key_loader(settings, manager, settings::api_key)
    }

    /// Like `new`, reading the DeepL key through `loader`. `Ok` results (even no key) are cached until
    /// `set_api_key`; an `Err` is not, so the next translation reads again.
    pub fn with_key_loader(
        settings: Arc<SettingsStore>,
        manager: Arc<ManagerState>,
        loader: impl Fn() -> Result<Option<String>, String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            settings,
            manager,
            deepl: translate::deepl::http_client(),
            local: translate::local::http_client(),
            cache: Mutex::new(TranslationCache::new(256)),
            key_loader: Box::new(loader),
            key: Mutex::new(None),
            deepl_base: None,
        }
    }

    /// Send DeepL requests to `base` instead of the host the key implies.
    pub fn with_deepl_base(mut self, base: String) -> Self {
        self.deepl_base = Some(base);
        self
    }

    /// Replace the cached DeepL key (Settings saved or removed it). `None` means no key.
    pub fn set_api_key(&self, key: Option<String>) {
        *self.key.lock().unwrap_or_else(|e| e.into_inner()) = Some(key);
    }

    fn api_key(&self) -> Result<Option<String>, String> {
        let mut cached = self.key.lock().unwrap_or_else(|e| e.into_inner());
        if cached.is_none() {
            *cached = Some((self.key_loader)()?);
        }
        Ok(cached.clone().flatten())
    }

    fn plan(&self) -> Plan {
        let target = self.settings.target_lang();
        let settings::Resolved {
            provider,
            local_url: url,
            local_model: model,
        } = self.settings.resolved();
        // On the managed server the model file picks the prompt template (and is part of the cache key).
        let managed = (provider == "local" && manager::is_managed_url(&url))
            .then(|| self.settings.managed_model())
            .flatten();
        let template = managed
            .as_deref()
            .and_then(|f| manager::catalog::by_file(manager::catalog::entries(), f))
            .map_or(translate::local::Template::Default, |e| {
                translate::local::Template::from_id(&e.prompt_template)
            });
        // Provider, server URL, model and managed file are part of the cache key, so switching any never serves stale results.
        let cache_target = if provider == "local" {
            format!(
                "local:{url}:{model}:{}:{target}",
                managed.as_deref().unwrap_or("")
            )
        } else {
            target.clone()
        };
        Plan {
            provider,
            url,
            model,
            target,
            template,
            cache_target,
        }
    }

    /// Translate `text` through the cache or the configured provider. The bool is true on a cache hit.
    pub async fn translate_text(&self, text: &str) -> Result<(Translation, bool), String> {
        let Plan {
            provider,
            url,
            model,
            target,
            template,
            cache_target,
            ..
        } = self.plan();
        if let Some(t) = self
            .cache
            .lock()
            .ok()
            .and_then(|mut c| c.get(&cache_target, text))
        {
            return Ok((t, true));
        }
        if provider == "local"
            && let Some(msg) = manager::managed_not_ready(&url, &self.manager.server.status())
        {
            return Err(msg.into());
        }
        let t = if provider == "local" {
            Local::new(url, model, self.local.clone())
                .with_template(template)
                .translate(text, &target)
                .await?
        } else {
            self.deepl_client()?.translate(text, &target).await?
        };
        if let Ok(mut c) = self.cache.lock() {
            c.put(&cache_target, text, t.clone());
        }
        Ok((t, false))
    }

    fn deepl_client(&self) -> Result<DeepL, String> {
        let Some(key) = self
            .api_key()
            .inspect_err(|e| log::warn!("reading the DeepL key failed: {e}"))?
        else {
            log::warn!("no DeepL API key set");
            return Err(NO_KEY.into());
        };
        let base = self
            .deepl_base
            .clone()
            .unwrap_or_else(|| translate::deepl::base_for_key(&key).into());
        Ok(DeepL::with_client(key, base, self.deepl.clone()))
    }

    /// Translate every text of one live fire; each result is `(translation, cache hit)`.
    /// DeepL gets all cache misses in a single request; the local provider goes one text at a time,
    /// `LIVE_CONCURRENCY` in flight.
    pub async fn translate_many(
        &self,
        texts: &[String],
    ) -> Vec<Result<(Translation, bool), String>> {
        let plan = self.plan();
        if plan.provider == "local" {
            let mut out = Vec::with_capacity(texts.len());
            for chunk in texts.chunks(LIVE_CONCURRENCY) {
                out.extend(
                    futures_util::future::join_all(chunk.iter().map(|t| self.translate_text(t)))
                        .await,
                );
            }
            return out;
        }
        let mut out: Vec<Option<Result<(Translation, bool), String>>> = vec![None; texts.len()];
        let mut misses: Vec<String> = Vec::new();
        {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            for (slot, text) in out.iter_mut().zip(texts) {
                match cache.get(&plan.cache_target, text) {
                    Some(t) => *slot = Some(Ok((t, true))),
                    None if !misses.contains(text) => misses.push(text.clone()),
                    None => {}
                }
            }
        }
        if !misses.is_empty() {
            let fetched = match self.deepl_client() {
                Ok(d) => {
                    let mut all = Vec::with_capacity(misses.len());
                    let mut result = Ok(());
                    for chunk in misses.chunks(DEEPL_MAX_TEXTS) {
                        match d.translate_batch(chunk, &plan.target).await {
                            Ok(ts) => all.extend(ts),
                            Err(e) => {
                                result = Err(e);
                                break;
                            }
                        }
                    }
                    result.map(|()| all)
                }
                Err(e) => Err(e),
            };
            let by_text: HashMap<&String, Result<Translation, String>> = match fetched {
                Ok(ts) => misses.iter().zip(ts.into_iter().map(Ok)).collect(),
                Err(e) => misses.iter().map(|m| (m, Err(e.clone()))).collect(),
            };
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            for (slot, text) in out.iter_mut().zip(texts) {
                if slot.is_none() {
                    let r = by_text[text].clone();
                    if let Ok(t) = &r {
                        cache.put(&plan.cache_target, text, t.clone());
                    }
                    *slot = Some(r.map(|t| (t, false)));
                }
            }
        }
        out.into_iter().flatten().collect()
    }

    /// OCR `img` and translate the recognized text. Needs a tokio runtime (OCR runs on its blocking pool).
    pub async fn translate_crop(&self, img: RgbaImage) -> RegionResult {
        let t_ocr = Instant::now();
        let lines = match tokio::task::spawn_blocking(move || ocr::recognize(&img)).await {
            Ok(Ok(lines)) => lines,
            Ok(Err(e)) => return RegionResult::failed(String::new(), e),
            Err(e) => return RegionResult::failed(String::new(), e.to_string()),
        };
        let original = ocr::join_lines(&lines);
        log::debug!("recognized text: {original:?}");
        let ocr_ms = t_ocr.elapsed().as_millis();
        if original.trim().is_empty() {
            return RegionResult::failed(original, "No text found");
        }

        let translation = match self.translate_text(&original).await {
            Ok((t, _)) => t,
            Err(e) => {
                log::warn!("translation failed: {e}");
                return RegionResult {
                    needs_settings: e == NO_KEY,
                    ..RegionResult::failed(original, e)
                };
            }
        };
        let Translation {
            text,
            detected_source,
        } = translation;
        log::debug!(
            "timing: ocr {ocr_ms}ms ({} chars), translate {}ms",
            original.chars().count(),
            t_ocr.elapsed().as_millis() - ocr_ms
        );
        log::debug!(
            "translation: {text:?} (from {})",
            detected_source.as_deref().unwrap_or("?")
        );
        RegionResult {
            original,
            translation: Some(text),
            detected_source,
            error: None,
            needs_settings: false,
        }
    }
}

/// Live loop for `session`: grab, change-gate, OCR and translate, then `on_update(lines, error)`.
/// Returns once the session is no longer current. Call it from a plain thread (not a runtime worker):
/// translations are driven with `rt.block_on`, on the app's runtime so pooled HTTP connections stay
/// alive. `on_update` only runs while the session is current, under the `Live` lock, so it must not
/// call `Live` methods (a stop or restart waits for it to finish).
#[allow(clippy::too_many_arguments)]
pub fn run_live(
    translator: &Translator,
    live: &Live,
    session: u64,
    rect: LogicalRect,
    tick: Duration,
    // The pause after the first failure; it doubles for each further consecutive one.
    backoff: Duration,
    rt: &tokio::runtime::Handle,
    mut grab: impl FnMut() -> Result<Option<RgbaImage>, String>,
    mut ocr: impl FnMut(&RgbaImage) -> Result<Vec<TextLine>, String>,
    mut on_update: impl FnMut(Vec<OverlayLine>, Option<String>),
) {
    let new_gate = || ChangeGate::new(LIVE_THRESHOLD, LIVE_SETTLE);
    let mut gate = new_gate();
    let mut last_fired: Option<Fingerprint> = None;
    let mut last_error = String::new();
    let mut failures = 0u32;
    let mut show = |lines: Vec<OverlayLine>, error: Option<String>| {
        live.show_with_error(session, lines, error.clone(), |lines| {
            on_update(lines.to_vec(), error)
        });
    };
    while live.is_current(session) {
        std::thread::sleep(tick);
        let img = match grab() {
            Ok(Some(img)) => img,
            Ok(None) => continue,
            Err(e) => {
                // Re-fire on the next good settled frame, even if the content stays static.
                gate = new_gate();
                // The same failure every tick would flood the log.
                if e != last_error {
                    log::warn!("live capture failed: {e}");
                    show(Vec::new(), Some(e.clone()));
                    last_error = e;
                }
                continue;
            }
        };
        let fp = live::fingerprint(&img);
        if !gate.tick(fp.clone()) {
            continue;
        }
        let t = Instant::now();
        let dist = last_fired.as_ref().map(|p| live::distance(p, &fp));
        last_fired = Some(fp);
        let fire = match live_translate(translator, live, session, rt, &img, rect, &mut ocr) {
            Ok(Some(fire)) => fire,
            Ok(None) => continue,
            Err(e) => Fire {
                lines: Vec::new(),
                hits: 0,
                misses: 0,
                error: Some(e),
            },
        };
        // Screen text is private: per-fire detail stays at debug.
        log::debug!(
            "live fire: distance {dist:?}, {} lines, cache {} hit / {} miss, {}ms",
            fire.lines.len(),
            fire.hits,
            fire.misses,
            t.elapsed().as_millis()
        );
        for l in &fire.lines {
            log::debug!("live line: {:?}", l.text);
        }
        last_error = fire.error.clone().unwrap_or_default();
        show(fire.lines, fire.error.clone());
        if let Some(e) = &fire.error {
            failures = failures.saturating_add(1);
            log::warn!("live translate failed: {e}");
            // Fire again on the next settled frame, after a pause that doubles per consecutive failure.
            gate = new_gate();
            let end = Instant::now() + live::backoff(failures, backoff, LIVE_BACKOFF_MAX);
            while live.is_current(session) && Instant::now() < end {
                std::thread::sleep((end - Instant::now()).min(Duration::from_millis(200)));
            }
        } else {
            failures = 0;
        }
    }
}

/// OCR `img`, group the lines into text blocks and translate each distinct block (DeepL: all in one
/// request), then split every translation back across its block's lines. Blocks that fail are left out
/// and reported in `error`. `Ok(None)` when the session ended meanwhile.
fn live_translate(
    translator: &Translator,
    live: &Live,
    session: u64,
    rt: &tokio::runtime::Handle,
    img: &RgbaImage,
    rect: LogicalRect,
    ocr: &mut impl FnMut(&RgbaImage) -> Result<Vec<TextLine>, String>,
) -> Result<Option<Fire>, String> {
    let mut lines = ocr(img)?;
    if !live.is_current(session) {
        return Ok(None);
    }
    lines.retain(|l| !l.text.trim().is_empty());
    let blocks = live::group_blocks(&lines);
    let block_text = |b: &[usize]| {
        b.iter()
            .map(|&i| lines[i].text.trim())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut texts: Vec<String> = Vec::new();
    for t in blocks.iter().map(|b| block_text(b)) {
        if !texts.contains(&t) {
            texts.push(t);
        }
    }
    let results = rt.block_on(translator.translate_many(&texts));
    if !live.is_current(session) {
        return Ok(None);
    }
    let (mut hits, mut misses, mut error) = (0, 0, None);
    let mut translated = HashMap::new();
    for (text, r) in texts.into_iter().zip(results) {
        match r {
            Ok((t, hit)) => {
                if hit {
                    hits += 1
                } else {
                    misses += 1
                }
                translated.insert(text, t.text);
            }
            Err(e) => {
                error.get_or_insert(e);
            }
        }
    }
    let mut pairs = Vec::new();
    for b in &blocks {
        let Some(t) = translated.get(&block_text(b)) else {
            continue;
        };
        let lens: Vec<usize> = b
            .iter()
            .map(|&i| lines[i].text.trim().chars().count())
            .collect();
        pairs.extend(
            b.iter()
                .zip(live::split_translation(t, &lens))
                .map(|(&i, part)| (lines[i].bbox, part)),
        );
    }
    // The image may be physical pixels where the rect is logical (retina), so derive the scale from both.
    let scale = img.width() as f64 / rect.width;
    Ok(Some(Fire {
        lines: live::layout(&pairs, scale),
        hits,
        misses,
        error,
    }))
}
