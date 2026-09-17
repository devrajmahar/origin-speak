//! Error Logging System for ListenOS
//!
//! Tracks errors that occur during voice processing and command execution
//! so they can be displayed to the user.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Types of errors that can occur
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ErrorType {
    Transcription,   // Whisper API failed
    LLMProcessing,   // Intent/LLM processing failed
    ActionExecution, // Command execution failed
    AudioCapture,    // Microphone/recording issue
    Network,         // Network/API connectivity
    RateLimit,       // API rate limit hit
    Unknown,         // Other errors
}

/// A logged error entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorEntry {
    pub id: String,
    pub error_type: ErrorType,
    pub message: String,
    pub details: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub dismissed: bool,
}

impl ErrorEntry {
    pub fn new(error_type: ErrorType, message: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            error_type,
            message: message.into(),
            details: None,
            timestamp: Utc::now(),
            dismissed: false,
        }
    }

    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }
}

/// In-memory error log
pub struct ErrorLog {
    entries: VecDeque<ErrorEntry>,
    max_entries: usize,
}

impl ErrorLog {
    pub fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            max_entries: 100,
        }
    }

    /// Log a new error
    pub fn log(&mut self, entry: ErrorEntry) {
        log::error!(
            "[{:?}] {}: {}",
            entry.error_type,
            entry.message,
            entry.details.as_deref().unwrap_or("")
        );

        self.entries.push_front(entry);

        // Keep under max
        while self.entries.len() > self.max_entries {
            self.entries.pop_back();
        }
    }

    /// Log an error with type and message
    pub fn log_error(&mut self, error_type: ErrorType, message: impl Into<String>) {
        self.log(ErrorEntry::new(error_type, message));
    }

    /// Log an error with details
    pub fn log_error_with_details(
        &mut self,
        error_type: ErrorType,
        message: impl Into<String>,
        details: impl Into<String>,
    ) {
        self.log(ErrorEntry::new(error_type, message).with_details(details));
    }

    /// Get all undismissed errors
    pub fn get_undismissed(&self) -> Vec<ErrorEntry> {
        self.entries
            .iter()
            .filter(|e| !e.dismissed)
            .cloned()
            .collect()
    }

    /// Get recent errors (last n)
    pub fn get_recent(&self, limit: usize) -> Vec<ErrorEntry> {
        self.entries.iter().take(limit).cloned().collect()
    }

    /// Dismiss an error by ID
    pub fn dismiss(&mut self, id: &str) -> bool {
        for entry in &mut self.entries {
            if entry.id == id {
                entry.dismissed = true;
                return true;
            }
        }
        false
    }

    /// Dismiss all errors
    pub fn dismiss_all(&mut self) {
        for entry in &mut self.entries {
            entry.dismissed = true;
        }
    }

    /// Clear all errors
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Check if there are any undismissed errors
    pub fn has_undismissed(&self) -> bool {
        self.entries.iter().any(|e| !e.dismissed)
    }

    /// Get count of undismissed errors
    pub fn undismissed_count(&self) -> usize {
        self.entries.iter().filter(|e| !e.dismissed).count()
    }
}

impl Default for ErrorLog {
    fn default() -> Self {
        Self::new()
    }
}
