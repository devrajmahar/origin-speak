use super::common::{
    action_target, empty_panel, loading_panel, page_header, panel, primary_button,
    secondary_button, status_pill, tab,
};
use super::types::*;
use crate::components::input::{ListenOsInput, ListenOsTextarea};
use crate::components::switch::ListenOsSwitch;
use crate::theme::DesignTokens;
use gpui::*;
use gpui_base::input::{InputState, TextareaState};

const ROW_COPY_MAX_WIDTH: f32 = 720.0;
const CARD_COPY_MAX_WIDTH: f32 = 360.0;
const EDITOR_FIELD_WIDTH: f32 = 520.0;

pub struct CommandEditorInputs {
    pub editing: bool,
    pub name: Entity<InputState>,
    pub trigger: Entity<InputState>,
    pub description: Entity<TextareaState>,
    pub action_rows: Vec<CommandActionEditorInputs>,
}

pub struct CommandActionEditorInputs {
    pub action_type: Entity<InputState>,
    pub payload: Entity<InputState>,
    pub delay_ms: Entity<InputState>,
}

pub struct DictionaryEditorInputs {
    pub editing: bool,
    pub word: Entity<InputState>,
    pub phonetic: Entity<InputState>,
}

pub struct SnippetEditorInputs {
    pub editing: bool,
    pub trigger: Entity<InputState>,
    pub expansion: Entity<TextareaState>,
}

fn editor_field(
    label: impl Into<SharedString>,
    help: impl Into<SharedString>,
    input: AnyElement,
    tokens: DesignTokens,
) -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_sm().text_color(tokens.text).child(label.into()))
        .child(input)
        .child(
            div()
                .text_xs()
                .text_color(tokens.text_muted)
                .child(help.into()),
        )
}

fn route_frame(header: Div, content: Div) -> Div {
    div()
        .w_full()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap_6()
        .child(header)
        .child(content)
}

fn activity_groups(
    groups: &[ActivityGroup],
    actions: &ConversationActionProps,
    tokens: DesignTokens,
) -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_5()
        .children(groups.iter().enumerate().map(|(group_index, group)| {
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(tokens.text_muted)
                        .child(group.date_label.clone()),
                )
                .child(
                    panel(tokens).children(group.rows.iter().enumerate().map(|(index, row)| {
                        let mut item = action_target(
                            div()
                                .w_full()
                                .min_w(px(0.0))
                                .px_5()
                                .py_3()
                                .flex()
                                .items_start()
                                .gap_6()
                                .cursor_pointer()
                                .hover(|style| style.bg(tokens.accent))
                                .child(
                                    div()
                                        .w(px(80.0))
                                        .text_sm()
                                        .text_color(tokens.text_muted)
                                        .child(row.time_label.clone()),
                                )
                                .child(
                                    div()
                                        .min_w(px(0.0))
                                        .max_w(px(ROW_COPY_MAX_WIDTH))
                                        .line_clamp(3)
                                        .text_sm()
                                        .text_color(tokens.text)
                                        .child(row.content.clone()),
                                ),
                            format!("conversation-activity-{group_index}-{index}"),
                            actions,
                            ConversationAction::CopyActivity {
                                group_index,
                                row_index: index,
                            },
                        );
                        if index + 1 < group.rows.len() {
                            item = item.border_b_1().border_color(tokens.border);
                        }
                        item
                    })),
                )
        }))
}

pub fn conversation_view_with_actions(
    props: &ConversationProps,
    actions: &ConversationActionProps,
    tokens: DesignTokens,
) -> Div {
    let header = div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .child(page_header(
            "Conversation History",
            Some("View your conversation with ListenOS including all commands and responses"),
            tokens,
        ))
        .child(
            div()
                .flex()
                .gap_2()
                .child(action_target(
                    primary_button("New Session", tokens),
                    "conversation-new-session",
                    actions,
                    ConversationAction::NewSession,
                ))
                .child(action_target(
                    secondary_button("Clear", tokens),
                    "conversation-clear",
                    actions,
                    ConversationAction::Clear,
                )),
        );

    let content = if props.loading {
        loading_panel("Loading conversation", tokens)
    } else if props.groups.is_empty() {
        empty_panel(
            "No conversation history",
            "Start speaking with ListenOS to see your conversation here.",
            tokens,
        )
    } else {
        activity_groups(&props.groups, actions, tokens)
    };

    route_frame(header, content)
}

