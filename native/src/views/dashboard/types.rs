use gpui::SharedString;
use std::sync::Arc;

pub type ActionCallback<A> = Arc<dyn Fn(A) + Send + Sync + 'static>;

pub struct ActionProps<A> {
    pub on_action: Option<ActionCallback<A>>,
}

impl<A> Clone for ActionProps<A> {
    fn clone(&self) -> Self {
        Self {
            on_action: self.on_action.clone(),
        }
    }
}

impl<A> Default for ActionProps<A> {
    fn default() -> Self {
        Self { on_action: None }
    }
}

impl<A> ActionProps<A> {
    pub fn new(on_action: impl Fn(A) + Send + Sync + 'static) -> Self {
        Self {
            on_action: Some(Arc::new(on_action)),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DashboardStats {
    pub streak_days: u32,
    pub total_words: u64,
    pub today_words: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GreetingProps {
    pub greeting: SharedString,
    pub date_label: SharedString,
    pub stats: DashboardStats,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FeatureTipProps {
    pub title: SharedString,
    pub description: SharedString,
    pub action_label: Option<SharedString>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActivityRow {
    pub time_label: SharedString,
    pub content: SharedString,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActivityGroup {
    pub date_label: SharedString,
    pub rows: Vec<ActivityRow>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DashboardProps {
    pub greeting: GreetingProps,
    pub feature_tip: FeatureTipProps,
    pub recent_activity: Vec<ActivityGroup>,
    pub activity_loading: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DashboardAction {
    FeatureTip,
    CopyActivity {
        group_index: usize,
        row_index: usize,
    },
}

pub type DashboardActionProps = ActionProps<DashboardAction>;

#[derive(Clone, Debug, PartialEq)]
pub struct ConversationProps {
    pub groups: Vec<ActivityGroup>,
    pub loading: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConversationAction {
    NewSession,
    Clear,
    CopyActivity {
        group_index: usize,
        row_index: usize,
    },
}

pub type ConversationActionProps = ActionProps<ConversationAction>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CommandsTab {
    #[default]
    Commands,
    Templates,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CommandCardProps {
    pub name: SharedString,
    pub trigger_phrase: SharedString,
    pub description: SharedString,
    pub action_labels: Vec<SharedString>,
    pub use_count: u32,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CommandsProps {
    pub active_tab: CommandsTab,
    pub commands: Vec<CommandCardProps>,
    pub templates: Vec<CommandCardProps>,
    pub loading: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandsAction {
    SelectTab(CommandsTab),
    Import,
    Export,
    NewCommand,
    SaveEditor,
    CancelEditor,
    AddEditorAction { action_type: SharedString },
    RemoveEditorAction { index: usize },
    ToggleEnabled { index: usize, enabled: bool },
    Edit { index: usize },
    Delete { index: usize },
    UseTemplate { index: usize },
}

pub type CommandsActionProps = ActionProps<CommandsAction>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClipboardTab {
    #[default]
    Current,
    History,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardContentKind {
    Text,
    Url,
    Email,
    Code,
    List,
}

impl ClipboardContentKind {
    pub fn short_label(self) -> &'static str {
        match self {
            Self::Text => "TXT",
            Self::Url => "URL",
            Self::Email => "MAIL",
            Self::Code => "CODE",
            Self::List => "LIST",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClipboardHistoryItem {
    pub content: SharedString,
    pub kind: ClipboardContentKind,
    pub word_count: usize,
    pub char_count: usize,
    pub timestamp_label: SharedString,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClipboardProps {
    pub active_tab: ClipboardTab,
    pub current_content: SharedString,
    pub history: Vec<ClipboardHistoryItem>,
    pub loading: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardQuickAction {
    BulletList,
    NumberedList,
    CleanUpText,
    Uppercase,
    Lowercase,
    TitleCase,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardAction {
    SelectTab(ClipboardTab),
    Refresh,
    UpdateCurrent,
    QuickAction(ClipboardQuickAction),
    CopyHistory { index: usize },
}

pub type ClipboardActionProps = ActionProps<ClipboardAction>;

#[derive(Clone, Debug, PartialEq)]
pub struct IntegrationActionProps {
    pub name: SharedString,
    pub description: SharedString,
    pub example_phrases: Vec<SharedString>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IntegrationCardProps {
    pub name: SharedString,
    pub description: SharedString,
    pub enabled: bool,
    pub available: bool,
    pub expanded: bool,
    pub actions: Vec<IntegrationActionProps>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IntegrationsProps {
    pub integrations: Vec<IntegrationCardProps>,
    pub loading: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntegrationsAction {
    ToggleEnabled { index: usize, enabled: bool },
    ToggleExpanded { index: usize, expanded: bool },
}

pub type IntegrationsActionProps = ActionProps<IntegrationsAction>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CollectionTab {
    #[default]
    All,
    Personal,
    Shared,
}

impl CollectionTab {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Personal => "Personal",
            Self::Shared => "Shared with team",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DictionaryEntryProps {
    pub word: SharedString,
    pub phonetic: Option<SharedString>,
    pub auto_learned: bool,
    pub use_count: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DictionaryProps {
    pub active_tab: CollectionTab,
    pub search_query: SharedString,
    pub entries: Vec<DictionaryEntryProps>,
    pub show_intro: bool,
    pub loading: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DictionaryAction {
    SelectTab(CollectionTab),
    SearchChanged(SharedString),
    AddNew,
    SaveEditor,
    CancelEditor,
    Edit { index: usize },
    Delete { index: usize },
}

pub type DictionaryActionProps = ActionProps<DictionaryAction>;

#[derive(Clone, Debug, PartialEq)]
pub struct SnippetEntryProps {
    pub trigger: SharedString,
    pub expansion: SharedString,
    pub use_count: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SnippetsProps {
    pub active_tab: CollectionTab,
    pub search_query: SharedString,
    pub entries: Vec<SnippetEntryProps>,
    pub show_intro: bool,
    pub loading: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnippetsAction {
    SelectTab(CollectionTab),
    SearchChanged(SharedString),
    AddNew,
    SaveEditor,
    CancelEditor,
    Edit { index: usize },
    Delete { index: usize },
}

pub type SnippetsActionProps = ActionProps<SnippetsAction>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StyleContext {
    #[default]
    Personal,
    Work,
    Email,
    Other,
}

impl StyleContext {
    pub const ALL: [Self; 4] = [Self::Personal, Self::Work, Self::Email, Self::Other];

    pub fn label(self) -> &'static str {
        match self {
            Self::Personal => "Personal messages",
            Self::Work => "Work messages",
            Self::Email => "Email",
            Self::Other => "Other",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StyleTone {
    #[default]
    Formal,
    Casual,
    VeryCasual,
}

impl StyleTone {
    pub const ALL: [Self; 3] = [Self::Formal, Self::Casual, Self::VeryCasual];

    pub fn label(self) -> &'static str {
        match self {
            Self::Formal => "Formal.",
            Self::Casual => "Casual.",
            Self::VeryCasual => "very casual",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Formal => "Caps + Punctuation",
            Self::Casual => "Caps + Less punctuation",
            Self::VeryCasual => "No Caps + Less punctuation",
        }
    }

    pub fn example(self) -> &'static str {
        match self {
            Self::Formal => {
                "Hey, are you free for lunch tomorrow? Let's do 12 if that works for you."
            }
            Self::Casual => {
                "Hey are you free for lunch tomorrow? Let's do 12 if that works for you"
            }
            Self::VeryCasual => {
                "hey are you free for lunch tomorrow? let's do 12 if that works for you"
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StyleProps {
    pub context: StyleContext,
    pub tone: StyleTone,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StyleAction {
    SelectContext(StyleContext),
    SelectTone(StyleTone),
}

pub type StyleActionProps = ActionProps<StyleAction>;
