use std::rc::Rc;

use crate::theme::DesignTokens;
use gpui::*;
use gpui_base::{Switch, SwitchThumb, SwitchTrack};

type ChangeHandler = Rc<dyn Fn(bool, &ClickEvent, &mut Window, &mut App)>;

/// ListenOS-owned switch styling on top of `gpui_base::Switch` semantics.
///
/// The value is controlled by the caller: `on_change` reports the next value,
/// and the caller renders that value back through `checked`.
#[derive(IntoElement)]
pub struct ListenOsSwitch {
    id: ElementId,
    checked: bool,
    accessibility_label: SharedString,
    tokens: DesignTokens,
    disabled: bool,
    tab_index: isize,
    tab_stop: bool,
    on_change: Option<ChangeHandler>,
}

impl ListenOsSwitch {
    pub fn new(
        id: impl Into<ElementId>,
        checked: bool,
        accessibility_label: impl Into<SharedString>,
        tokens: DesignTokens,
    ) -> Self {
        Self {
            id: id.into(),
            checked,
            accessibility_label: accessibility_label.into(),
            tokens,
            disabled: false,
            tab_index: 0,
            tab_stop: true,
            on_change: None,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_change(
        mut self,
        handler: impl Fn(bool, &ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for ListenOsSwitch {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let tokens = self.tokens;
        let checked = self.checked;
        let disabled = self.disabled;
        let switch_id = self.id.clone();

        let thumb = SwitchThumb::new(checked)
            .disabled(disabled)
            .size(px(16.0))
            .rounded(px(tokens.radius.pill))
            .bg(tokens.primary_foreground)
            .styles(|styles| {
                styles
                    .checked(|style| style.ml(px(16.0)))
                    .disabled(|style| style.opacity(0.7))
            });

        let track = SwitchTrack::new((switch_id, "track"))
            .checked(checked)
            .disabled(disabled)
            .size_full()
            .p(px(2.0))
            .rounded(px(tokens.radius.pill))
            .bg(tokens.accent)
            .styles(|styles| {
                styles
                    .checked(|style| style.bg(tokens.primary))
                    .disabled(|style| style.opacity(0.45))
            })
            .child(thumb);

        let switch = Switch::new(self.id)
            .checked(checked)
            .disabled(disabled)
            .accessibility_label(self.accessibility_label)
            .tab_index(self.tab_index)
            .tab_stop(self.tab_stop)
            .w(px(36.0))
            .h(px(20.0))
            .rounded(px(tokens.radius.pill))
            .focus_visible(|style| style.border_1().border_color(tokens.ring))
            .child(track);

        if let Some(on_change) = self.on_change {
            switch.on_change(move |value, event, window, cx| on_change(value, event, window, cx))
        } else {
            switch
        }
    }
}