fn command_card(
    index: usize,
    item: &CommandCardProps,
    template: bool,
    actions: &CommandsActionProps,
    tokens: DesignTokens,
) -> Div {
    let visible_actions = item.action_labels.iter().take(3);
    let overflow = item.action_labels.len().saturating_sub(3);

    let mut action_badges = div().flex().flex_wrap().gap_1().children(
        visible_actions.map(|label| status_pill(label.clone(), tokens.text_muted, tokens)),
    );
    if overflow > 0 {
        action_badges = action_badges.child(status_pill(
            format!("+{overflow} more"),
            tokens.text_muted,
            tokens,
        ));
    }

    let footer: AnyElement = if template {
        action_target(
            primary_button("Use This Template", tokens),
            ("command-use-template", index),
            actions,
            CommandsAction::UseTemplate { index },
        )
        .into_any_element()
    } else {
        div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .text_xs()
            .text_color(tokens.text_muted)
            .child(format!("Used {} times", item.use_count))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(action_target(
                        div().cursor_pointer().child("Edit"),
                        ("command-edit", index),
                        actions,
                        CommandsAction::Edit { index },
                    ))
                    .child(action_target(
                        div().cursor_pointer().child("Delete"),
                        ("command-delete", index),
                        actions,
                        CommandsAction::Delete { index },
                    )),
            )
            .into_any_element()
    };

    let enabled_control = (!template).then(|| {
        let mut toggle = ListenOsSwitch::new(
            ("command-enabled", index),
            item.enabled,
            format!("Toggle {} command", item.name),
            tokens,
        );
        if let Some(callback) = actions.on_action.clone() {
            toggle = toggle.on_change(move |enabled, _, _, _| {
                callback(CommandsAction::ToggleEnabled { index, enabled });
            });
        } else {
            toggle = toggle.disabled(true);
        }

        div()
            .flex()
            .items_center()
            .gap_2()
            .child(status_pill(
                if item.enabled { "Enabled" } else { "Disabled" },
                if item.enabled {
                    tokens.positive
                } else {
                    tokens.text_muted
                },
                tokens,
            ))
            .child(toggle)
    });

    panel(tokens)
        .p_4()
        .min_w(px(260.0))
        .flex_1()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .min_w(px(0.0))
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .max_w(px(CARD_COPY_MAX_WIDTH))
                                .truncate()
                                .text_color(tokens.text)
                                .child(item.name.clone()),
                        )
                        .child(
                            div()
                                .max_w(px(CARD_COPY_MAX_WIDTH))
                                .truncate()
                                .text_sm()
                                .text_color(tokens.primary)
                                .child(format!("\"{}\"", item.trigger_phrase)),
                        ),
                )
                .children(enabled_control),
        )
        .child(
            div()
                .max_w(px(CARD_COPY_MAX_WIDTH))
                .line_clamp(2)
                .text_sm()
                .text_color(tokens.text_muted)
                .child(item.description.clone()),
        )
        .child(action_badges)
        .child(footer)
}

fn command_editor(
    inputs: &CommandEditorInputs,
    actions: &CommandsActionProps,
    tokens: DesignTokens,
) -> Div {
    let action_rows = if inputs.action_rows.is_empty() {
        div()
            .text_sm()
            .text_color(tokens.text_muted)
            .child("No actions yet. Add one below.")
    } else {
        div().w_full().flex().flex_col().gap_2().children(
            inputs.action_rows.iter().enumerate().map(|(index, row)| {
                panel(tokens)
                    .p_3()
                    .flex()
                    .items_end()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_xs().text_color(tokens.text_muted).child("Type"))
                            .child(ListenOsInput::new(&row.action_type, tokens).width(px(150.0))),
                    )
                    .child(
                        div()
                            .min_w(px(0.0))
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(tokens.text_muted)
                                    .child("Payload JSON"),
                            )
                            .child(ListenOsInput::new(&row.payload, tokens).width(px(260.0))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(tokens.text_muted)
                                    .child("Delay ms"),
                            )
                            .child(ListenOsInput::new(&row.delay_ms, tokens).width(px(90.0))),
                    )
                    .child(action_target(
                        secondary_button("Remove", tokens),
                        ("command-action-remove", index),
                        actions,
                        CommandsAction::RemoveEditorAction { index },
                    ))
            }),
        )
    };
    let action_types = [
        ("open_app", "Open App"),
        ("open_url", "Open URL"),
        ("web_search", "Web Search"),
        ("type_text", "Type Text"),
        ("volume_control", "Volume Control"),
        ("spotify_control", "Spotify Control"),
        ("discord_control", "Discord Control"),
        ("system_control", "System Control"),
    ];
    let add_actions =
        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(
                action_types
                    .into_iter()
                    .enumerate()
                    .map(|(index, (action_type, label))| {
                        action_target(
                            secondary_button(format!("Add {label}"), tokens),
                            ("command-action-add", index),
                            actions,
                            CommandsAction::AddEditorAction {
                                action_type: action_type.into(),
                            },
                        )
                    }),
            );

    panel(tokens)
        .p_5()
        .flex()
        .flex_col()
        .gap_4()
        .child(
            div()
                .text_lg()
                .text_color(tokens.text)
                .child(if inputs.editing {
                    "Edit Command"
                } else {
                    "New Command"
                }),
        )
        .child(editor_field(
            "Name",
            "A short label for this command.",
            ListenOsInput::new(&inputs.name, tokens)
                .width(px(EDITOR_FIELD_WIDTH))
                .into_any_element(),
            tokens,
        ))
        .child(editor_field(
            "Trigger Phrase",
            "Say this phrase to trigger the command.",
            ListenOsInput::new(&inputs.trigger, tokens)
                .width(px(EDITOR_FIELD_WIDTH))
                .into_any_element(),
            tokens,
        ))
        .child(editor_field(
            "Description",
            "Describe what the command does.",
            ListenOsTextarea::new(&inputs.description, tokens)
                .width(px(EDITOR_FIELD_WIDTH))
                .min_height(px(72.0))
                .into_any_element(),
            tokens,
        ))
        .child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_sm().text_color(tokens.text).child("Actions"))
                .child(action_rows)
                .child(add_actions),
        )
        .child(
            div()
                .flex()
                .gap_2()
                .child(action_target(
                    secondary_button("Cancel", tokens),
                    "commands-editor-cancel",
                    actions,
                    CommandsAction::CancelEditor,
                ))
                .child(action_target(
                    primary_button("Save Command", tokens),
                    "commands-editor-save",
                    actions,
                    CommandsAction::SaveEditor,
                )),
        )
}

