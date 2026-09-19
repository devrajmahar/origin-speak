use gpui::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OriginSpeakIcon {
    Success,
    Alert,
}

impl OriginSpeakIcon {
    fn data(self) -> &'static [u8] {
        match self {
            Self::Success => include_bytes!("../../assets/icons/hugeicons/checkmark-circle-02.svg"),
            Self::Alert => include_bytes!("../../assets/icons/hugeicons/alert-02.svg"),
        }
    }
}

pub fn hugeicon(icon: OriginSpeakIcon, size: f32, color: Hsla) -> Svg {
    svg()
        .data(icon.data())
        .w(px(size))
        .h(px(size))
        .text_color(color)
}
