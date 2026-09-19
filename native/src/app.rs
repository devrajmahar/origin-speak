use crate::runtime::{RuntimeController, RuntimeEvent};
use crate::state::OverlayState;
use crate::theme::transparent;
use gpui::{prelude::*, *};
use std::time::Duration;

pub struct OriginSpeakApp {
    _runtime: RuntimeController,
    overlay: OverlayState,
    audio_level: f32,
    status_overlay_window: Option<WindowHandle<crate::overlay::StatusOverlayView>>,
    status_overlay_opening: bool,
    _runtime_bridge: Task<()>,
}

impl OriginSpeakApp {
    pub fn new(runtime: RuntimeController, cx: &mut Context<Self>) -> Self {
        let bridge_events = runtime.events();
        let runtime_bridge = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;

                let pending = match bridge_events.lock() {
                    Ok(receiver) => receiver.try_iter().collect::<Vec<_>>(),
                    Err(_) => return,
                };
                let quit_requested = crate::platform_shell::take_quit_request();
                if pending.is_empty() && !quit_requested {
                    continue;
                }

                if this
                    .update(cx, |this, cx| {
                        if quit_requested {
                            cx.quit();
                            return;
                        }
                        for event in pending {
                            let auto_idle = match event {
                                RuntimeEvent::Success => {
                                    this.apply_runtime_state(OverlayState::Success, cx);
                                    Some((OverlayState::Success, Duration::from_millis(900)))
                                }
                                RuntimeEvent::Error => {
                                    this.apply_runtime_state(OverlayState::Error, cx);
                                    Some((OverlayState::Error, Duration::from_millis(1500)))
                                }
                                RuntimeEvent::Idle => {
                                    this.apply_runtime_state(OverlayState::Idle, cx);
                                    None
                                }
                                RuntimeEvent::Listening => {
                                    this.apply_runtime_state(OverlayState::Listening, cx);
                                    None
                                }
                                RuntimeEvent::Processing => {
                                    this.apply_runtime_state(OverlayState::Processing, cx);
                                    None
                                }
                                RuntimeEvent::AudioLevel(level) => {
                                    this.audio_level = level;
                                    cx.notify();
                                    None
                                }
                            };

                            if let Some((expected, delay)) = auto_idle {
                                cx.spawn(async move |this, cx| {
                                    cx.background_executor().timer(delay).await;
                                    let _ = this.update(cx, |this, cx| {
                                        if this.overlay == expected {
                                            this.apply_runtime_state(OverlayState::Idle, cx);
                                        }
                                    });
                                })
                                .detach();
                            }
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
        });

        Self {
            _runtime: runtime,
            overlay: OverlayState::Idle,
            audio_level: 0.0,
            status_overlay_window: None,
            status_overlay_opening: false,
            _runtime_bridge: runtime_bridge,
        }
    }

    pub(crate) fn overlay_snapshot(&self) -> (OverlayState, f32) {
        (self.overlay, self.audio_level)
    }

    fn apply_runtime_state(&mut self, state: OverlayState, cx: &mut Context<Self>) {
        self.overlay = state;
        if state != OverlayState::Listening {
            self.audio_level = 0.0;
        }
        self.reconcile_overlay(cx);
        cx.notify();
    }

    fn reconcile_overlay(&mut self, cx: &mut Context<Self>) {
        let should_be_open = self.overlay != OverlayState::Idle;
        if should_be_open && self.status_overlay_window.is_none() && !self.status_overlay_opening {
            self.status_overlay_opening = true;
            let app = cx.entity();
            cx.defer(move |cx| {
                let opened = crate::overlay::open_status_overlay(app.clone(), cx);
                let mut close_after_open = None;
                app.update(cx, |this, _| {
                    this.status_overlay_opening = false;
                    match opened {
                        Ok(window) if this.overlay != OverlayState::Idle => {
                            this.status_overlay_window = Some(window);
                        }
                        Ok(window) => close_after_open = Some(window),
                        Err(_error) => {
                            #[cfg(debug_assertions)]
                            eprintln!("[Origin Speak] could not open status overlay: {_error}");
                        }
                    }
                });
                if let Some(window) = close_after_open {
                    let _ = window.update(cx, |_, window, _| window.remove_window());
                }
            });
        } else if !should_be_open && let Some(window) = self.status_overlay_window.take() {
            cx.defer(move |cx| {
                let _ = window.update(cx, |_, window, _| window.remove_window());
            });
        }
    }
}

impl Render for OriginSpeakApp {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // The resident window is deliberately hidden. GPUI remains alive only
        // to own the platform event loop, global shortcut manager, and compact
        // overlay windows.
        div().size_full().bg(transparent())
    }
}
