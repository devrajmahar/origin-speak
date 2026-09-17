use crate::theme::DesignTokens;
use gpui::*;
use gpui_base::input::{Input, InputState, Textarea, TextareaState};

/// ListenOS-owned single-line input chrome around `gpui-base` editing behavior.
///
/// The state/entity stays owned by the parent view so typed product actions can
/// subscribe to `InputEvent` without pushing product state into the component.
#[derive(IntoElement)]
pub struct ListenOsInput {
    state: Entity<InputState>,
    tokens: DesignTokens,
    width: Pixels,
}

impl ListenOsInput {
    pub fn new(state: &Entity<InputState>, tokens: DesignTokens) -> Self {
        Self {
            state: state.clone(),
            tokens,
            width: px(180.0),
        }
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }
}

impl RenderOnce for ListenOsInput {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .w(self.width)
            .min_h(px(34.0))
            .rounded(px(self.tokens.radius.default))
            .border_1()
            .border_color(self.tokens.muted_border)
            .bg(self.tokens.muted)
            .px(px(self.tokens.spacing.md))
            .py(px(self.tokens.spacing.sm))
            .text_size(px(self.tokens.type_scale.sm))
            .text_color(self.tokens.text)
            .child(Input::new(&self.state))
    }
}

/// ListenOS-owned multi-line input chrome around `gpui-base` textarea behavior.
#[derive(IntoElement)]
pub struct ListenOsTextarea {
    state: Entity<TextareaState>,
    tokens: DesignTokens,
    width: Pixels,
    min_height: Pixels,
}

impl ListenOsTextarea {
    pub fn new(state: &Entity<TextareaState>, tokens: DesignTokens) -> Self {
        Self {
            state: state.clone(),
            tokens,
            width: px(420.0),
            min_height: px(84.0),
        }
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    pub fn min_height(mut self, min_height: Pixels) -> Self {
        self.min_height = min_height;
        self
    }
}

impl RenderOnce for ListenOsTextarea {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .w(self.width)
            .min_h(self.min_height)
            .rounded(px(self.tokens.radius.default))
            .border_1()
            .border_color(self.tokens.muted_border)
            .bg(self.tokens.muted)
            .px(px(self.tokens.spacing.md))
            .py(px(self.tokens.spacing.sm))
            .text_size(px(self.tokens.type_scale.sm))
            .text_color(self.tokens.text)
            .child(Textarea::new(&self.state))
    }
}
