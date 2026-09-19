#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlayState {
    #[default]
    Idle,
    Listening,
    Processing,
    Success,
    Error,
}
