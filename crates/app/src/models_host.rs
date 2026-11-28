//! Runs the Models tab's commands on the core manager (the app's tokio runtime) and feeds its status and
//! download progress back into the open Settings window.

use std::sync::Arc;

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{AnyWindowHandle, App, Entity, Global, WeakEntity, Window};
use screen_translate_app::models::{ModelsCommand, ModelsView, Progress};
use screen_translate_app::settings::SettingsView;
use screen_translate_core::manager::runtime::Backend;
use screen_translate_core::manager::server::Status;
use screen_translate_core::manager::{self, Events, ProgressEvent};

use crate::AppState;

enum Msg {
    Status(Status),
    Progress(Progress),
}

struct Target {
    models: WeakEntity<ModelsView>,
    settings: WeakEntity<SettingsView>,
    window: AnyWindowHandle,
}

/// The manager's events always go to the Settings window that is open now.
struct Host {
    events: Events,
    target: Option<Target>,
    /// Refresh counter: a snapshot older than the newest applied one is dropped.
    refreshes: u64,
    applied: u64,
}

impl Global for Host {}

/// Starts forwarding manager events (they arrive on any thread) to the gpui main thread.
pub fn init(cx: &mut App) {
    let (tx, mut rx) = unbounded();
    let (status_tx, progress_tx): (UnboundedSender<Msg>, _) = (tx.clone(), tx);
    let events = Events {
        status: Arc::new(move |s| {
            let _ = status_tx.unbounded_send(Msg::Status(s));
        }),
        progress: Arc::new(move |p: ProgressEvent<'_>| {
            let _ = progress_tx.unbounded_send(Msg::Progress(Progress {
                job: p.job.into(),
                file: p.file.into(),
                downloaded: p.downloaded,
                total: p.total,
            }));
        }),
    };
    cx.set_global(Host {
        events,
        target: None,
        refreshes: 0,
        applied: 0,
    });
    cx.spawn(async move |cx| {
        while let Some(msg) = rx.next().await {
            cx.update(|cx| on_msg(msg, cx));
        }
    })
    .detach();
}

/// The manager's events, for anything that starts the server (autostart).
pub fn events(cx: &App) -> Events {
    cx.global::<Host>().events.clone()
}

/// Drives the Models tab of `settings` (open in `window`) and loads its first status.
pub fn attach(cx: &mut App, window: AnyWindowHandle, settings: &Entity<SettingsView>) {
    let models = settings.read(cx).models().clone();
    cx.global_mut::<Host>().target = Some(Target {
        models: models.downgrade(),
        settings: settings.downgrade(),
        window,
    });
    cx.subscribe(&models, |_, command: &ModelsCommand, cx| {
        handle(command.clone(), cx)
    })
    .detach();
    refresh(cx, false);
}

fn with_models(
    cx: &mut App,
    f: impl FnOnce(&mut ModelsView, &mut Window, &mut gpui::Context<ModelsView>),
) {
    let Some(t) = cx.global::<Host>().target.as_ref() else {
        return;
    };
    let (models, window) = (t.models.clone(), t.window);
    let _ = window.update(cx, |_, window, cx| {
        if let Some(m) = models.upgrade() {
            m.update(cx, |v, cx| f(v, window, cx));
        }
    });
}

fn with_settings(
    cx: &mut App,
    f: impl FnOnce(&mut SettingsView, &mut Window, &mut gpui::Context<SettingsView>),
) {
    let Some(t) = cx.global::<Host>().target.as_ref() else {
        return;
    };
    let (settings, window) = (t.settings.clone(), t.window);
    let _ = window.update(cx, |_, window, cx| {
        if let Some(s) = settings.upgrade() {
            s.update(cx, |v, cx| f(v, window, cx));
        }
    });
}

fn show_status(cx: &mut App, result: Result<String, String>) {
    with_settings(cx, |s, _, cx| s.show_status(result, cx));
}

fn on_msg(msg: Msg, cx: &mut App) {
    match msg {
        Msg::Progress(p) => with_models(cx, |v, _, cx| v.progress(p, cx)),
        Msg::Status(s) => refresh(cx, matches!(s, Status::Ready { .. })),
    }
}

/// Loads `manager::status` into the view; the Local provider fields reload too when `reload_local`.
fn refresh(cx: &mut App, reload_local: bool) {
    // Nobody to show it to: skip the disk scan.
    let host = cx.global_mut::<Host>();
    if !host
        .target
        .as_ref()
        .is_some_and(|t| t.models.upgrade().is_some())
    {
        return;
    }
    host.refreshes += 1;
    let seq = host.refreshes;
    let state = cx.global::<AppState>();
    let (rt, manager, settings) = (
        state.runtime.handle().clone(),
        state.manager.clone(),
        state.settings.clone(),
    );
    cx.spawn(async move |cx| {
        let task = rt.spawn_blocking(move || manager::status(&manager, &settings));
        let result = task.await.unwrap_or_else(|e| Err(e.to_string()));
        cx.update(|cx| {
            let host = cx.global_mut::<Host>();
            if seq < host.applied {
                return;
            }
            host.applied = seq;
            if let Err(e) = &result {
                show_status(cx, Err(e.clone()));
            }
            with_models(cx, |v, window, cx| v.refreshed(result, window, cx));
            if reload_local {
                with_settings(cx, |s, window, cx| s.reload_local(window, cx));
            }
        });
    })
    .detach();
}

