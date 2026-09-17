//! Conversation Memory System for ListenOS
//!
//! Manages conversation history, context, and persistent memory
//! to enable multi-turn dialogues and context-aware responses.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::voice::ActionType;

/// Maximum messages to keep in short-term memory for LLM context
const MAX_SHORT_TERM_MESSAGES: usize = 10;

/// Maximum facts to store per session
const MAX_FACTS_PER_SESSION: usize = 50;

/// Role in the conversation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    User,
    Assistant,
    System,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::User => write!(f, "user"),
            Role::Assistant => write!(f, "assistant"),
            Role::System => write!(f, "system"),
        }
    }
}

/// A single message in the conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub role: Role,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub action_taken: Option<String>,
    pub action_success: Option<bool>,
}

impl Message {
    pub fn user(content: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            role: Role::User,
            content,
            timestamp: Utc::now(),
            action_taken: None,
            action_success: None,
        }
    }

    pub fn assistant(content: String, action: Option<ActionType>, success: Option<bool>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            role: Role::Assistant,
            content,
            timestamp: Utc::now(),
            action_taken: action.map(|a| format!("{:?}", a)),
            action_success: success,
        }
    }

    pub fn system(content: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            role: Role::System,
            content,
            timestamp: Utc::now(),
            action_taken: None,
            action_success: None,
        }
    }
}

/// An extracted fact from conversation for long-term memory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    pub id: String,
    pub category: String, // e.g., "preference", "contact", "routine"
    pub key: String,      // e.g., "favorite_music"
    pub value: String,    // e.g., "lofi hip hop"
    pub source_message_id: String,
    pub created_at: DateTime<Utc>,
    pub last_used: DateTime<Utc>,
    pub use_count: u32,
}

/// Conversation memory state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMemory {
    pub session_id: String,
    pub messages: Vec<Message>,
    pub extracted_facts: Vec<Fact>,
    pub last_action: Option<String>,
    pub last_action_payload: Option<serde_json::Value>,
    pub started_at: DateTime<Utc>,
}

impl Default for ConversationMemory {
    fn default() -> Self {
        Self {
            session_id: uuid::Uuid::new_v4().to_string(),
            messages: Vec::new(),
            extracted_facts: Vec::new(),
            last_action: None,
            last_action_payload: None,
            started_at: Utc::now(),
        }
    }
}

impl ConversationMemory {
    /// Create a new session
    pub fn new_session() -> Self {
        Self::default()
    }

    /// Add a user message
    pub fn add_user_message(&mut self, content: String) {
        self.messages.push(Message::user(content));
        self.trim_messages();
    }

    /// Add an assistant response with optional action info
    pub fn add_assistant_message(
        &mut self,
        content: String,
        action: Option<ActionType>,
        success: Option<bool>,
        payload: Option<serde_json::Value>,
    ) {
        self.messages
            .push(Message::assistant(content, action, success));
        if let Some(a) = action {
            self.last_action = Some(format!("{:?}", a));
            self.last_action_payload = payload;
        }
        self.trim_messages();
    }

    /// Get recent messages for LLM context (respects MAX_SHORT_TERM_MESSAGES)
    pub fn get_context_messages(&self) -> Vec<&Message> {
        let skip = self.messages.len().saturating_sub(MAX_SHORT_TERM_MESSAGES);
        self.messages.iter().skip(skip).collect()
    }

    /// Format conversation history for LLM prompt
    pub fn format_for_llm(&self) -> String {
        let messages = self.get_context_messages();
        if messages.is_empty() {
            return String::from("No previous conversation.");
        }

        let mut formatted = String::new();
        for msg in messages {
            let timestamp = msg.timestamp.format("%H:%M:%S");
            let action_info = match (&msg.action_taken, msg.action_success) {
                (Some(action), Some(true)) => format!(" [Executed: {}]", action),
                (Some(action), Some(false)) => format!(" [Failed: {}]", action),
                (Some(action), None) => format!(" [Action: {}]", action),
                _ => String::new(),
            };
            formatted.push_str(&format!(
                "[{}] {}: {}{}\n",
                timestamp, msg.role, msg.content, action_info
            ));
        }
        formatted
    }