pub fn commands_view_with_actions(
    props: &CommandsProps,
    actions: &CommandsActionProps,
    editor: Option<&CommandEditorInputs>,
    tokens: DesignTokens,
) -> Div {
    let header = div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .child(page_header(
            "Custom Commands",
            Some("Create voice-triggered command sequences for common tasks"),
            tokens,
        ))
        .child(
            div()
                .flex()
                .gap_2()
                .child(action_target(
                    secondary_button("Import", tokens),
                    "commands-import",
                    actions,
                    CommandsAction::Import,
                ))
                .child(action_target(
                    secondary_button("Export", tokens),
                    "commands-export",
                    actions,
                    CommandsAction::Export,
                ))
                .child(action_target(
                    primary_button("New Command", tokens),
                    "commands-new",
                    actions,
                    CommandsAction::NewCommand,
                )),
        );

    let tabs = div()
        .w_full()
        .flex()
        .border_b_1()
        .border_color(tokens.border)
        .child(action_target(
            tab(
                format!("My Commands ({})", props.commands.len()),
                props.active_tab == CommandsTab::Commands,
                tokens,
            ),
            "commands-tab-custom",
            actions,
            CommandsAction::SelectTab(CommandsTab::Commands),
        ))
        .child(action_target(
            tab(
                format!("Templates ({})", props.templates.len()),
                props.active_tab == CommandsTab::Templates,
                tokens,
            ),
            "commands-tab-templates",
            actions,
            CommandsAction::SelectTab(CommandsTab::Templates),
        ));

    let items = match props.active_tab {
        CommandsTab::Commands => &props.commands,
        CommandsTab::Templates => &props.templates,
    };
    let template = props.active_tab == CommandsTab::Templates;
    let cards = if props.loading {
        loading_panel("Loading commands", tokens)
    } else if items.is_empty() {
        empty_panel(
            if template {
                "No templates"
            } else {
                "No custom commands yet"
            },
            if template {
                "Command templates will appear here when available."
            } else {
                "Create your own voice-triggered commands or use a template."
            },
            tokens,
        )
    } else {
        div().w_full().flex().flex_wrap().gap_4().children(
            items
                .iter()
                .enumerate()
                .map(|(index, item)| command_card(index, item, template, actions, tokens)),
        )
    };

    let mut content = div().w_full().flex().flex_col().gap_4().child(tabs);
    if let Some(editor) = editor {
        content = content.child(command_editor(editor, actions, tokens));
    }
    content = content.child(cards);

    route_frame(header, content)
}

fn clipboard_quick_actions(actions: &ClipboardActionProps, tokens: DesignTokens) -> Div {
    let quick_actions = [
        ("•", "Bullet List", ClipboardQuickAction::BulletList),
        ("1.", "Numbered List", ClipboardQuickAction::NumberedList),
        ("✦", "Clean Up Text", ClipboardQuickAction::CleanUpText),
        ("AA", "UPPERCASE", ClipboardQuickAction::Uppercase),
        ("aa", "lowercase", ClipboardQuickAction::Lowercase),
        ("Aa", "Title Case", ClipboardQuickAction::TitleCase),
    ];

    panel(tokens)
        .p_4()
        .min_w(px(230.0))
        .flex()
        .flex_col()
        .gap_3()
        .child(div().text_color(tokens.text).child("Quick Actions"))
        .children(quick_actions.into_iter().enumerate().map(|(index, (icon, label, action))| {
            action_target(
                div()
                .w_full()
                .px_2()
                .py_2()
                .rounded_lg()
                .flex()
                .items_center()
                .gap_3()
                .hover(|style| style.bg(tokens.accent))
                .child(
                    div()
                        .w(px(34.0))
                        .text_xs()
                        .text_color(tokens.primary)
                        .child(icon),
                )
                .child(div().text_sm().text_color(tokens.text).child(label)),
                ("clipboard-quick-action", index),
                actions,
                ClipboardAction::QuickAction(action),
            )
        }))
        .child(
            div()
                .pt_3()
                .border_t_1()
                .border_color(tokens.border)
                .max_w(px(280.0))
                .line_clamp(4)
                .text_xs()
                .text_color(tokens.text_muted)
                .child("Try saying: “Format my clipboard as a bullet list” or “Translate clipboard to Spanish”."),
        )
}