/// Runs `work` on the runtime, then shows `done` (or its error) in the footer and refreshes the tab.
fn run(
    cx: &mut App,
    work: impl FnOnce(
        Arc<manager::ManagerState>,
        Arc<screen_translate_core::settings::SettingsStore>,
        Events,
    ) -> tokio::task::JoinHandle<Result<(), String>>,
    done: Option<&'static str>,
    reload_local: bool,
) {
    show_status(cx, Ok(String::new()));
    let state = cx.global::<AppState>();
    let events = cx.global::<Host>().events.clone();
    let task = work(state.manager.clone(), state.settings.clone(), events);
    cx.spawn(async move |cx| {
        let result = task.await.unwrap_or_else(|e| Err(e.to_string()));
        cx.update(|cx| {
            show_status(cx, result.map(|()| done.unwrap_or_default().to_string()));
            refresh(cx, reload_local);
        });
    })
    .detach();
}

fn handle(command: ModelsCommand, cx: &mut App) {
    let done = command.done_message();
    let rt = cx.global::<AppState>().runtime.handle().clone();
    match command {
        ModelsCommand::Cancel => manager::cancel_download(&cx.global::<AppState>().manager),
        ModelsCommand::Search { seq, query } => {
            let manager = cx.global::<AppState>().manager.clone();
            cx.spawn(async move |cx| {
                let r = rt
                    .spawn(async move { manager::hf_search(&manager, &query).await })
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()));
                cx.update(|cx| with_models(cx, |v, _, cx| v.searched(seq, r, cx)));
            })
            .detach();
        }
        ModelsCommand::Files(repo) => {
            let manager = cx.global::<AppState>().manager.clone();
            cx.spawn(async move |cx| {
                let name = repo.clone();
                let r = rt
                    .spawn(async move { manager::hf_files(&manager, &repo).await })
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()));
                cx.update(|cx| with_models(cx, |v, _, cx| v.files_loaded(&name, r, cx)));
            })
            .detach();
        }
        ModelsCommand::Use { file, stop_first } => run(
            cx,
            move |m, settings, events| {
                // `start` needs the runtime context; stopping and spawning the process block.
                rt.spawn_blocking(move || {
                    manager::set_note(&m, None);
                    if stop_first {
                        m.server.stop();
                    }
                    manager::start(&m, &file, &events)?;
                    settings.set_managed_model(Some(&file))?;
                    settings.use_managed_server()
                })
            },
            done,
            true,
        ),
        ModelsCommand::Stop => run(
            cx,
            move |m, _, _| {
                rt.spawn_blocking(move || {
                    manager::set_note(&m, None);
                    m.server.stop();
                    Ok(())
                })
            },
            done,
            false,
        ),
        ModelsCommand::Install(backend) => run(
            cx,
            move |m, _, events| {
                rt.spawn(async move {
                    manager::install_backend(&m, &events, backend)
                        .await
                        .map(|_| ())
                })
            },
            done,
            false,
        ),
        ModelsCommand::SwitchVulkan => run(
            cx,
            move |m, _, events| {
                rt.spawn(async move {
                    manager::set_note(&m, None);
                    let stopper = m.clone();
                    tokio::task::spawn_blocking(move || stopper.server.stop())
                        .await
                        .map_err(|e| e.to_string())?;
                    manager::install_backend(&m, &events, Some(Backend::Vulkan))
                        .await
                        .map(|_| ())
                })
            },
            done,
            false,
        ),
        ModelsCommand::DownloadCatalog(id) => run(
            cx,
            move |m, _, events| {
                rt.spawn(async move {
                    manager::download_catalog(&m, &events, &id)
                        .await
                        .map(|_| ())
                })
            },
            done,
            false,
        ),
        ModelsCommand::DownloadModel(source) => run(
            cx,
            move |m, _, events| {
                rt.spawn(async move {
                    manager::download_model(&m, &events, &source)
                        .await
                        .map(|_| ())
                })
            },
            done,
            false,
        ),
        ModelsCommand::Delete(file) => run(
            cx,
            move |m, settings, _| {
                rt.spawn_blocking(move || manager::delete_model(&m, &settings, &file))
            },
            done,
            false,
        ),
        ModelsCommand::SetAutostart(enabled) => run(
            cx,
            move |_, settings, _| {
                rt.spawn_blocking(move || settings.set_managed_autostart(enabled))
            },
            done,
            false,
        ),
    }
}
