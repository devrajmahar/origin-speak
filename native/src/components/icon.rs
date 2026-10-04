use gpui::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OriginSpeakIcon {
    Success,
    Alert,
    SpinnerTrack,
    SpinnerArc,
}

impl OriginSpeakIcon {
    fn data(self) -> &'static [u8] {
        match self {
            Self::Success => {
                include_bytes!("../../assets/icons/hugeicons/checkmark-circle-02-solid.svg")
            }
            Self::Alert => include_bytes!("../../assets/icons/hugeicons/alert-circle-solid.svg"),
            Self::SpinnerTrack => include_bytes!("../../assets/icons/spinner-track.svg"),
            Self::SpinnerArc => include_bytes!("../../assets/icons/spinner-arc.svg"),
        }
    }
}

pub fn icon(kind: OriginSpeakIcon, size: f32, color: Hsla) -> Svg {
    svg()
        .data(kind.data())
        .w(px(size))
        .h(px(size))
        .text_color(color)
}
