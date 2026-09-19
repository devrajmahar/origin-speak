//! Dictionary Storage System for Origin Speak
//!
//! Custom words and spellings for voice recognition.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

/// A custom dictionary word
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictionaryWord {
    pub id: String,
    pub word: String,             // The correct spelling
    pub phonetic: Option<String>, // How it sounds (optional)
    pub category: String,         // "personal" or "shared"
    pub is_auto_learned: bool,    // Was it learned automatically?
    pub created_at: DateTime<Utc>,
    pub use_count: u32,
}

impl DictionaryWord {
    pub fn new(word: String, is_auto_learned: bool) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            word,
            phonetic: None,
            category: "personal".to_string(),
            is_auto_learned,
            created_at: Utc::now(),
            use_count: 0,
        }
    }
}

/// Persistent storage for dictionary words
pub struct DictionaryStore {
    conn: Mutex<Connection>,
}

impl DictionaryStore {
    /// Create or open the dictionary database
    pub fn new() -> Result<Self, String> {
        let db_path = Self::get_db_path()?;

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

    fn get_db_path() -> Result<PathBuf, String> {
        let candidates = crate::paths::app_data_file_candidates("dictionary.db")?;
        let active = candidates[0].clone();
        if crate::paths::path_exists_no_follow(&active) {
            if crate::paths::regular_file_exists_no_follow(&active) {
                return Ok(active);
            }
            return Err(format!(
                "Refusing non-regular Origin Speak dictionary path: {}",
                active.display()
            ));
        }
        Ok(candidates
            .into_iter()
            .skip(1)
            .find(|path| crate::paths::regular_file_exists_no_follow(path))
            .unwrap_or(active))
    }

    fn init_tables(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS words (
                id TEXT PRIMARY KEY,
                word TEXT NOT NULL UNIQUE,
                phonetic TEXT,
                category TEXT NOT NULL DEFAULT 'personal',
                is_auto_learned INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                use_count INTEGER NOT NULL DEFAULT 0
            );

            CREATE INDEX IF NOT EXISTS idx_words_word ON words(word);
            CREATE INDEX IF NOT EXISTS idx_words_category ON words(category);
            ",
        )
        .map_err(|e| format!("Failed to initialize tables: {}", e))?;

        Ok(())
    }

    /// Add a new word to the dictionary
    pub fn add_word(&self, word: String, is_auto_learned: bool) -> Result<DictionaryWord, String> {
        let dict_word = DictionaryWord::new(word, is_auto_learned);
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        conn.execute(
            "INSERT OR REPLACE INTO words (id, word, phonetic, category, is_auto_learned, created_at, use_count) 
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                dict_word.id,
                dict_word.word,
                dict_word.phonetic,
                dict_word.category,
                if dict_word.is_auto_learned { 1 } else { 0 },
                dict_word.created_at.to_rfc3339(),
                dict_word.use_count,
            ],
        ).map_err(|e| format!("Failed to add word: {}", e))?;

        Ok(dict_word)
    }

    /// Get all dictionary words
    pub fn get_all_words(&self) -> Result<Vec<DictionaryWord>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let mut stmt = conn
            .prepare(
                "SELECT id, word, phonetic, category, is_auto_learned, created_at, use_count 
             FROM words ORDER BY use_count DESC, word ASC",
            )
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let words = stmt
            .query_map([], |row| {
                Ok(DictionaryWord {
                    id: row.get(0)?,
                    word: row.get(1)?,
                    phonetic: row.get(2)?,
                    category: row.get(3)?,
                    is_auto_learned: row.get::<_, i32>(4)? != 0,
                    created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(5)?)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    use_count: row.get(6)?,
                })
            })
            .map_err(|e| format!("Failed to query words: {}", e))?;

        words
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect words: {}", e))
    }

    /// Update a word
    pub fn update_word(
        &self,
        id: &str,
        word: String,
        phonetic: Option<String>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        conn.execute(
            "UPDATE words SET word = ?1, phonetic = ?2 WHERE id = ?3",
            params![word, phonetic, id],
        )
        .map_err(|e| format!("Failed to update word: {}", e))?;

        Ok(())
    }

    /// Delete a word
    pub fn delete_word(&self, id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        conn.execute("DELETE FROM words WHERE id = ?1", [id])
            .map_err(|e| format!("Failed to delete word: {}", e))?;

        Ok(())
    }

    /// Get all words for voice recognition context
    pub fn get_words_for_recognition(&self) -> Result<Vec<String>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let mut stmt = conn
            .prepare("SELECT word FROM words ORDER BY use_count DESC")
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let words = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| format!("Failed to query words: {}", e))?;

        words
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect words: {}", e))
    }
}
