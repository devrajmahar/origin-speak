use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ErrorType {
    AudioCapture,
    Transcription,
    Delivery,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorEntry {
    pub error_type: ErrorType,
    pub message: String,
    pub details: Option<String>,
    pub timestamp: DateTime<Utc>,
}

impl ErrorEntry {
    fn new(error_type: ErrorType, message: impl Into<String>) -> Self {
        Self {
            error_type,
            message: message.into(),
            details: None,
            timestamp: Utc::now(),
        }
    }

    fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }
}

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

    pub fn log_error_with_details(
        &mut self,
        error_type: ErrorType,
        message: impl Into<String>,
        details: impl Into<String>,
    ) {
        let entry = ErrorEntry::new(error_type, message).with_details(details);
        log::error!(
            "[{:?}] {}: {}",
            entry.error_type,
            entry.message,
            entry.details.as_deref().unwrap_or("")
        );
        self.entries.push_front(entry);
        while self.entries.len() > self.max_entries {
            self.entries.pop_back();
        }
    }
}

impl Default for ErrorLog {
    fn default() -> Self {
        Self::new()
    }
}
