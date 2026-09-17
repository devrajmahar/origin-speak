use crate::app::ListenOsApp;
use crate::components::button::{ListenOsButton, ListenOsButtonVariant};
use crate::state::OverlayState;
use crate::theme::{DesignTokens, transparent};
use gpui::*;
use std::time::{Duration, Instant};

const STATUS_WIDTH: f32 = 88.0;
const STATUS_HEIGHT: f32 = 32.0;
const CONTROLS_WIDTH: f32 = 380.0;
const CONTROLS_HEIGHT: f32 = 112.0;
const BOTTOM_MARGIN: f32 = 28.0;
fn bottom_center_bounds(display_bounds: Bounds<Pixels>, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(
            display_bounds.center().x - px(width / 2.0),
            display_bounds.bottom() - px(height + BOTTOM_MARGIN),
        ),
        size: size(px(width), px(height)),
    }
}

pub(crate) fn open_status_overlay(
    app: Entity<ListenOsApp>,
    cx: &mut App,
) -> Result<WindowHandle<StatusOverlayView>, String> {
    let display = cx
        .primary_display()
        .ok_or_else(|| "No primary display is available for the status overlay".to_string())?;
    let status_bounds = bottom_center_bounds(display.bounds(), STATUS_WIDTH, STATUS_HEIGHT);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(status_bounds)),
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        inactive_frame_interval: Some(Duration::from_millis(16)),
        is_resizable: false,
        is_minimizable: false,
        display_id: Some(display.id()),
        window_background: WindowBackgroundAppearance::Opaque,
        window_decorations: Some(WindowDecorations::Client),
        ..WindowOptions::default()
    };

    cx.open_window(options, move |window, cx| {
        cx.new(|cx| StatusOverlayView::new(app, window, cx))
    })
    .map_err(|error| error.to_string())
}

pub(crate) fn open_controls_overlay(
    app: Entity<ListenOsApp>,
    cx: &mut App,
) -> Result<WindowHandle<OverlayControlsView>, String> {
    let display = cx
        .primary_display()
        .ok_or_else(|| "No primary display is available for the controls overlay".to_string())?;
    let bounds = bottom_center_bounds(display.bounds(), CONTROLS_WIDTH, CONTROLS_HEIGHT);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id: Some(display.id()),
        window_background: WindowBackgroundAppearance::Transparent,
        window_decorations: Some(WindowDecorations::Client),
        ..WindowOptions::default()
    };

    cx.open_window(options, move |window, cx| {
        cx.new(|cx| OverlayControlsView::new(app, window, cx))
    })
    .map_err(|error| error.to_string())
}

pub(crate) struct StatusOverlayView {
    app: Entity<ListenOsApp>,
    tokens: DesignTokens,
    last_state: OverlayState,
    last_level_bucket: u8,
    state_started_at: Instant,
    smoothed_level: f32,
    _app_subscription: Subscription,
}

impl StatusOverlayView {
    fn new(app: Entity<ListenOsApp>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe_in(&app, window, |this, app, _window, cx| {
            let (state, level) = app.read(cx).overlay_snapshot();
            let level_bucket = (level.clamp(0.0, 1.0) * 20.0).round() as u8;
            let state_changed = this.last_state != state;
            let level_changed = this.last_level_bucket != level_bucket;
            if state_changed {
                this.last_state = state;
                this.state_started_at = Instant::now();
            }
            if level_changed {
                this.last_level_bucket = level_bucket;
            }
            if state_changed || level_changed {
                cx.notify();
            }
        });

        Self {
            app,
            tokens: DesignTokens::default(),
            last_state: OverlayState::Listening,
            last_level_bucket: 0,
            state_started_at: Instant::now(),
            smoothed_level: 0.0,
            _app_subscription: subscription,
        }
    }

    fn listening_indicator(&self, level: f32, phase: f32) -> Div {
        let energy = level.clamp(0.0, 1.0);
        let amplitude = 4.0 + energy * 6.0;
        let offset = phase * std::f32::consts::TAU;
        let mut bars = div()
            .w(px(72.0))
            .h(px(20.0))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(2.0));
        for index in 0..9 {
            let wave = (offset + index as f32 * 0.62).sin();
            let height = (6.0 + amplitude * (0.5 + 0.5 * wave)).clamp(5.0, 19.0);
            bars = bars.child(
                div()
                    .w(px(4.0))
                    .h(px(height))
                    .rounded_full()
                    .bg(self.tokens.primary),
            );
        }
        bars
    }

    fn processing_indicator(&self, phase: f32) -> Div {
        let mut dots = div()
            .w(px(72.0))
            .h(px(20.0))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.0));
        for index in 0..3 {
            let pulse = ((phase * std::f32::consts::TAU + index as f32 * 2.1).sin() + 1.0) * 0.5;
            dots = dots.child(
                div()
                    .size(px(8.0))
                    .rounded_full()
                    .bg(self.tokens.primary.alpha(0.24 + pulse * 0.76)),
            );
        }
        dots
    }

    fn state_marker(&self, state: OverlayState, level: f32, phase: f32) -> Div {
        match state {
            OverlayState::Listening => self.listening_indicator(level, phase),
            OverlayState::Processing => self.processing_indicator(phase),
            OverlayState::Success => {
                let pulse = ((phase * std::f32::consts::TAU).sin() + 1.0) * 0.5;
                div()
                    .w(px(72.0))
                    .h(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .size(px(16.0))
                            .rounded_full()
                            .bg(self.tokens.positive.alpha(0.30 + pulse * 0.32))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .text_color(self.tokens.positive)
                            .child("\u{2713}"),
                    )
            }
            OverlayState::Error => {
                let pulse = ((phase * std::f32::consts::TAU).sin() + 1.0) * 0.5;
                div()
                    .w(px(72.0))
                    .h(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .size(px(16.0))
                            .rounded_full()
                            .bg(self.tokens.negative.alpha(0.30 + pulse * 0.32))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .text_color(self.tokens.negative)
                            .child("!"),
                    )
            }
            _ => div().w(px(72.0)).h(px(20.0)),
        }
    }
}