    /// Get the last action description for context
    pub fn get_last_action_context(&self) -> Option<String> {
        self.last_action.as_ref().map(|action| {
            let payload_info = self
                .last_action_payload
                .as_ref()
                .map(|p| format!(" with {:?}", p))
                .unwrap_or_default();
            format!("{}{}", action, payload_info)
        })
    }

    /// Add an extracted fact
    pub fn add_fact(
        &mut self,
        category: String,
        key: String,
        value: String,
        source_msg_id: String,
    ) {
        // Check if fact already exists and update it
        if let Some(existing) = self.extracted_facts.iter_mut().find(|f| f.key == key) {
            existing.value = value;
            existing.last_used = Utc::now();
            existing.use_count += 1;
            return;
        }

        let fact = Fact {
            id: uuid::Uuid::new_v4().to_string(),
            category,
            key,
            value,
            source_message_id: source_msg_id,
            created_at: Utc::now(),
            last_used: Utc::now(),
            use_count: 1,
        };
        self.extracted_facts.push(fact);

        // Trim old facts if over limit
        if self.extracted_facts.len() > MAX_FACTS_PER_SESSION {
            // Remove least used facts
            self.extracted_facts
                .sort_by_key(|fact| Reverse(fact.use_count));
            self.extracted_facts.truncate(MAX_FACTS_PER_SESSION);
        }
    }

    /// Clear the session
    pub fn clear(&mut self) {
        self.messages.clear();
        self.last_action = None;
        self.last_action_payload = None;
        // Keep facts for continuity
    }

    /// Keep messages within limit
    fn trim_messages(&mut self) {
        if self.messages.len() > MAX_SHORT_TERM_MESSAGES * 2 {
            let drain_count = self.messages.len() - MAX_SHORT_TERM_MESSAGES;
            self.messages.drain(0..drain_count);
        }
    }
}

/// Persistent storage for conversation history and facts
pub struct ConversationStore {
    conn: Mutex<Connection>,
}

impl ConversationStore {
    /// Create or open the conversation database
    pub fn new() -> Result<Self, String> {
        let db_path = Self::get_db_path()?;

        // Ensure directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create data directory: {}", e))?;
        }

        let conn =
            Connection::open(&db_path).map_err(|e| format!("Failed to open database: {}", e))?;

