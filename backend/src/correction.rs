//! Correction Learning System for ListenOS
//!
//! Tracks what text was typed via voice and detects when the user
//! corrects it, then learns from those corrections.

use chrono::{DateTime, Duration, Utc};
use std::collections::VecDeque;

/// A record of text that was typed via voice
#[derive(Debug, Clone)]
pub struct TypedTextRecord {
    pub original_text: String, // What was transcribed
    pub typed_text: String,    // What was actually typed (may be refined)
    pub timestamp: DateTime<Utc>,
}

/// Tracks recent typed text for correction detection
pub struct CorrectionTracker {
    /// Recent typed records (last 5 minutes worth)
    recent_typed: VecDeque<TypedTextRecord>,
    /// Maximum age to keep records
    max_age_minutes: i64,
    /// Maximum records to keep
    max_records: usize,
}

impl CorrectionTracker {
    pub fn new() -> Self {
        Self {
            recent_typed: VecDeque::new(),
            max_age_minutes: 5,
            max_records: 50,
        }
    }

    /// Record that text was typed
    pub fn record_typed(&mut self, original: String, typed: String) {
        // Clean up old records first
        self.cleanup_old_records();

        self.recent_typed.push_back(TypedTextRecord {
            original_text: original,
            typed_text: typed,
            timestamp: Utc::now(),
        });

        // Keep under max records
        while self.recent_typed.len() > self.max_records {
            self.recent_typed.pop_front();
        }
    }

    /// Check if user corrected any recent text and learn from it
    /// Returns list of (original_word, corrected_word) pairs
    pub fn detect_corrections(&mut self, new_text: &str) -> Vec<(String, String)> {
        self.cleanup_old_records();

        let mut corrections = Vec::new();
        let new_words: Vec<&str> = new_text.split_whitespace().collect();

        // Look through recent typed records
        for record in &self.recent_typed {
            let typed_words: Vec<&str> = record.typed_text.split_whitespace().collect();

            // Find words that appear corrected
            // Simple heuristic: if a word in new_text is similar to but different from typed_text
            for typed_word in &typed_words {
                for new_word in &new_words {
                    // Check if this looks like a correction (similar but different)
                    if is_likely_correction(typed_word, new_word) {
                        corrections.push((typed_word.to_string(), new_word.to_string()));
                    }
                }
            }
        }

        corrections
    }

    /// Get all recent typed text (for debugging)
    pub fn get_recent(&self) -> Vec<TypedTextRecord> {
        self.recent_typed.iter().cloned().collect()
    }

    fn cleanup_old_records(&mut self) {
        let cutoff = Utc::now() - Duration::minutes(self.max_age_minutes);
        while let Some(front) = self.recent_typed.front() {
            if front.timestamp < cutoff {
                self.recent_typed.pop_front();
            } else {
                break;
            }
        }
    }
}

impl Default for CorrectionTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if word2 looks like a correction of word1
fn is_likely_correction(original: &str, corrected: &str) -> bool {
    let orig_lower = original.to_lowercase();
    let corr_lower = corrected.to_lowercase();

    // Must be different
    if orig_lower == corr_lower {
        return false;
    }

    // Both must be substantial words (at least 3 chars)
    if orig_lower.len() < 3 || corr_lower.len() < 3 {
        return false;
    }

    // Calculate similarity using Levenshtein-like approach
    let len_diff = (orig_lower.len() as i32 - corr_lower.len() as i32).abs();

    // Length difference should be small (max 2 chars)
    if len_diff > 2 {
        return false;
    }

    let edit_distance = levenshtein_distance(&orig_lower, &corr_lower);
    if (1..=2).contains(&edit_distance) {
        return true;
    }

    // Count matching characters in order
    let matching = count_matching_chars(&orig_lower, &corr_lower);
    let min_len = orig_lower.len().min(corr_lower.len());

    // At least 60% of shorter word should match
    let match_ratio = matching as f32 / min_len as f32;

    // It's a correction if:
    // - High similarity (60-95% matching) - not too similar (typo), not too different (different word)
    (0.5..1.0).contains(&match_ratio)
}

fn levenshtein_distance(a: &str, b: &str) -> usize {
    let b_chars: Vec<char> = b.chars().collect();
    let mut costs: Vec<usize> = (0..=b_chars.len()).collect();

    for (i, a_char) in a.chars().enumerate() {
        let mut previous_diagonal = costs[0];
        costs[0] = i + 1;

        for (j, b_char) in b_chars.iter().enumerate() {
            let insertion = costs[j + 1] + 1;
            let deletion = costs[j] + 1;
            let substitution = previous_diagonal + usize::from(a_char != *b_char);
            previous_diagonal = costs[j + 1];
            costs[j + 1] = insertion.min(deletion).min(substitution);
        }
    }

    costs[b_chars.len()]
}

/// Count characters that match in order between two strings
fn count_matching_chars(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();

    let mut matches = 0;
    let mut b_idx = 0;

    for a_char in &a_chars {
        while b_idx < b_chars.len() {
            if b_chars[b_idx] == *a_char {
                matches += 1;
                b_idx += 1;
                break;
            }
            b_idx += 1;
        }
    }

    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_correction_detection() {
        assert!(is_likely_correction("recieve", "receive"));
        assert!(is_likely_correction("teh", "the"));
        assert!(is_likely_correction("helo", "hello"));
        assert!(!is_likely_correction("hello", "hello")); // same word
        assert!(!is_likely_correction("cat", "elephant")); // too different
    }
}
