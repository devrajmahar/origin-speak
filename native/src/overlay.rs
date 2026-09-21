use crate::app::OriginSpeakApp;
use crate::components::icon::{OriginSpeakIcon, hugeicon};
use crate::state::OverlayState;
use crate::theme::{DesignTokens, transparent};
use gpui::*;
use std::time::{Duration, Instant};

const STATUS_WIDTH: f32 = 88.0;
const STATUS_HEIGHT: f32 = 32.0;
const BOTTOM_MARGIN: f32 = 28.0;
const CHIP_BORDER_WIDTH: f32 = 0.5;

fn smoothstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

fn completion_pop(elapsed: f32) -> f32 {
    let settle = 1.0 - (-elapsed * 12.0).exp();
    let bounce = (-elapsed * 7.0).exp() * (elapsed * 22.0).sin() * 0.16;
    (settle + bounce).clamp(0.72, 1.08)
}

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
    app: Entity<OriginSpeakApp>,
    cx: &mut App,
) -> Result<WindowHandle<StatusOverlayView>, String> {
    let display = cx
        .primary_display()
        .ok_or_else(|| "No primary display is available for the status overlay".to_string())?;
    let bounds = bottom_center_bounds(display.bounds(), STATUS_WIDTH, STATUS_HEIGHT);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
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

pub(crate) struct StatusOverlayView {
    app: Entity<OriginSpeakApp>,
    tokens: DesignTokens,
    last_state: OverlayState,
    last_level_bucket: u8,
    state_started_at: Instant,
    smoothed_level: f32,
    _app_subscription: Subscription,
}

impl StatusOverlayView {
    fn new(app: Entity<OriginSpeakApp>, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
            let wave = ((phase * std::f32::consts::TAU - index as f32 * 1.45).sin() + 1.0) * 0.5;
            let pulse = smoothstep(wave);
            dots = dots.child(
                div()
                    .size(px(5.5 + pulse * 3.0))
                    .rounded_full()
                    .bg(self.tokens.primary.alpha(0.28 + pulse * 0.72)),
            );
        }
        dots
    }

    fn state_marker(&self, state: OverlayState, level: f32, phase: f32, elapsed: f32) -> Div {
        match state {
            OverlayState::Listening => self.listening_indicator(level, phase),
            OverlayState::Processing => self.processing_indicator(phase),
            OverlayState::Success => {
                let pop = completion_pop(elapsed);
                div()
                    .w(px(72.0))
                    .h(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .size(px(16.0 * pop))
                            .rounded_full()
                            .bg(self.tokens.positive.alpha(0.42))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(hugeicon(
                                OriginSpeakIcon::Success,
                                13.0,
                                self.tokens.positive,
                            )),
                    )
            }
            OverlayState::Error => {
                let pop = completion_pop(elapsed);
                div()
                    .w(px(72.0))
                    .h(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .size(px(16.0 * pop))
                            .rounded_full()
                            .bg(self.tokens.negative.alpha(0.42))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(hugeicon(OriginSpeakIcon::Alert, 13.0, self.tokens.negative)),
                    )
            }
            OverlayState::Idle => div().w(px(72.0)).h(px(20.0)),
        }
    }
}

impl Render for StatusOverlayView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (state, level) = self.app.read(cx).overlay_snapshot();
        if state == OverlayState::Idle {
            return div().size_full().bg(transparent());
        }

        window.request_animation_frame();
        let target_level = level.clamp(0.0, 1.0);
        self.smoothed_level += (target_level - self.smoothed_level) * 0.16;
        let elapsed = self.state_started_at.elapsed().as_secs_f32();
        let phase = (elapsed / 0.82).fract();

        div()
            .size_full()
            .border(px(CHIP_BORDER_WIDTH))
            .border_color(self.tokens.chip_border)
            .bg(self.tokens.chip_surface)
            .flex()
            .items_center()
            .justify_center()
            .child(self.state_marker(state, self.smoothed_level, phase, elapsed))
    }
}