fn clipboard_current(
    props: &ClipboardProps,
    actions: &ClipboardActionProps,
    current_input: Option<&Entity<TextareaState>>,
    tokens: DesignTokens,
) -> Div {
    let words = props.current_content.split_whitespace().count();
    let chars = props.current_content.chars().count();
    let content = if props.loading {
        "Loading clipboard".into()
    } else if props.current_content.is_empty() {
        "Clipboard is empty. Copy some text to see it here.".into()
    } else {
        props.current_content.clone()
    };

    div()
        .w_full()
        .flex()
        .flex_wrap()
        .gap_6()
        .child(
            panel(tokens)
                .p_4()
                .min_w(px(380.0))
                .flex_1()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(div().text_color(tokens.text).child("Clipboard Content"))
                        .child(action_target(
                            secondary_button("Refresh", tokens),
                            "clipboard-refresh",
                            actions,
                            ClipboardAction::Refresh,
                        )),
                )
                .child(if let Some(input) = current_input {
                    ListenOsTextarea::new(input, tokens)
                        .width(px(640.0))
                        .min_height(px(192.0))
                        .into_any_element()
                } else {
                    div()
                        .h(px(192.0))
                        .w_full()
                        .min_w(px(0.0))
                        .rounded_lg()
                        .border_1()
                        .border_color(tokens.muted_border)
                        .bg(tokens.muted)
                        .p_3()
                        .line_clamp(8)
                        .text_sm()
                        .text_color(if props.current_content.is_empty() {
                            tokens.text_muted
                        } else {
                            tokens.text
                        })
                        .child(content)
                        .into_any_element()
                })
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_xs()
                        .text_color(tokens.text_muted)
                        .child(format!("{words} words, {chars} characters"))
                        .child(action_target(
                            div().cursor_pointer().child("Update Clipboard"),
                            "clipboard-update-current",
                            actions,
                            ClipboardAction::UpdateCurrent,
                        )),
                ),
        )
        .child(clipboard_quick_actions(actions, tokens))
}

fn clipboard_history(
    props: &ClipboardProps,
    actions: &ClipboardActionProps,
    tokens: DesignTokens,
) -> Div {
    if props.loading {
        return loading_panel("Loading clipboard history", tokens);
    }
    if props.history.is_empty() {
        return empty_panel(
            "No clipboard history",
            "Your clipboard history will appear here.",
            tokens,
        );
    }

    panel(tokens).children(props.history.iter().enumerate().map(|(index, item)| {
        let mut row = div()
            .w_full()
            .min_w(px(0.0))
            .p_4()
            .flex()
            .items_start()
            .gap_4()
            .child(status_pill(item.kind.short_label(), tokens.primary, tokens))
            .child(
                div()
                    .min_w(px(0.0))
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .max_w(px(ROW_COPY_MAX_WIDTH))
                            .truncate()
                            .text_sm()
                            .text_color(tokens.text)
                            .child(item.content.clone()),
                    )
                    .child(div().text_xs().text_color(tokens.text_muted).child(format!(
                        "{} words · {} chars · {}",
                        item.word_count, item.char_count, item.timestamp_label
                    ))),
            )
            .child(action_target(
                secondary_button("Copy", tokens),
                ("clipboard-copy-history", index),
                actions,
                ClipboardAction::CopyHistory { index },
            ));
        if index + 1 < props.history.len() {
            row = row.border_b_1().border_color(tokens.border);
        }
        row
    }))
}

pub fn clipboard_view_with_actions(
    props: &ClipboardProps,
    actions: &ClipboardActionProps,
    current_input: Option<&Entity<TextareaState>>,
    tokens: DesignTokens,
) -> Div {
    let tabs = div()
        .w_full()
        .flex()
        .border_b_1()
        .border_color(tokens.border)
        .child(action_target(
            tab(
                "Current Clipboard",
                props.active_tab == ClipboardTab::Current,
                tokens,
            ),
            "clipboard-tab-current",
            actions,
            ClipboardAction::SelectTab(ClipboardTab::Current),
        ))
        .child(action_target(
            tab(
                format!("History ({})", props.history.len()),
                props.active_tab == ClipboardTab::History,
                tokens,
            ),
            "clipboard-tab-history",
            actions,
            ClipboardAction::SelectTab(ClipboardTab::History),
        ));
    let body = match props.active_tab {
        ClipboardTab::Current => clipboard_current(props, actions, current_input, tokens),
        ClipboardTab::History => clipboard_history(props, actions, tokens),
    };

    route_frame(
        page_header(
            "Clipboard Tools",
            Some("Format, transform, and manage your clipboard content"),
            tokens,
        ),
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .child(tabs)
            .child(body),
    )
}