        let store = Self {
            conn: Mutex::new(conn),
        };
        store.init_tables()?;
        Ok(store)
    }

    /// Get the database path
    fn get_db_path() -> Result<PathBuf, String> {
        let data_dir =
            dirs_next::data_dir().ok_or_else(|| "Could not find data directory".to_string())?;
        Ok(data_dir.join("ListenOS").join("conversation.db"))
    }

    /// Initialize database tables
    fn init_tables(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                started_at TEXT NOT NULL,
                ended_at TEXT
            );

            CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                action_taken TEXT,
                action_success INTEGER,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            );

            CREATE TABLE IF NOT EXISTS facts (
                id TEXT PRIMARY KEY,
                category TEXT NOT NULL,
                key TEXT NOT NULL UNIQUE,
                value TEXT NOT NULL,
                source_message_id TEXT,
                created_at TEXT NOT NULL,
                last_used TEXT NOT NULL,
                use_count INTEGER DEFAULT 1
            );

            CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id);
            CREATE INDEX IF NOT EXISTS idx_facts_category ON facts(category);
            CREATE INDEX IF NOT EXISTS idx_facts_key ON facts(key);
            ",
        )
        .map_err(|e| format!("Failed to initialize tables: {}", e))?;

        Ok(())
    }

    /// Save a session to the database
    pub fn save_session(&self, memory: &ConversationMemory) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        // Insert or replace session
        conn.execute(
            "INSERT OR REPLACE INTO sessions (id, started_at) VALUES (?1, ?2)",
            params![memory.session_id, memory.started_at.to_rfc3339()],
        )
        .map_err(|e| format!("Failed to save session: {}", e))?;

        // Save messages
        for msg in &memory.messages {
            conn.execute(
                "INSERT OR REPLACE INTO messages (id, session_id, role, content, timestamp, action_taken, action_success)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    msg.id,
                    memory.session_id,
                    msg.role.to_string(),
                    msg.content,
                    msg.timestamp.to_rfc3339(),
                    msg.action_taken,
                    msg.action_success.map(|b| if b { 1 } else { 0 }),
                ],
            ).map_err(|e| format!("Failed to save message: {}", e))?;
        }

        // Save facts
        for fact in &memory.extracted_facts {
            conn.execute(
                "INSERT OR REPLACE INTO facts (id, category, key, value, source_message_id, created_at, last_used, use_count)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    fact.id,
                    fact.category,
                    fact.key,
                    fact.value,
                    fact.source_message_id,
                    fact.created_at.to_rfc3339(),
                    fact.last_used.to_rfc3339(),
                    fact.use_count,
                ],
            ).map_err(|e| format!("Failed to save fact: {}", e))?;
        }

        Ok(())
    }

    /// Load all facts from database
    pub fn load_facts(&self) -> Result<Vec<Fact>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let mut stmt = conn.prepare(
            "SELECT id, category, key, value, source_message_id, created_at, last_used, use_count 
             FROM facts ORDER BY use_count DESC"
        ).map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let facts = stmt
            .query_map([], |row| {
                Ok(Fact {
                    id: row.get(0)?,
                    category: row.get(1)?,
                    key: row.get(2)?,
                    value: row.get(3)?,
                    source_message_id: row.get(4)?,
                    created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(5)?)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    last_used: DateTime::parse_from_rfc3339(&row.get::<_, String>(6)?)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    use_count: row.get(7)?,
                })
            })
            .map_err(|e| format!("Failed to query facts: {}", e))?;

        facts
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect facts: {}", e))
    }

    /// Get recent sessions
    pub fn get_recent_sessions(&self, limit: usize) -> Result<Vec<String>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let mut stmt = conn
            .prepare("SELECT id FROM sessions ORDER BY started_at DESC LIMIT ?1")
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let sessions = stmt
            .query_map([limit], |row| row.get(0))
            .map_err(|e| format!("Failed to query sessions: {}", e))?;

        sessions
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect sessions: {}", e))
    }

    /// Load messages for a session
    pub fn load_session_messages(&self, session_id: &str) -> Result<Vec<Message>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let mut stmt = conn
            .prepare(
                "SELECT id, role, content, timestamp, action_taken, action_success 
             FROM messages WHERE session_id = ?1 ORDER BY timestamp ASC",
            )
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let messages = stmt
            .query_map([session_id], |row| {
                let role_str: String = row.get(1)?;
                let role = match role_str.as_str() {
                    "user" => Role::User,
                    "assistant" => Role::Assistant,
                    "system" => Role::System,
                    _ => Role::User,
                };

                Ok(Message {
                    id: row.get(0)?,
                    role,
                    content: row.get(2)?,
                    timestamp: DateTime::parse_from_rfc3339(&row.get::<_, String>(3)?)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    action_taken: row.get(4)?,
                    action_success: row.get::<_, Option<i32>>(5)?.map(|v| v != 0),
                })
            })
            .map_err(|e| format!("Failed to query messages: {}", e))?;

        messages
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect messages: {}", e))
    }

    /// Update a fact's usage
    pub fn touch_fact(&self, key: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        conn.execute(
            "UPDATE facts SET last_used = ?1, use_count = use_count + 1 WHERE key = ?2",
            params![Utc::now().to_rfc3339(), key],
        )
        .map_err(|e| format!("Failed to update fact: {}", e))?;

        Ok(())
    }

    /// Delete old sessions (keep last N)
    pub fn cleanup_old_sessions(&self, keep_count: usize) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        // Get sessions to delete
        let mut stmt = conn
            .prepare("SELECT id FROM sessions ORDER BY started_at DESC LIMIT -1 OFFSET ?1")
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let sessions_to_delete: Vec<String> = stmt
            .query_map([keep_count], |row| row.get(0))
            .map_err(|e| format!("Failed to query: {}", e))?
            .filter_map(|r| r.ok())
            .collect();

        drop(stmt);

        if sessions_to_delete.is_empty() {
            return Ok(0);
        }

        // Delete messages for those sessions
        for session_id in &sessions_to_delete {
            conn.execute("DELETE FROM messages WHERE session_id = ?1", [session_id])
                .map_err(|e| format!("Failed to delete messages: {}", e))?;
            conn.execute("DELETE FROM sessions WHERE id = ?1", [session_id])
                .map_err(|e| format!("Failed to delete session: {}", e))?;
        }

        Ok(sessions_to_delete.len())
    }
}

// Add dirs_next dependency for data directory
// This will be added to Cargo.toml
