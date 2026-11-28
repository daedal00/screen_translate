//! Settings window: the General tab (mode, language, provider, DeepL key, local server) and the Models tab.
//! `SettingsForm` is the pure model; `SettingsView` is the gpui view over it.

use std::sync::Arc;

use crate::models::{ModelsModel, ModelsView};
use gpui::{
    AnyElement, App, Context, EventEmitter, IntoElement, ParentElement, Render, SharedString,
    Styled, Subscription, Window, div, prelude::*, px,
};
use gpui_component::{
    ActiveTheme, IndexPath,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    searchable_list::SearchableListItem,
    select::{Select, SelectEvent, SelectState},
    tab::{Tab, TabBar},
};
use screen_translate_core::mode::Mode;
use screen_translate_core::settings::{
    PROVIDERS, SettingsStore, TARGETS, clear_api_key, is_local_host, is_valid_url, set_api_key,
};

/// The Settings form as the user sees it. `key` is what was typed; the saved key is never read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsForm {
    pub mode: Mode,
    pub target_lang: String,
    pub provider: String,
    pub key: String,
    pub has_key: bool,
    pub local_url: String,
    pub local_model: String,
}

impl SettingsForm {
    pub fn load(store: &SettingsStore, has_key: bool) -> Self {
        let r = store.resolved();
        Self {
            mode: store.mode(),
            target_lang: store.target_lang(),
            provider: r.provider,
            key: String::new(),
            has_key,
            local_url: r.local_url,
            local_model: r.local_model,
        }
    }

    pub fn deepl_fields_visible(&self) -> bool {
        self.provider != "local"
    }

    pub fn local_fields_visible(&self) -> bool {
        self.provider == "local"
    }

    /// An unparsable URL counts as local, like the Tauri settings page.
    pub fn remote_hint_visible(&self) -> bool {
        is_valid_url(&self.local_url) && !is_local_host(&self.local_url)
    }

    pub fn key_placeholder(&self) -> &str {
        if self.has_key { "saved" } else { "" }
    }

    /// Validates first, so an invalid form never touches the keyring; then stores a typed key via `save_key`
    /// and persists the rest. Nothing is persisted when `save_key` fails.
    pub fn save(
        &mut self,
        store: &SettingsStore,
        save_key: impl FnOnce(&str) -> Result<(), String>,
    ) -> Result<String, String> {
        let err = |e: String| format!("Error: {e}");
        let mode = self.mode.as_str();
        SettingsStore::validate(&self.target_lang, &self.provider, &self.local_url, mode)
            .map_err(err)?;
        let key = self.key.trim().to_owned();
        if !key.is_empty() {
            save_key(&key).map_err(err)?;
            self.has_key = true;
            self.key.clear();
        }
        store
            .save(
                None,
                self.target_lang.clone(),
                self.provider.clone(),
                self.local_url.clone(),
                self.local_model.clone(),
                mode.into(),
            )
            .map_err(err)?;
        Ok("Saved.".into())
    }

    pub fn remove_key(
        &mut self,
        clear: impl FnOnce() -> Result<(), String>,
    ) -> Result<String, String> {
        clear().map_err(|e| format!("Error: {e}"))?;
        self.has_key = false;
        Ok("Key removed.".into())
    }

    /// Reloads the saved URL and model (the Models tab can change them behind the form's back).
    pub fn reload_local(&mut self, store: &SettingsStore) {
        let r = store.resolved();
        self.local_url = r.local_url;
        self.local_model = r.local_model;
    }
}

fn language_label(code: &str) -> &str {
    match code {
        "EN-US" => "English (US)",
        "EN-GB" => "English (UK)",
        "DE" => "German",
        "FR" => "French",
        "ES" => "Spanish",
        "IT" => "Italian",
        "PT-BR" => "Portuguese (BR)",
        "JA" => "Japanese",
        "KO" => "Korean",
        "ZH-HANS" => "Chinese (Simplified)",
        other => other,
    }
}