fn integration_card(
    index: usize,
    item: &IntegrationCardProps,
    actions: &IntegrationsActionProps,
    tokens: DesignTokens,
) -> Div {
    let mut enabled_switch = ListenOsSwitch::new(
        ("integration-enabled", index),
        item.enabled,
        format!("Toggle {} integration", item.name),
        tokens,
    )
    .disabled(!item.available);
    if item.available {
        if let Some(callback) = actions.on_action.clone() {
            enabled_switch = enabled_switch.on_change(move |enabled, _, _, _| {
                callback(IntegrationsAction::ToggleEnabled { index, enabled });
            });
        } else {
            enabled_switch = enabled_switch.disabled(true);
        }
    }

    let mut card = panel(tokens).child(
        div()
            .w_full()
            .p_4()
            .flex()
            .items_center()
            .gap_4()
            .child(
                div()
                    .size(px(56.0))
                    .rounded_lg()
                    .bg(tokens.muted)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_lg()
                    .text_color(if item.available {
                        tokens.primary
                    } else {
                        tokens.text_muted
                    })
                    .child(
                        item.name
                            .chars()
                            .next()
                            .unwrap_or('•')
                            .to_uppercase()
                            .to_string(),
                    ),
            )
            .child(
                div()
                    .min_w(px(0.0))
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .max_w(px(320.0))
                                    .truncate()
                                    .text_color(tokens.text)
                                    .child(item.name.clone()),
                            )
                            .children(
                                (!item.available)
                                    .then(|| status_pill("Not Installed", tokens.warning, tokens)),
                            ),
                    )
                    .child(
                        div()
                            .max_w(px(560.0))
                            .line_clamp(2)
                            .text_sm()
                            .text_color(tokens.text_muted)
                            .child(item.description.clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(tokens.text_muted)
                            .child(format!("{} commands", item.actions.len())),
                    )
                    .child(status_pill(
                        if item.enabled { "Enabled" } else { "Disabled" },
                        if item.enabled && item.available {
                            tokens.positive
                        } else {
                            tokens.text_muted
                        },
                        tokens,
                    ))
                    .child(enabled_switch)
                    .child(action_target(
                        secondary_button(if item.expanded { "Hide" } else { "Commands" }, tokens),
                        ("integration-expand", index),
                        actions,
                        IntegrationsAction::ToggleExpanded {
                            index,
                            expanded: !item.expanded,
                        },
                    )),
            ),
    );

    if item.expanded {
        card = card.child(
            div()
                .w_full()
                .p_4()
                .border_t_1()
                .border_color(tokens.muted_border)
                .bg(tokens.muted)
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .text_color(tokens.text)
                        .child("Available Voice Commands"),
                )
                .child(div().w_full().flex().flex_wrap().gap_3().children(
                    item.actions.iter().map(|action| {
                        panel(tokens)
                            .p_3()
                            .min_w(px(260.0))
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .max_w(px(CARD_COPY_MAX_WIDTH))
                                    .truncate()
                                    .text_color(tokens.text)
                                    .child(action.name.clone()),
                            )
                            .child(
                                div()
                                    .max_w(px(CARD_COPY_MAX_WIDTH))
                                    .line_clamp(2)
                                    .text_xs()
                                    .text_color(tokens.text_muted)
                                    .child(action.description.clone()),
                            )
                            .child(div().flex().flex_wrap().gap_1().children(
                                action.example_phrases.iter().take(2).map(|phrase| {
                                    status_pill(format!("\"{phrase}\""), tokens.primary, tokens)
                                }),
                            ))
                    }),
                )),
        );
    }

    card
}

pub fn integrations_view_with_actions(
    props: &IntegrationsProps,
    actions: &IntegrationsActionProps,
    tokens: DesignTokens,
) -> Div {
    let list = if props.loading {
        loading_panel("Loading integrations", tokens)
    } else if props.integrations.is_empty() {
        empty_panel(
            "No integrations available",
            "Supported native integrations will appear here.",
            tokens,
        )
    } else {
        div().w_full().flex().flex_col().gap_4().children(
            props
                .integrations
                .iter()
                .enumerate()
                .map(|(index, item)| integration_card(index, item, actions, tokens)),
        )
    };

    let tips = panel(tokens)
        .p_4()
        .flex()
        .flex_col()
        .gap_2()
        .child(div().text_color(tokens.text).child("Pro Tips"))
        .children(
            [
                "Say “pause spotify” or “skip this song” to control music",
                "Say “mute discord” or “deafen discord” during calls",
                "Say “lock my computer” or “take a screenshot” for system control",
                "Integrations work even when the app is in the background",
            ]
            .into_iter()
            .map(|tip| {
                div()
                    .max_w(px(760.0))
                    .line_clamp(2)
                    .text_sm()
                    .text_color(tokens.text_muted)
                    .child(format!("• {tip}"))
            }),
        );

    route_frame(
        page_header(
            "App Integrations",
            Some("Control your favorite apps with voice commands"),
            tokens,
        ),
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .child(list)
            .child(tips),
    )
}

fn collection_tabs<A>(
    active: CollectionTab,
    actions: &ActionProps<A>,
    id_prefix: &'static str,
    make_action: fn(CollectionTab) -> A,
    tokens: DesignTokens,
) -> Div
where
    A: Clone + 'static,
{
    div()
        .flex()
        .border_b_1()
        .border_color(tokens.border)
        .children(
            CollectionTab::ALL
                .into_iter()
                .enumerate()
                .map(|(index, item)| {
                    action_target(
                        tab(item.label(), item == active, tokens),
                        (id_prefix, index),
                        actions,
                        make_action(item),
                    )
                }),
        )
}

impl CollectionTab {
    const ALL: [Self; 3] = [Self::All, Self::Personal, Self::Shared];
}

