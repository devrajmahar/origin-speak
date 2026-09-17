use crate::theme::DesignTokens;
use gpui::*;

/// Explicit text constraints used by native ListenOS surfaces.
///
/// Keep the intended width and line budget alongside each text surface so
/// callers do not rely on incidental parent sizing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextConstraint {
    pub max_width: f32,
    pub max_lines: usize,
}

impl TextConstraint {
    pub const fn single_line(max_width: f32) -> Self {
        Self {
            max_width,
            max_lines: 1,
        }
    }

    pub const fn lines(max_width: f32, max_lines: usize) -> Self {
        Self {
            max_width,
            max_lines,
        }
    }
}

pub const SETTINGS_NAV_LABEL: TextConstraint = TextConstraint::single_line(164.0);
pub const SETTINGS_ROW_DESCRIPTION: TextConstraint = TextConstraint::lines(300.0, 3);
pub const ONBOARDING_CHOICE_TITLE: TextConstraint = TextConstraint::single_line(316.0);
pub const ONBOARDING_CHOICE_DETAIL: TextConstraint = TextConstraint::lines(316.0, 2);
pub const ONBOARDING_BODY: TextConstraint = TextConstraint::lines(400.0, 4);

/// A one-line label with GPUI's native ellipsis behavior.
pub fn single_line_text(
    text: impl Into<SharedString>,
    constraint: TextConstraint,
    color: Hsla,
) -> impl IntoElement {
    div()
        .min_w(px(0.0))
        .max_w(px(constraint.max_width))
        .truncate()
        .text_sm()
        .text_color(color)
        .child(text.into())
}

/// Multi-line copy with an explicit maximum line count.
pub fn clamped_text(
    text: impl Into<SharedString>,
    constraint: TextConstraint,
    color: Hsla,
) -> impl IntoElement {
    div()
        .min_w(px(0.0))
        .max_w(px(constraint.max_width))
        .line_clamp(constraint.max_lines.max(1))
        .text_sm()
        .text_color(color)
        .child(text.into())
}

pub fn muted_clamped(
    text: impl Into<SharedString>,
    constraint: TextConstraint,
    tokens: DesignTokens,
) -> impl IntoElement {
    clamped_text(text, constraint, tokens.text_muted)
}