impl Render for StatusOverlayView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (state, level) = self.app.read(cx).overlay_snapshot();
        if !matches!(
            state,
            OverlayState::Listening
                | OverlayState::Processing
                | OverlayState::Success
                | OverlayState::Error
        ) {
            return div().size_full().bg(transparent());
        }

        if matches!(
            state,
            OverlayState::Listening
                | OverlayState::Processing
                | OverlayState::Success
                | OverlayState::Error
        ) {
            window.request_animation_frame();
        }
        let target_level = level.clamp(0.0, 1.0);
        self.smoothed_level += (target_level - self.smoothed_level) * 0.16;
        let phase = (self.state_started_at.elapsed().as_secs_f32() / 0.72).fract();

        div()
            .size_full()
            .bg(self.tokens.muted)
            .flex()
            .items_center()
            .justify_center()
            .child(self.state_marker(state, self.smoothed_level, phase))
    }
}

pub(crate) struct OverlayControlsView {
    app: Entity<ListenOsApp>,
    tokens: DesignTokens,
    last_state: OverlayState,
    _app_subscription: Subscription,
}

impl OverlayControlsView {
    fn new(app: Entity<ListenOsApp>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe_in(&app, window, |this, app, _window, cx| {
            let (state, _) = app.read(cx).overlay_controls_snapshot();
            if this.last_state != state {
                this.last_state = state;
                cx.notify();
            }
        });

        Self {
            app,
            tokens: DesignTokens::default(),
            last_state: OverlayState::Handsfree,
            _app_subscription: subscription,
        }
    }

    fn confirmation_controls(&self, summary: String, cx: &mut Context<Self>) -> AnyElement {
        let tokens = self.tokens;
        let confirm_app = self.app.clone();
        let cancel_app = self.app.clone();
        div()
            .size_full()
            .rounded(px(10.0))
            .border_1()
            .border_color(tokens.border)
            .bg(tokens.muted)
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(tokens.text)
                    .child("Confirmation required"),
            )
            .child(
                div()
                    .line_clamp(1)
                    .text_xs()
                    .text_color(tokens.text_muted)
                    .child(summary),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        ListenOsButton::new("overlay-confirm", "Confirm", tokens).on_click(
                            cx.listener(move |_, _, _, cx| {
                                confirm_app.update(cx, |app, cx| {
                                    app.overlay_confirm_pending(cx);
                                    cx.notify();
                                });
                            }),
                        ),
                    )
                    .child(
                        ListenOsButton::new("overlay-cancel-confirmation", "Cancel", tokens)
                            .variant(ListenOsButtonVariant::Secondary)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cancel_app.update(cx, |app, cx| {
                                    app.overlay_cancel_pending(cx);
                                    cx.notify();
                                });
                            })),
                    ),
            )
            .into_any_element()
    }

    fn handsfree_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let tokens = self.tokens;
        let cancel_app = self.app.clone();
        let stop_app = self.app.clone();
        div()
            .size_full()
            .rounded(px(10.0))
            .border_1()
            .border_color(tokens.border)
            .bg(tokens.muted)
            .p_3()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .text_color(tokens.text)
                            .child("Listening hands-free"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(tokens.text_muted)
                            .child("Press Stop to process or Cancel to discard"),
                    ),
            )
            .child(
                ListenOsButton::new("overlay-cancel-handsfree", "Cancel", tokens)
                    .variant(ListenOsButtonVariant::Secondary)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cancel_app.update(cx, |app, cx| {
                            app.overlay_cancel_capture(cx);
                            cx.notify();
                        });
                    })),
            )
            .child(
                ListenOsButton::new("overlay-stop-handsfree", "Stop", tokens)
                    .variant(ListenOsButtonVariant::Destructive)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        stop_app.update(cx, |app, cx| {
                            app.overlay_stop_handsfree(cx);
                            cx.notify();
                        });
                    })),
            )
            .into_any_element()
    }
}

impl Render for OverlayControlsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (state, summary) = self.app.read(cx).overlay_controls_snapshot();
        match state {
            OverlayState::ConfirmationRequired => self.confirmation_controls(summary, cx),
            OverlayState::Handsfree => self.handsfree_controls(cx),
            _ => div().size_full().bg(transparent()).into_any_element(),
        }
    }
}