fn dictionary_intro(actions: &DictionaryActionProps, tokens: DesignTokens) -> Div {
    panel(tokens)
        .bg(tokens.muted)
        .p_6()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .text_xl()
                .text_color(tokens.text)
                .child("ListenOS speaks the way you speak."),
        )
        .child(
            div()
                .max_w(px(760.0))
                .line_clamp(4)
                .text_sm()
                .text_color(tokens.text_muted)
                .child("ListenOS learns your unique words and names automatically or manually. Add personal terms, company jargon, client names, or industry-specific lingo."),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .children(["Q3 Roadmap", "Whispr → Wispr", "SF MOMA", "Figma Jam", "Company name"].map(|example| {
                    status_pill(example, tokens.text, tokens)
                })),
        )
        .child(action_target(
            primary_button("Add new word", tokens),
            "dictionary-intro-add",
            actions,
            DictionaryAction::AddNew,
        ))
}

fn dictionary_editor(
    inputs: &DictionaryEditorInputs,
    actions: &DictionaryActionProps,
    tokens: DesignTokens,
) -> Div {
    panel(tokens)
        .p_5()
        .flex()
        .flex_col()
        .gap_4()
        .child(
            div()
                .text_lg()
                .text_color(tokens.text)
                .child(if inputs.editing {
                    "Edit Word"
                } else {
                    "Add Word"
                }),
        )
        .child(editor_field(
            "Word or phrase",
            "The spelling you want ListenOS to recognize.",
            ListenOsInput::new(&inputs.word, tokens)
                .width(px(EDITOR_FIELD_WIDTH))
                .into_any_element(),
            tokens,
        ))
        .child(editor_field(
            "Phonetic (optional)",
            "How the word sounds, to help with recognition.",
            ListenOsInput::new(&inputs.phonetic, tokens)
                .width(px(EDITOR_FIELD_WIDTH))
                .into_any_element(),
            tokens,
        ))
        .child(
            div()
                .flex()
                .gap_2()
                .child(action_target(
                    secondary_button("Cancel", tokens),
                    "dictionary-editor-cancel",
                    actions,
                    DictionaryAction::CancelEditor,
                ))
                .child(action_target(
                    primary_button(
                        if inputs.editing {
                            "Save Changes"
                        } else {
                            "Add Word"
                        },
                        tokens,
                    ),
                    "dictionary-editor-save",
                    actions,
                    DictionaryAction::SaveEditor,
                )),
        )
}

