use gpui::*;
use gpui_base::{Theme, ThemeAppearance};

/// Tokens required by the compact resident voice overlay.
#[derive(Clone, Copy)]
pub struct DesignTokens {
    pub canvas: Hsla,
    pub muted: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub primary: Hsla,
    pub primary_foreground: Hsla,
    pub positive: Hsla,
    pub negative: Hsla,
    pub border: Hsla,
    pub ring: Hsla,
}

impl Default for DesignTokens {
    fn default() -> Self {
        Self {
            canvas: rgb(0x070a0f).into(),
            muted: rgb(0x0c1115).into(),
            text: rgb(0xfafafa).into(),
            text_muted: rgb(0x9da3aa).into(),
            primary: rgb(0x3e63dd).into(),
            primary_foreground: rgb(0xf0f4ff).into(),
            positive: rgb(0x089981).into(),
            negative: rgb(0xf7525f).into(),
            border: rgb(0x16191f).into(),
            ring: rgb(0x73777e).into(),
        }
    }
}

pub fn transparent() -> Hsla {
    hsla(0.0, 0.0, 0.0, 0.0)
}

pub fn configure_base_theme(cx: &mut App) {
    let tokens = DesignTokens::default();
    let theme = Theme::global_mut(cx);
    theme.appearance = ThemeAppearance::Dark;
    let colors = &mut theme.tokens.colors;
    colors.background = tokens.canvas;
    colors.foreground = tokens.text;
    colors.surface = tokens.muted;
    colors.surface_foreground = tokens.text;
    colors.primary = tokens.primary;
    colors.primary_foreground = tokens.primary_foreground;
    colors.secondary = tokens.muted;
    colors.secondary_foreground = tokens.text;
    colors.muted = tokens.muted;
    colors.muted_foreground = tokens.text_muted;
    colors.accent = tokens.primary;
    colors.accent_foreground = tokens.primary_foreground;
    colors.destructive = tokens.negative;
    colors.destructive_foreground = tokens.primary_foreground;
    colors.border = tokens.border;
    colors.input = tokens.border;
    colors.ring = tokens.ring;
    colors.selection = tokens.primary.alpha(0.35);
}
