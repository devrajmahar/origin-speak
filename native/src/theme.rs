use gpui::*;

/// ListenOS application-owned dark UI tokens.
#[derive(Clone, Copy)]
pub struct DesignTokens {
    pub canvas: Hsla,
    pub card: Hsla,
    pub muted: Hsla,
    pub accent: Hsla,
    pub border: Hsla,
    pub muted_border: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_disabled: Hsla,
    pub primary: Hsla,
    pub primary_hover: Hsla,
    pub primary_foreground: Hsla,
    pub positive: Hsla,
    pub negative: Hsla,
    pub warning: Hsla,
    pub ring: Hsla,
    pub overlay_scrim: Hsla,
    pub spacing: SpacingTokens,
    pub radius: RadiusTokens,
    pub type_scale: TypeScale,
}

/// ListenOS spacing is intentionally explicit. Values are logical pixels and
/// therefore track GPUI's per-display scale factor.
#[derive(Clone, Copy, Debug)]
pub struct SpacingTokens {
    pub sm: f32,
    pub md: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct RadiusTokens {
    pub default: f32,
    pub pill: f32,
}

/// Typography metrics used by native views. The application uses the system
/// sans face with explicit sizes so rendering stays consistent across native
/// surfaces.
#[derive(Clone, Copy, Debug)]
pub struct TypeScale {
    pub sm: f32,
}

impl Default for DesignTokens {
    fn default() -> Self {
        Self {
            canvas: rgb(0x070a0f).into(),
            card: rgb(0x070a0f).into(),
            muted: rgb(0x0c1115).into(),
            accent: rgb(0x222224).into(),
            border: rgb(0x16191f).into(),
            muted_border: rgb(0x131519).into(),
            text: rgb(0xfafafa).into(),
            text_muted: rgb(0x9da3aa).into(),
            text_disabled: rgb(0x777b82).into(),
            // sRGB approximation of brand.css oklch(0.54375 0.191015 267.005).
            primary: rgb(0x3e63dd).into(),
            primary_hover: rgb(0x4e76f2).into(),
            primary_foreground: rgb(0xf0f4ff).into(),
            positive: rgb(0x089981).into(),
            negative: rgb(0xf7525f).into(),
            warning: rgb(0xf59e0b).into(),
            ring: rgb(0x73777e).into(),
            overlay_scrim: hsla(0.0, 0.0, 0.0, 0.5),
            spacing: SpacingTokens { sm: 8.0, md: 12.0 },
            radius: RadiusTokens {
                default: 6.0,
                pill: 999.0,
            },
            type_scale: TypeScale { sm: 13.0 },
        }
    }
}

pub fn transparent() -> Hsla {
    hsla(0.0, 0.0, 0.0, 0.0)
}
