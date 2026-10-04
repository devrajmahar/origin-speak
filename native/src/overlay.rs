use crate::app::OriginSpeakApp;
use crate::components::icon::{OriginSpeakIcon, icon};
use crate::state::OverlayState;
use crate::theme::{DesignTokens, transparent};
use gpui::*;
use std::f32::consts::TAU;
use std::time::{Duration, Instant};

const STATUS_WIDTH: f32 = 88.0;
const STATUS_HEIGHT: f32 = 32.0;
const BOTTOM_MARGIN: f32 = 28.0;
const CHIP_BORDER_WIDTH: f32 = 0.5;
const MARKER_WIDTH: f32 = 72.0;
const MARKER_HEIGHT: f32 = 20.0;

const BAR_COUNT: usize = 9;
const BAR_WIDTH: f32 = 4.0;
const BAR_MIN_HEIGHT: f32 = 4.0;
const BAR_MAX_HEIGHT: f32 = 18.0;
const BAR_PROFILE: [f32; BAR_COUNT] = [0.42, 0.58, 0.76, 0.9, 1.0, 0.9, 0.76, 0.58, 0.42];
/// Per-second smoothing rates: bars jump up with the voice and settle back
/// more slowly, like a VU meter.
const BAR_ATTACK_RATE: f32 = 28.0;
const BAR_RELEASE_RATE: f32 = 9.0;

const SPINNER_SIZE: f32 = 16.0;
const SPINNER_PERIOD_SECS: f32 = 0.9;
const COMPLETION_ICON_SIZE: f32 = 18.0;
const FADE_IN_SECS: f32 = 0.14;

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

/// Fixed-size box every state renders into, so switching states or animating
/// inside one never moves anything else.
fn marker_slot() -> Div {
    div()
        .w(px(MARKER_WIDTH))
        .h(px(MARKER_HEIGHT))
        .flex()
        .items_center()
        .justify_center()
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
    last_frame_at: Instant,
    bar_levels: [f32; BAR_COUNT],
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
                if state == OverlayState::Listening {
                    this.bar_levels = [0.0; BAR_COUNT];
                }
            }
            if level_changed {
                this.last_level_bucket = level_bucket;
            }
            if state_changed || level_changed {
                cx.notify();
            }
        });

        let now = Instant::now();
        Self {
            app,
            tokens: DesignTokens::default(),
            last_state: OverlayState::Listening,
            last_level_bucket: 0,
            state_started_at: now,
            last_frame_at: now,
            bar_levels: [0.0; BAR_COUNT],
            _app_subscription: subscription,
        }
    }

    fn update_bars(&mut self, level: f32, elapsed: f32, dt: f32) {
        let level = level.clamp(0.0, 1.0);
        for (index, bar) in self.bar_levels.iter_mut().enumerate() {
            let offset = index as f32;
            // The shimmer only scales the voice level, so silence stays flat.
            let shimmer = 0.72
                + 0.14 * (elapsed * 7.3 + offset * 1.9).sin()
                + 0.14 * (elapsed * 11.1 - offset * 0.8).sin();
            let target = level * BAR_PROFILE[index] * shimmer;
            let rate = if target > *bar {
                BAR_ATTACK_RATE
            } else {
                BAR_RELEASE_RATE
            };
            *bar += (target - *bar) * (1.0 - (-rate * dt).exp());
        }
    }

    fn listening_indicator(&self) -> Div {
        let mut bars = marker_slot().gap(px(2.0));
        for bar in self.bar_levels {
            let bar = bar.clamp(0.0, 1.0);
            bars = bars.child(
                div()
                    .w(px(BAR_WIDTH))
                    .h(px(BAR_MIN_HEIGHT + bar * (BAR_MAX_HEIGHT - BAR_MIN_HEIGHT)))
                    .rounded_full()
                    .bg(self.tokens.primary.alpha(0.5 + bar * 0.5)),
            );
        }
        bars
    }

    fn processing_indicator(&self, elapsed: f32) -> Div {
        let fade = smoothstep(elapsed / FADE_IN_SECS);
        let angle = (elapsed / SPINNER_PERIOD_SECS).fract() * TAU;
        marker_slot().child(
            div()
                .relative()
                .size(px(SPINNER_SIZE))
                .child(
                    icon(
                        OriginSpeakIcon::SpinnerTrack,
                        SPINNER_SIZE,
                        self.tokens.primary.alpha(0.22 * fade),
                    )
                    .absolute()
                    .top_0()
                    .left_0(),
                )
                .child(
                    icon(
                        OriginSpeakIcon::SpinnerArc,
                        SPINNER_SIZE,
                        self.tokens.primary.alpha(fade),
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .with_transformation(Transformation::rotate(radians(angle))),
                ),
        )
    }

    /// The pop is a paint-time transform, so the icon's layout box never changes.
    fn completion_indicator(&self, kind: OriginSpeakIcon, color: Hsla, elapsed: f32) -> Div {
        let pop = completion_pop(elapsed);
        let fade = smoothstep(elapsed / FADE_IN_SECS);
        marker_slot().child(
            icon(kind, COMPLETION_ICON_SIZE, color.alpha(fade))
                .with_transformation(Transformation::scale(size(pop, pop))),
        )
    }

    fn state_marker(&self, state: OverlayState, elapsed: f32) -> Div {
        match state {
            OverlayState::Listening => self.listening_indicator(),
            OverlayState::Processing => self.processing_indicator(elapsed),
            OverlayState::Success => {
                self.completion_indicator(OriginSpeakIcon::Success, self.tokens.positive, elapsed)
            }
            OverlayState::Error => {
                self.completion_indicator(OriginSpeakIcon::Alert, self.tokens.negative, elapsed)
            }
            OverlayState::Idle => marker_slot(),
        }
    }
}

impl Render for StatusOverlayView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (state, level) = self.app.read(cx).overlay_snapshot();
        let now = Instant::now();
        let dt = now
            .duration_since(self.last_frame_at)
            .as_secs_f32()
            .min(0.1);
        self.last_frame_at = now;
        if state == OverlayState::Idle {
            return div().size_full().bg(transparent());
        }

        window.request_animation_frame();
        let elapsed = self.state_started_at.elapsed().as_secs_f32();
        if state == OverlayState::Listening {
            self.update_bars(level, elapsed, dt);
        }

        div()
            .size_full()
            .border(px(CHIP_BORDER_WIDTH))
            .border_color(self.tokens.chip_border)
            .bg(self.tokens.chip_surface)
            .flex()
            .items_center()
            .justify_center()
            .child(self.state_marker(state, elapsed))
    }
}
