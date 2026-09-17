mod common;
mod home;
mod routes;
mod types;

pub use home::dashboard_view_with_actions;
pub use routes::{
    CommandActionEditorInputs, CommandEditorInputs, DictionaryEditorInputs, SnippetEditorInputs,
    clipboard_view_with_actions, commands_view_with_actions, conversation_view_with_actions,
    dictionary_view_with_actions, integrations_view_with_actions, snippets_view_with_actions,
    style_view_with_actions,
};
pub use types::*;
