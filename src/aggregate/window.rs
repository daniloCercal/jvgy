//! Bounded rolling context window of chat + transcript, capped by both wall-clock
//! age and an approximate token budget. Oldest entries are evicted first.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Chat,
    Transcript,
}

#[derive(Debug, Clone)]
struct Entry {
    source: Source,
    who: String,
    text: String,
    at: Instant,
    tokens: usize,
    summarized: bool,
}

/// Rough token estimate (~4 chars/token). Good enough for budgeting.
pub fn estimate_tokens(text: &str) -> usize {
    (text.chars().count() / 4).max(1)
}

pub struct Window {
    max_age: Duration,
    max_tokens: usize,
    entries: VecDeque<Entry>,
    tokens: usize,
}

impl Window {
    pub fn new(max_age: Duration, max_tokens: usize) -> Self {
        Self {
            max_age,
            max_tokens,
            entries: VecDeque::new(),
            tokens: 0,
        }
    }

    pub fn push_chat(&mut self, who: impl Into<String>, text: impl Into<String>) {
        self.push(Source::Chat, who.into(), text.into());
    }

    pub fn push_transcript(&mut self, text: impl Into<String>) {
        self.push(Source::Transcript, String::new(), text.into());
    }

    fn push(&mut self, source: Source, who: String, text: String) {
        let tokens = estimate_tokens(&text);
        self.entries.push_back(Entry {
            source,
            who,
            text,
            at: Instant::now(),
            tokens,
            summarized: false,
        });
        self.tokens += tokens;
        self.evict(Instant::now());
    }

    fn evict(&mut self, now: Instant) {
        while let Some(front) = self.entries.front() {
            let too_old = now.duration_since(front.at) > self.max_age;
            let over_budget = self.tokens > self.max_tokens;
            if too_old || over_budget {
                self.tokens -= front.tokens;
                self.entries.pop_front();
            } else {
                break;
            }
        }
    }

    /// Tokens not yet included in a summary — drives the volume trigger.
    pub fn unsummarized_tokens(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| !e.summarized)
            .map(|e| e.tokens)
            .sum()
    }

    pub fn has_unsummarized(&self) -> bool {
        self.entries.iter().any(|e| !e.summarized)
    }

    /// Render only the not-yet-summarized entries (the incremental delta sent to
    /// the LLM). Does not mutate state — call `mark_summarized` after a success.
    pub fn render_delta(&self) -> String {
        let mut out = String::new();
        for e in self.entries.iter().filter(|e| !e.summarized) {
            match e.source {
                Source::Chat => {
                    out.push_str("[chat] ");
                    out.push_str(&e.who);
                    out.push_str(": ");
                }
                Source::Transcript => out.push_str("[fala] "),
            }
            out.push_str(&e.text);
            out.push('\n');
        }
        out
    }

    pub fn mark_summarized(&mut self) {
        for e in self.entries.iter_mut() {
            e.summarized = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_oldest_when_over_token_budget() {
        // Budget ~ 5 tokens. Each 8-char message ~ 2 tokens.
        let mut w = Window::new(Duration::from_secs(3600), 5);
        w.push_transcript("aaaabbbb"); // ~2
        w.push_transcript("ccccdddd"); // ~2
        w.push_transcript("eeeeffff"); // ~2 -> total 6 > 5, evicts the first
        let delta = w.render_delta();
        assert!(!delta.contains("aaaabbbb"), "oldest should be evicted");
        assert!(delta.contains("eeeeffff"));
    }

    #[test]
    fn delta_then_mark_summarized() {
        let mut w = Window::new(Duration::from_secs(3600), 10_000);
        w.push_chat("alice", "ola");
        w.push_transcript("falando agora");
        assert!(w.has_unsummarized());
        let d = w.render_delta();
        assert!(d.contains("[chat] alice: ola"));
        assert!(d.contains("[fala] falando agora"));
        w.mark_summarized();
        assert!(!w.has_unsummarized());
        assert_eq!(w.unsummarized_tokens(), 0);
        assert_eq!(w.render_delta(), "");
    }
}
