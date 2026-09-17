use std::rc::Rc;

use crate::theme::DesignTokens;
use gpui::*;
use gpui_base::{Button, StyledExt as _};

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ListenOsButtonVariant {
    #[default]
    Primary,
    Secondary,
    Ghost,
    Destructive,
}

/// ListenOS-owned button styling on top of `gpui_base::Button` behavior.
///
/// The base primitive keeps keyboard/focus/accessibility behavior centralized;
/// this wrapper owns only product visuals and the small API application views
/// need when replacing ad-hoc clickable divs.
#[derive(IntoElement)]
pub struct ListenOsButton {
    id: ElementId,
    label: SharedString,
    accessibility_label: Option<SharedString>,
    tokens: DesignTokens,
    variant: ListenOsButtonVariant,
    disabled: bool,
    on_click: Option<ClickHandler>,
}

impl ListenOsButton {
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        tokens: DesignTokens,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            accessibility_label: None,
            tokens,
            variant: ListenOsButtonVariant::Primary,
            disabled: false,
            on_click: None,
        }
    }

    pub fn variant(mut self, variant: ListenOsButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Overrides the accessible name. By default the visible label is used.
    pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessibility_label = Some(label.into());
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for ListenOsButton {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let tokens = self.tokens;
        let accessible_name = self
            .accessibility_label
            .unwrap_or_else(|| self.label.clone());

        let button = Button::new(self.id)
            .accessibility_label(accessible_name)
            .disabled(self.disabled)
            .px(px(tokens.spacing.md))
            .py(px(tokens.spacing.sm))
            .rounded(px(tokens.radius.default))
            .border_1()
            .text_size(px(tokens.type_scale.sm))
            .font_medium()
            .styles(|styles| {
                styles.disabled(|style| style.opacity(0.45).text_color(tokens.text_disabled))
            })
            .active(|style| style.opacity(0.82))
            .focus_visible(|style| style.border_color(tokens.ring));

        let button = match self.variant {
            ListenOsButtonVariant::Primary => button
                .bg(tokens.primary)
                .border_color(tokens.primary)
                .text_color(tokens.primary_foreground)
                .hover(|style| style.bg(tokens.primary_hover)),
            ListenOsButtonVariant::Secondary => button
                .bg(tokens.muted)
                .border_color(tokens.border)
                .text_color(tokens.text)
                .hover(|style| style.bg(tokens.accent)),
            ListenOsButtonVariant::Ghost => button
                .bg(crate::theme::transparent())
                .border_color(crate::theme::transparent())
                .text_color(tokens.text)
                .hover(|style| style.bg(tokens.accent)),
            ListenOsButtonVariant::Destructive => button
                .bg(tokens.negative)
                .border_color(tokens.negative)
                .text_color(tokens.primary_foreground)
                .hover(|style| style.opacity(0.9)),
        };

        let button = if let Some(on_click) = self.on_click {
            button.on_click(move |event, window, cx| on_click(event, window, cx))
        } else {
            button
        };

        button.child(self.label)
    }
}
