pub mod deepl;
pub mod ollama;

use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Translation {
    pub text: String,
    pub detected_source: Option<String>,
}

/// Small LRU keyed by (target, text). Front = least recently used.
pub struct TranslationCache {
    capacity: usize,
    entries: VecDeque<((String, String), Translation)>,
}

impl TranslationCache {
    pub fn new(capacity: usize) -> Self {
        Self { capacity, entries: VecDeque::new() }
    }

    pub fn get(&mut self, target: &str, text: &str) -> Option<Translation> {
        let i = self.entries.iter().position(|((t, x), _)| t == target && x == text)?;
        let entry = self.entries.remove(i)?;
        let value = entry.1.clone();
        self.entries.push_back(entry);
        Some(value)
    }

    pub fn put(&mut self, target: &str, text: &str, t: Translation) {
        if let Some(i) = self.entries.iter().position(|((tg, x), _)| tg == target && x == text) {
            self.entries.remove(i);
        }
        self.entries.push_back(((target.to_owned(), text.to_owned()), t));
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
