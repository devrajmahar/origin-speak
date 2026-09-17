use super::common::{
    action_target, empty_panel, format_number, loading_panel, panel, primary_button,
};
use super::types::{
    ActivityGroup, DashboardAction, DashboardActionProps, DashboardProps, DashboardStats,
    FeatureTipProps, GreetingProps,
};
use crate::theme::DesignTokens;
use gpui::*;

const ACTIVITY_CONTENT_MAX_WIDTH: f32 = 760.0;
const FEATURE_COPY_MAX_WIDTH: f32 = 760.0;

fn stat_item(label: &'static str, value: String, tokens: DesignTokens) -> Div {
    div()
        .min_w(px(132.0))
        .px_4()
        .py_3()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(tokens.text_muted).child(label))
        .child(div().text_lg().text_color(tokens.text).child(value))
}

fn stats_strip(stats: DashboardStats, tokens: DesignTokens) -> Div {
    panel(tokens).flex().flex_wrap().children([
        stat_item(
            "CURRENT STREAK",
            format!(
                "{} day{}",
                stats.streak_days,
                if stats.streak_days == 1 { "" } else { "s" }
            ),
            tokens,
        ),
        stat_item("TOTAL WORDS", format_number(stats.total_words), tokens),
        stat_item("WORDS TODAY", format_number(stats.today_words), tokens),
    ])
}

fn greeting_card(props: &GreetingProps, tokens: DesignTokens) -> Div {
    div()
        .w_full()
        .pb_6()
        .border_b_1()
        .border_color(tokens.border)
        .flex()
        .flex_wrap()
        .items_end()
        .justify_between()
        .gap_5()
        .child(
            div()
                .min_w(px(260.0))
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .max_w(px(560.0))
                        .line_clamp(2)
                        .text_3xl()
                        .text_color(tokens.text)
                        .child(props.greeting.clone()),
                )
                .child(
                    div()
                        .max_w(px(560.0))
                        .line_clamp(2)
                        .text_sm()
                        .text_color(tokens.text_muted)
                        .child(format!(
                            "{} · Your voice workspace is ready.",
                            props.date_label
                        )),
                ),
        )
        .child(stats_strip(props.stats, tokens))
}

fn feature_tip(
    props: &FeatureTipProps,
    actions: &DashboardActionProps,
    tokens: DesignTokens,
) -> Div {
    let mut tip = panel(tokens)
        .p_6()
        .flex()
        .flex_wrap()
        .items_end()
        .justify_between()
        .gap_4()
        .child(
            div()
                .min_w(px(0.0))
                .flex_1()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(tokens.text_muted)
                        .child("WORKFLOW TIP"),
                )
                .child(
                    div()
                        .max_w(px(FEATURE_COPY_MAX_WIDTH))
                        .line_clamp(2)
                        .text_xl()
                        .text_color(tokens.text)
                        .child(props.title.clone()),
                )
                .child(
                    div()
                        .max_w(px(FEATURE_COPY_MAX_WIDTH))
                        .line_clamp(4)
                        .text_sm()
                        .text_color(tokens.text_muted)
                        .child(props.description.clone()),
                ),
        );

    if let Some(label) = &props.action_label {
        tip = tip.child(action_target(
            primary_button(label.clone(), tokens),
            "dashboard-feature-tip",
            actions,
            DashboardAction::FeatureTip,
        ));
    }

    tip
}

fn activity_group(
    group_index: usize,
    group: &ActivityGroup,
    actions: &DashboardActionProps,
    tokens: DesignTokens,
) -> Div {
    let rows = group.rows.iter().enumerate().map(|(index, row)| {
        let mut item = action_target(
            div()
                .w_full()
                .min_w(px(0.0))
                .px_5()
                .py_3()
                .flex()
                .items_start()
                .gap_5()
                .cursor_pointer()
                .hover(|style| style.bg(tokens.accent))
                .child(
                    div()
                        .w(px(68.0))
                        .text_xs()
                        .text_color(tokens.text_muted)
                        .child(row.time_label.clone()),
                )
                .child(
                    div()
                        .min_w(px(0.0))
                        .max_w(px(ACTIVITY_CONTENT_MAX_WIDTH))
                        .line_clamp(2)
                        .text_sm()
                        .text_color(tokens.text)
                        .child(row.content.clone()),
                ),
            format!("dashboard-activity-{group_index}-{index}"),
            actions,
            DashboardAction::CopyActivity {
                group_index,
                row_index: index,
            },
        );

        if index + 1 < group.rows.len() {
            item = item.border_b_1().border_color(tokens.border);
        }
        item
    });

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
        .child(panel(tokens).children(rows))
}

fn recent_activity(
    props: &DashboardProps,
    actions: &DashboardActionProps,
    tokens: DesignTokens,
) -> Div {
    let content = if props.activity_loading {
        loading_panel("Loading activity", tokens)
    } else if props.recent_activity.is_empty() {
        empty_panel(
            "No activity yet",
            "Start dictating to populate this feed.",
            tokens,
        )
    } else {
        div().w_full().flex().flex_col().gap_5().children(
            props
                .recent_activity
                .iter()
                .enumerate()
                .map(|(index, group)| activity_group(index, group, actions, tokens)),
        )
    };

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .text_color(tokens.text_muted)
                        .child("RECENT ACTIVITY"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(tokens.text_muted)
                        .child("Click any row to copy dictation instantly."),
                ),
        )
        .child(content)
}

pub fn dashboard_view_with_actions(
    props: &DashboardProps,
    actions: &DashboardActionProps,
    tokens: DesignTokens,
) -> Div {
    div()
        .w_full()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap_6()
        .pb_6()
        .child(greeting_card(&props.greeting, tokens))
        .child(feature_tip(&props.feature_tip, actions, tokens))
        .child(recent_activity(props, actions, tokens))
}
