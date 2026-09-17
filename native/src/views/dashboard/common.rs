use super::types::ActionProps;
use crate::theme::DesignTokens;
use gpui::*;

pub(super) fn action_target<A>(
    element: Div,
    id: impl Into<ElementId>,
    actions: &ActionProps<A>,
    action: A,
) -> Stateful<Div>
where
    A: Clone + 'static,
{
    let element = element.id(id);
    let Some(callback) = actions.on_action.clone() else {
        return element;
    };

    element
        .cursor_pointer()
        .on_click(move |_, _, _| callback(action.clone()))
}

pub(super) fn page_header(
    title: impl Into<SharedString>,
    subtitle: Option<impl Into<SharedString>>,
    tokens: DesignTokens,
) -> Div {
    let mut header = div()
        .w_full()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .max_w(px(760.0))
                .line_clamp(2)
                .text_2xl()
                .text_color(tokens.text)
                .child(title.into()),
        );

    if let Some(subtitle) = subtitle {
        header = header.child(
            div()
                .max_w(px(760.0))
                .line_clamp(3)
                .text_sm()
                .text_color(tokens.text_muted)
                .child(subtitle.into()),
        );
    }

    header
}

pub(super) fn panel(tokens: DesignTokens) -> Div {
    div()
        .w_full()
        .min_w(px(0.0))
        .rounded_lg()
        .border_1()
        .border_color(tokens.border)
        .bg(tokens.card)
}

pub(super) fn primary_button(label: impl Into<SharedString>, tokens: DesignTokens) -> Div {
    div()
        .px_4()
        .py_2()
        .rounded_lg()
        .bg(tokens.primary)
        .text_sm()
        .text_color(tokens.primary_foreground)
        .cursor_pointer()
        .hover(|style| style.bg(tokens.primary_hover))
        .child(label.into())
}

pub(super) fn secondary_button(label: impl Into<SharedString>, tokens: DesignTokens) -> Div {
    div()
        .px_3()
        .py_2()
        .rounded_lg()
        .border_1()
        .border_color(tokens.muted_border)
        .bg(tokens.muted)
        .text_sm()
        .text_color(tokens.text)
        .cursor_pointer()
        .hover(|style| style.bg(tokens.accent))
        .child(label.into())
}

pub(super) fn tab(label: impl Into<SharedString>, selected: bool, tokens: DesignTokens) -> Div {
    div()
        .px_3()
        .py_2()
        .border_b_1()
        .border_color(if selected {
            tokens.primary
        } else {
            tokens.border
        })
        .text_sm()
        .text_color(if selected {
            tokens.primary
        } else {
            tokens.text_muted
        })
        .child(label.into())
}

pub(super) fn status_pill(
    label: impl Into<SharedString>,
    color: Hsla,
    tokens: DesignTokens,
) -> Div {
    div()
        .px_2()
        .py_1()
        .rounded_full()
        .border_1()
        .border_color(tokens.muted_border)
        .bg(tokens.muted)
        .text_xs()
        .text_color(color)
        .child(label.into())
}

pub(super) fn loading_panel(label: &'static str, tokens: DesignTokens) -> Div {
    panel(tokens)
        .p_6()
        .flex()
        .justify_center()
        .child(div().text_sm().text_color(tokens.text_muted).child(label))
}

pub(super) fn empty_panel(
    title: &'static str,
    description: &'static str,
    tokens: DesignTokens,
) -> Div {
    panel(tokens)
        .p_6()
        .flex()
        .flex_col()
        .items_center()
        .gap_2()
        .child(div().text_lg().text_color(tokens.text).child(title))
        .child(
            div()
                .max_w(px(520.0))
                .line_clamp(3)
                .text_sm()
                .text_color(tokens.text_muted)
                .child(description),
        )
}

pub(super) fn format_number(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);

    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }

    out
}