pub fn dictionary_view_with_actions(
    props: &DictionaryProps,
    actions: &DictionaryActionProps,
    search_input: Option<&Entity<InputState>>,
    editor: Option<&DictionaryEditorInputs>,
    tokens: DesignTokens,
) -> Div {
    let header = div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .child(page_header("Dictionary", None::<&'static str>, tokens))
        .child(action_target(
            primary_button("Add new", tokens),
            "dictionary-add",
            actions,
            DictionaryAction::AddNew,
        ));
    let search = search_input
        .map(|input| {
            ListenOsInput::new(input, tokens)
                .width(px(200.0))
                .into_any_element()
        })
        .unwrap_or_else(|| {
            div()
                .w(px(200.0))
                .px_3()
                .py_2()
                .rounded_lg()
                .border_1()
                .border_color(tokens.muted_border)
                .bg(tokens.muted)
                .text_sm()
                .text_color(tokens.text_muted)
                .child("Search...")
                .into_any_element()
        });
    let search = if props.search_query.is_empty() {
        div().child(search)
    } else {
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(search)
            .child(action_target(
                secondary_button("Clear", tokens),
                "dictionary-search-clear",
                actions,
                DictionaryAction::SearchChanged("".into()),
            ))
    };
    let controls = div()
        .w_full()
        .flex()
        .items_end()
        .justify_between()
        .gap_4()
        .child(collection_tabs(
            props.active_tab,
            actions,
            "dictionary-tab",
            DictionaryAction::SelectTab,
            tokens,
        ))
        .child(search);

    let list = if props.loading {
        loading_panel("Loading dictionary", tokens)
    } else if props.entries.is_empty() {
        empty_panel(
            "No custom words yet",
            if props.search_query.is_empty() {
                "Add your first word to improve local recognition."
            } else {
                "No words match your search."
            },
            tokens,
        )
    } else {
        panel(tokens).children(props.entries.iter().enumerate().map(|(index, entry)| {
            let mut row = div()
                .w_full()
                .min_w(px(0.0))
                .px_6()
                .py_4()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .child(
                    div()
                        .min_w(px(0.0))
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .max_w(px(420.0))
                                .truncate()
                                .text_sm()
                                .text_color(tokens.text)
                                .child(entry.word.clone()),
                        )
                        .children(entry.phonetic.as_ref().map(|phonetic| {
                            div()
                                .max_w(px(220.0))
                                .truncate()
                                .text_xs()
                                .text_color(tokens.text_muted)
                                .child(format!("/{phonetic}/"))
                        }))
                        .children(
                            entry
                                .auto_learned
                                .then(|| status_pill("Auto", tokens.warning, tokens)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(div().text_xs().text_color(tokens.text_muted).child(
                            if entry.use_count > 0 {
                                format!("Used {}x", entry.use_count)
                            } else {
                                String::new()
                            },
                        ))
                        .child(action_target(
                            div().text_xs().text_color(tokens.text_muted).child("Edit"),
                            ("dictionary-edit", index),
                            actions,
                            DictionaryAction::Edit { index },
                        ))
                        .child(action_target(
                            div()
                                .text_xs()
                                .text_color(tokens.text_muted)
                                .child("Delete"),
                            ("dictionary-delete", index),
                            actions,
                            DictionaryAction::Delete { index },
                        )),
                );
            if index + 1 < props.entries.len() {
                row = row.border_b_1().border_color(tokens.border);
            }
            row
        }))
    };

    let mut content = div().w_full().flex().flex_col().gap_4().child(controls);
    if let Some(editor) = editor {
        content = content.child(dictionary_editor(editor, actions, tokens));
    }
    if props.show_intro && props.entries.is_empty() && props.search_query.is_empty() {
        content = content.child(dictionary_intro(actions, tokens));
    }
    content = content.child(list);
    route_frame(header, content)
}

fn snippets_intro(actions: &SnippetsActionProps, tokens: DesignTokens) -> Div {
    let examples = [
        ("Linkedin", "linkedin.com/in/you"),
        (
            "intro email",
            "Hey, would love to find some time to chat later...",
        ),
        ("my calendly link", "calendly.com/you/invite-name"),
    ];

    panel(tokens)
        .bg(tokens.muted)
        .p_6()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .text_xl()
                .text_color(tokens.text)
                .child("The stuff you shouldn't have to re-type."),
        )
        .child(
            div()
                .max_w(px(760.0))
                .line_clamp(4)
                .text_sm()
                .text_color(tokens.text_muted)
                .child("Save shortcuts to speak the things you type all the time: emails, links, addresses, bios, and anything else you reuse."),
        )
        .children(examples.into_iter().map(|(trigger, expansion)| {
            div()
                .w_full()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .gap_3()
                .child(status_pill(trigger, tokens.text, tokens))
                .child(div().text_color(tokens.text_muted).child("→"))
                .child(
                    div()
                        .max_w(px(520.0))
                        .truncate()
                        .text_sm()
                        .text_color(tokens.text)
                        .child(expansion),
                )
        }))
        .child(action_target(
            primary_button("Add new snippet", tokens),
            "snippets-intro-add",
            actions,
            SnippetsAction::AddNew,
        ))
}

fn snippets_editor(
    inputs: &SnippetEditorInputs,
    actions: &SnippetsActionProps,
    tokens: DesignTokens,
) -> Div {
    panel(tokens)
        .p_5()
        .flex()
        .flex_col()
        .gap_4()
        .child(
            div()
                .text_lg()
                .text_color(tokens.text)
                .child(if inputs.editing {
                    "Edit Snippet"
                } else {
                    "New Snippet"
                }),
        )
        .child(editor_field(
            "Trigger phrase",
            "Say this phrase to trigger the snippet.",
            ListenOsInput::new(&inputs.trigger, tokens)
                .width(px(EDITOR_FIELD_WIDTH))
                .into_any_element(),
            tokens,
        ))
        .child(editor_field(
            "Expansion",
            "This text will be typed when you say the trigger.",
            ListenOsTextarea::new(&inputs.expansion, tokens)
                .width(px(EDITOR_FIELD_WIDTH))
                .min_height(px(110.0))
                .into_any_element(),
            tokens,
        ))
        .child(
            div()
                .flex()
                .gap_2()
                .child(action_target(
                    secondary_button("Cancel", tokens),
                    "snippets-editor-cancel",
                    actions,
                    SnippetsAction::CancelEditor,
                ))
                .child(action_target(
                    primary_button(
                        if inputs.editing {
                            "Save Changes"
                        } else {
                            "Create Snippet"
                        },
                        tokens,
                    ),
                    "snippets-editor-save",
                    actions,
                    SnippetsAction::SaveEditor,
                )),
        )
}

pub fn snippets_view_with_actions(
    props: &SnippetsProps,
    actions: &SnippetsActionProps,
    search_input: Option<&Entity<InputState>>,
    editor: Option<&SnippetEditorInputs>,
    tokens: DesignTokens,
) -> Div {
    let header = div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .child(page_header("Snippets", None::<&'static str>, tokens))
        .child(action_target(
            primary_button("Add new", tokens),
            "snippets-add",
            actions,
            SnippetsAction::AddNew,
        ));
    let search = search_input
        .map(|input| {
            ListenOsInput::new(input, tokens)
                .width(px(200.0))
                .into_any_element()
        })
        .unwrap_or_else(|| {
            div()
                .w(px(200.0))
                .px_3()
                .py_2()
                .rounded_lg()
                .border_1()
                .border_color(tokens.muted_border)
                .bg(tokens.muted)
                .text_sm()
                .text_color(tokens.text_muted)
                .child("Search...")
                .into_any_element()
        });
    let search = if props.search_query.is_empty() {
        div().child(search)
    } else {
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(search)
            .child(action_target(
                secondary_button("Clear", tokens),
                "snippets-search-clear",
                actions,
                SnippetsAction::SearchChanged("".into()),
            ))
    };
    let controls = div()
        .w_full()
        .flex()
        .items_end()
        .justify_between()
        .gap_4()
        .child(collection_tabs(
            props.active_tab,
            actions,
            "snippets-tab",
            SnippetsAction::SelectTab,
            tokens,
        ))
        .child(search);
    let list = if props.loading {
        loading_panel("Loading snippets", tokens)
    } else if props.entries.is_empty() {
        empty_panel(
            "No snippets yet",
            if props.search_query.is_empty() {
                "Create your first snippet to expand frequently used text by voice."
            } else {
                "No snippets match your search."
            },
            tokens,
        )
    } else {
        panel(tokens).children(props.entries.iter().enumerate().map(|(index, entry)| {
            let mut row = div()
                .w_full()
                .min_w(px(0.0))
                .px_6()
                .py_4()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .max_w(px(220.0))
                        .truncate()
                        .text_sm()
                        .text_color(tokens.text)
                        .child(entry.trigger.clone()),
                )
                .child(div().text_color(tokens.text_muted).child("→"))
                .child(
                    div()
                        .min_w(px(0.0))
                        .max_w(px(ROW_COPY_MAX_WIDTH))
                        .flex_1()
                        .truncate()
                        .text_sm()
                        .text_color(tokens.text_muted)
                        .child(entry.expansion.clone()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(div().text_xs().text_color(tokens.text_muted).child(
                            if entry.use_count > 0 {
                                format!("Used {}x", entry.use_count)
                            } else {
                                String::new()
                            },
                        ))
                        .child(action_target(
                            div().text_xs().text_color(tokens.text_muted).child("Edit"),
                            ("snippets-edit", index),
                            actions,
                            SnippetsAction::Edit { index },
                        ))
                        .child(action_target(
                            div()
                                .text_xs()
                                .text_color(tokens.text_muted)
                                .child("Delete"),
                            ("snippets-delete", index),
                            actions,
                            SnippetsAction::Delete { index },
                        )),
                );
            if index + 1 < props.entries.len() {
                row = row.border_b_1().border_color(tokens.border);
            }
            row
        }))
    };

    let mut content = div().w_full().flex().flex_col().gap_4().child(controls);
    if let Some(editor) = editor {
        content = content.child(snippets_editor(editor, actions, tokens));
    }
    if props.show_intro && props.entries.is_empty() && props.search_query.is_empty() {
        content = content.child(snippets_intro(actions, tokens));
    }
    content = content.child(list);
    route_frame(header, content)
}

fn tone_card(
    index: usize,
    tone: StyleTone,
    selected: bool,
    actions: &StyleActionProps,
    tokens: DesignTokens,
) -> Stateful<Div> {
    action_target(
        panel(tokens)
            .min_w(px(240.0))
            .flex_1()
            .p_5()
            .border_color(if selected {
                tokens.primary
            } else {
                tokens.border
            })
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_2xl().text_color(tokens.text).child(tone.label()))
            .child(
                div()
                    .text_sm()
                    .text_color(tokens.text_muted)
                    .child(tone.description()),
            )
            .child(
                div()
                    .max_w(px(CARD_COPY_MAX_WIDTH))
                    .line_clamp(4)
                    .rounded_lg()
                    .bg(tokens.muted)
                    .p_3()
                    .text_sm()
                    .text_color(tokens.text_muted)
                    .child(tone.example()),
            )
            .children(selected.then(|| status_pill("Selected", tokens.primary, tokens))),
        ("style-tone", index),
        actions,
        StyleAction::SelectTone(tone),
    )
}

pub fn style_view_with_actions(
    props: StyleProps,
    actions: &StyleActionProps,
    tokens: DesignTokens,
) -> Div {
    let tabs = div()
        .w_full()
        .flex()
        .border_b_1()
        .border_color(tokens.border)
        .children(
            StyleContext::ALL
                .into_iter()
                .enumerate()
                .map(|(index, context)| {
                    action_target(
                        tab(context.label(), context == props.context, tokens),
                        ("style-context", index),
                        actions,
                        StyleAction::SelectContext(context),
                    )
                }),
        );
    let messenger_note = div()
        .w_full()
        .rounded_lg()
        .bg(tokens.muted)
        .p_4()
        .flex()
        .items_center()
        .gap_4()
        .child(
            div()
                .flex()
                .child(status_pill("M", tokens.primary, tokens))
                .child(status_pill("W", tokens.primary, tokens))
                .child(status_pill("T", tokens.primary, tokens)),
        )
        .child(
            div()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .text_color(tokens.text)
                        .child("This style applies in personal messengers"),
                )
                .child(
                    div()
                        .max_w(px(620.0))
                        .line_clamp(2)
                        .text_sm()
                        .text_color(tokens.text_muted)
                        .child(
                            "Available on desktop in English. iOS and more languages coming soon",
                        ),
                ),
        );

    route_frame(
        page_header("Style", None::<&'static str>, tokens),
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .child(tabs)
            .child(messenger_note)
            .child(div().w_full().flex().flex_wrap().gap_4().children(
                StyleTone::ALL.into_iter().enumerate().map(|(index, tone)| {
                    tone_card(index, tone, tone == props.tone, actions, tokens)
                }),
            )),
    )
}