fn provider_label(p: &str) -> &str {
    match p {
        "deepl" => "DeepL (needs API key)",
        "local" => "Local model (llama.cpp)",
        other => other,
    }
}

fn mode_label(m: &str) -> &str {
    match m {
        "live" => "Live (pinned region)",
        _ => "Popup (one-shot)",
    }
}

#[derive(Clone)]
struct Choice {
    value: &'static str,
    label: &'static str,
}

impl SearchableListItem for Choice {
    type Value = &'static str;

    fn title(&self) -> SharedString {
        self.label.into()
    }

    fn value(&self) -> &Self::Value {
        &self.value
    }
}

type Choices = Vec<Choice>;

fn select_state(
    items: Choices,
    selected: &str,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> gpui::Entity<SelectState<Choices>> {
    let ix = items
        .iter()
        .position(|c| c.value == selected)
        .map(IndexPath::new);
    cx.new(|cx| SelectState::new(items, ix, window, cx))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsEvent {
    /// Settings were written; the tray labels depend on them.
    Saved,
    /// The DeepL key was stored (`Some`) or removed (`None`), so the translator's cached copy must follow.
    KeyChanged(Option<String>),
}

pub struct SettingsView {
    store: Arc<SettingsStore>,
    form: SettingsForm,
    tab: usize,
    status: Option<(SharedString, bool)>,
    mode: gpui::Entity<SelectState<Choices>>,
    lang: gpui::Entity<SelectState<Choices>>,
    provider: gpui::Entity<SelectState<Choices>>,
    key: gpui::Entity<InputState>,
    models: gpui::Entity<ModelsView>,
    url: gpui::Entity<InputState>,
    model: gpui::Entity<InputState>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    /// `has_key` starts false: the keychain is only asked by the app (see `set_has_key`), never by tests.
    pub fn new(store: Arc<SettingsStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let form = SettingsForm::load(&store, false);
        let modes = [Mode::Popup, Mode::Live]
            .map(|m| Choice {
                value: m.as_str(),
                label: mode_label(m.as_str()),
            })
            .to_vec();
        let langs = TARGETS
            .iter()
            .map(|c| Choice {
                value: c,
                label: language_label(c),
            })
            .collect();
        let providers = PROVIDERS
            .iter()
            .map(|p| Choice {
                value: p,
                label: provider_label(p),
            })
            .collect();
        let mode = select_state(modes, form.mode.as_str(), window, cx);
        let lang = select_state(langs, &form.target_lang, window, cx);
        let provider = select_state(providers, &form.provider, window, cx);
        let key = cx.new(|cx| InputState::new(window, cx).masked(true));
        let url = cx.new(|cx| InputState::new(window, cx).default_value(form.local_url.clone()));
        let model = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(form.local_model.clone())
                .placeholder("leave empty for llama-server")
        });

        let models = cx.new(|cx| ModelsView::new(ModelsModel::new(), window, cx));
        let mut subs = Vec::new();
        subs.push(
            cx.subscribe_in(&mode, window, |this, _, e: &SelectEvent<Choices>, _, cx| {
                let SelectEvent::Confirm(Some(v)) = e else {
                    return;
                };
                if let Some(m) = Mode::parse(v) {
                    this.form.mode = m;
                }
                cx.notify();
            }),
        );
        subs.push(
            cx.subscribe_in(&lang, window, |this, _, e: &SelectEvent<Choices>, _, cx| {
                if let SelectEvent::Confirm(Some(v)) = e {
                    this.form.target_lang = (*v).into();
                }
                cx.notify();
            }),
        );
        subs.push(cx.subscribe_in(
            &provider,
            window,
            |this, _, e: &SelectEvent<Choices>, _, cx| {
                if let SelectEvent::Confirm(Some(v)) = e {
                    this.form.provider = (*v).into();
                }
                cx.notify();
            },
        ));
        subs.push(
            cx.subscribe_in(&key, window, |this, s, e: &InputEvent, _, cx| {
                if matches!(e, InputEvent::Change) {
                    this.form.key = s.read(cx).value().to_string();
                }
            }),
        );
        subs.push(
            cx.subscribe_in(&url, window, |this, s, e: &InputEvent, _, cx| {
                if matches!(e, InputEvent::Change) {
                    this.form.local_url = s.read(cx).value().to_string();
                    cx.notify();
                }
            }),
        );
        subs.push(
            cx.subscribe_in(&model, window, |this, s, e: &InputEvent, _, cx| {
                if matches!(e, InputEvent::Change) {
                    this.form.local_model = s.read(cx).value().to_string();
                }
            }),
        );

        Self {
            store,
            form,
            tab: 0,
            status: None,
            mode,
            lang,
            provider,
            key,
            models,
            url,
            model,
            _subs: subs,
        }
    }

    /// The Models tab, which the app drives (see `ModelsCommand`).
    pub fn models(&self) -> &gpui::Entity<ModelsView> {
        &self.models
    }

    pub fn select_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.tab = ix;
        cx.notify();
    }

    /// Shows `result` in the footer (an empty `Ok` clears it).
    pub fn show_status(&mut self, result: Result<String, String>, cx: &mut Context<Self>) {
        self.set_status(result);
        cx.notify();
    }

    /// The Models tab can change the saved server URL and model: show them in the form.
    pub fn reload_local(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.form.reload_local(&self.store);
        let (url, model) = (self.form.local_url.clone(), self.form.local_model.clone());
        self.url.update(cx, |i, cx| i.set_value(url, window, cx));
        self.model
            .update(cx, |i, cx| i.set_value(model, window, cx));
        cx.notify();
    }

    /// Tells the view whether a DeepL key is saved (the placeholder says so).
    pub fn set_has_key(&mut self, has_key: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.form.has_key = has_key;
        self.sync_key_placeholder(window, cx);
    }

    fn sync_key_placeholder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = self.form.key_placeholder().to_owned();
        self.key
            .update(cx, |k, cx| k.set_placeholder(placeholder, window, cx));
        cx.notify();
    }

    fn set_status(&mut self, result: Result<String, String>) {
        self.status = Some(match result {
            Ok(s) => (s.into(), false),
            Err(e) => (e.into(), true),
        });
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // `SettingsStore::save` applies the values in memory before it writes the file, so the tray
        // must follow whenever validation passed, even if the write then fails.
        let f = &self.form;
        let valid =
            SettingsStore::validate(&f.target_lang, &f.provider, &f.local_url, f.mode.as_str())
                .is_ok();
        let typed = self.form.key.trim().to_owned();
        let result = self.form.save(&self.store, set_api_key);
        self.set_status(result);
        if self.form.key.is_empty() {
            if !typed.is_empty() {
                cx.emit(SettingsEvent::KeyChanged(Some(typed)));
            }
            // The typed key was stored (or there was none): never keep it in the field.
            self.key.update(cx, |k, cx| k.set_value("", window, cx));
        }
        if valid {
            cx.emit(SettingsEvent::Saved);
        }
        self.sync_key_placeholder(window, cx);
    }

    fn remove_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let result = self.form.remove_key(clear_api_key);
        if result.is_ok() {
            cx.emit(SettingsEvent::KeyChanged(None));
            // A typed, unsaved key goes too, or the next Save would store it again.
            self.form.key.clear();
            self.key.update(cx, |k, cx| k.set_value("", window, cx));
        }
        self.set_status(result);
        self.sync_key_placeholder(window, cx);
    }

    fn field(
        &self,
        cx: &App,
        label: &'static str,
        control: impl IntoElement,
        hint: Option<&'static str>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(label))
            .child(control)
            .children(hint.map(|h| div().text_size(px(12.0)).text_color(muted).child(h)))
            .into_any_element()
    }

    fn card(&self, cx: &App) -> gpui::Div {
        let t = cx.theme();
        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(16.0))
            .rounded(px(12.0))
            .border_1()
            .border_color(t.border)
            .bg(t.group_box)
    }

    fn general(&self, cx: &mut Context<Self>) -> AnyElement {
        let warning = cx.theme().warning;
        let f = &self.form;
        let mut provider_card = self.card(cx).child(self.field(
            cx,
            "Translation provider",
            Select::new(&self.provider).w_full(),
            None,
        ));
        if f.deepl_fields_visible() {
            let key_row = div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&self.key).mask_toggle()),
                )
                .child(
                    Button::new("remove-key")
                        .label("Remove key")
                        .on_click(cx.listener(|this, _, window, cx| this.remove_key(window, cx))),
                );
            provider_card = provider_card.child(self.field(
                cx,
                "DeepL API key",
                key_row,
                Some("Get a free key at https://www.deepl.com/pro-api"),
            ));
        }
        if f.local_fields_visible() {
            let mut url = self.field(
                cx,
                "Server URL",
                Input::new(&self.url),
                Some(
                    "Managed models fill this in automatically. Or run your own llama-server / Ollama and set the URL here.",
                ),
            );
            if f.remote_hint_visible() {
                url = div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(url)
                    .child(
                        div()
                            .px(px(12.0))
                            .py(px(8.0))
                            .rounded(px(8.0))
                            .text_size(px(12.0))
                            .bg(warning.opacity(0.18))
                            .text_color(warning)
                            .child("Screen text will be sent to this server."),
                    )
                    .into_any_element();
            }
            provider_card = provider_card.child(url).child(self.field(
                cx,
                "Model (optional)",
                Input::new(&self.model),
                None,
            ));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                self.card(cx)
                    .child(self.field(
                        cx,
                        "Translation mode",
                        Select::new(&self.mode).w_full(),
                        Some(if cfg!(target_os = "macos") {
                            "Cmd+Shift+T runs this mode. Cmd+Shift+L runs the other."
                        } else {
                            "Ctrl+Shift+T runs this mode. Ctrl+Shift+L runs the other."
                        }),
                    ))
                    .child(self.field(
                        cx,
                        "Translate into",
                        Select::new(&self.lang).w_full(),
                        None,
                    )),
            )
            .child(provider_card)
            .into_any_element()
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let (bg, fg, muted, border, danger) = (
            t.background,
            t.foreground,
            t.muted_foreground,
            t.border,
            t.danger,
        );
        let body = if self.tab == 0 {
            self.general(cx)
        } else {
            self.models.clone().into_any_element()
        };
        let status = self.status.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .text_size(px(13.0))
            .child(
                div()
                    .flex()
                    .justify_center()
                    .px(px(16.0))
                    .pt(px(12.0))
                    .pb(px(8.0))
                    .child(
                        TabBar::new("settings-tabs")
                            .segmented()
                            .selected_index(self.tab)
                            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                                this.tab = *ix;
                                cx.notify();
                            }))
                            .child(Tab::new().label("General"))
                            .child(Tab::new().label("Models")),
                    ),
            )
            .child(
                div()
                    .id("settings-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(16.0))
                    .pb(px(16.0))
                    .child(div().max_w(px(680.0)).mx_auto().child(body)),
            )
            .when(self.tab == 0, |this| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.0))
                        .px(px(16.0))
                        .py(px(12.0))
                        .border_t_1()
                        .border_color(border)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_color(match &status {
                                    Some((_, true)) => danger,
                                    _ => muted,
                                })
                                .child(status.map(|(s, _)| s).unwrap_or_default()),
                        )
                        .child(
                            Button::new("save")
                                .primary()
                                .label("Save")
                                .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                        ),
                )
            })
    }
}
