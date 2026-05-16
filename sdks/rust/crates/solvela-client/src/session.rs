use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

use solvela_protocol::ChatMessage;

const DEFAULT_TTL_SECS: u64 = 1800; // 30 minutes
const MAX_RECENT_HASHES: usize = 10;
const THREE_STRIKE_THRESHOLD: usize = 3;

pub(crate) struct SessionInfo {
    pub model: String,
    pub escalated: bool,
}

struct SessionEntry {
    model: String,
    created: Instant,
    request_count: u64,
    recent_hashes: VecDeque<u64>,
    escalated: bool,
}

pub(crate) struct SessionStore {
    sessions: RwLock<HashMap<String, SessionEntry>>,
    ttl: Duration,
}

impl SessionStore {
    pub(crate) fn new(ttl: Duration) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            ttl,
        }
    }

    pub(crate) async fn get_or_create(&self, session_id: &str, default_model: &str) -> SessionInfo {
        // Check for existing non-expired session first with read lock
        {
            let sessions = self.sessions.read().await;
            if let Some(entry) = sessions.get(session_id) {
                if entry.created.elapsed() < self.ttl {
                    return SessionInfo {
                        model: entry.model.clone(),
                        escalated: entry.escalated,
                    };
                }
            }
        }

        // Create or replace with write lock
        let mut sessions = self.sessions.write().await;
        // Double-check after acquiring write lock
        if let Some(entry) = sessions.get(session_id) {
            if entry.created.elapsed() < self.ttl {
                return SessionInfo {
                    model: entry.model.clone(),
                    escalated: entry.escalated,
                };
            }
        }

        let entry = SessionEntry {
            model: default_model.to_string(),
            created: Instant::now(),
            request_count: 0,
            recent_hashes: VecDeque::new(),
            escalated: false,
        };
        sessions.insert(session_id.to_string(), entry);

        SessionInfo {
            model: default_model.to_string(),
            escalated: false,
        }
    }

    pub(crate) async fn record_request(&self, session_id: &str, request_hash: u64) {
        let mut sessions = self.sessions.write().await;
        let Some(entry) = sessions.get_mut(session_id) else {
            return;
        };

        entry.request_count += 1;

        if entry.recent_hashes.len() >= MAX_RECENT_HASHES {
            entry.recent_hashes.pop_front();
        }
        entry.recent_hashes.push_back(request_hash);

        // Three-strike check: if any hash appears >= 3 times in recent_hashes
        if !entry.escalated {
            let mut counts: HashMap<u64, usize> = HashMap::new();
            for &h in &entry.recent_hashes {
                let count = counts.entry(h).or_insert(0);
                *count += 1;
                if *count >= THREE_STRIKE_THRESHOLD {
                    entry.escalated = true;
                    break;
                }
            }
        }
    }

    pub(crate) async fn cleanup_expired(&self) {
        let mut sessions = self.sessions.write().await;
        let ttl = self.ttl;
        sessions.retain(|_, entry| entry.created.elapsed() < ttl);
    }

    pub(crate) fn derive_session_id(messages: &[ChatMessage]) -> String {
        let mut hasher = DefaultHasher::new();
        if let Some(first) = messages.first() {
            first.content.hash(&mut hasher);
        }
        format!("{:016x}", hasher.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use solvela_protocol::Role;

    fn make_messages(content: &str) -> Vec<ChatMessage> {
        vec![ChatMessage {
            role: Role::User,
            content: content.to_string(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }]
    }

    #[tokio::test]
    async fn new_session_returns_default_model() {
        let store = SessionStore::new(Duration::from_mins(1));
        let info = store.get_or_create("sess-1", "gpt-4o").await;
        assert_eq!(info.model, "gpt-4o");
        assert!(!info.escalated);
    }

    #[tokio::test]
    async fn existing_session_returns_stored_model() {
        let store = SessionStore::new(Duration::from_mins(1));
        store.get_or_create("sess-1", "gpt-4o").await;

        // Second call with different default should still return original model
        let info = store.get_or_create("sess-1", "claude-sonnet").await;
        assert_eq!(info.model, "gpt-4o");
    }

    #[tokio::test]
    async fn expired_session_creates_new_entry() {
        let store = SessionStore::new(Duration::from_millis(1));
        store.get_or_create("sess-1", "gpt-4o").await;

        tokio::time::sleep(Duration::from_millis(5)).await;

        let info = store.get_or_create("sess-1", "claude-sonnet").await;
        assert_eq!(info.model, "claude-sonnet");
    }

    #[tokio::test]
    async fn three_strike_sets_escalated() {
        let store = SessionStore::new(Duration::from_mins(1));
        store.get_or_create("sess-1", "gpt-4o").await;

        let same_hash = 42;
        store.record_request("sess-1", same_hash).await;
        store.record_request("sess-1", same_hash).await;
        store.record_request("sess-1", same_hash).await;

        let info = store.get_or_create("sess-1", "gpt-4o").await;
        assert!(info.escalated);
    }

    #[tokio::test]
    async fn less_than_three_identical_does_not_escalate() {
        let store = SessionStore::new(Duration::from_mins(1));
        store.get_or_create("sess-1", "gpt-4o").await;

        store.record_request("sess-1", 42).await;
        store.record_request("sess-1", 42).await;
        store.record_request("sess-1", 99).await;

        let info = store.get_or_create("sess-1", "gpt-4o").await;
        assert!(!info.escalated);
    }

    #[tokio::test]
    async fn cleanup_expired_removes_old_sessions() {
        let store = SessionStore::new(Duration::from_millis(1));
        store.get_or_create("sess-1", "gpt-4o").await;
        store.get_or_create("sess-2", "gpt-4o").await;

        tokio::time::sleep(Duration::from_millis(5)).await;

        // Create a fresh session that should survive cleanup
        store.get_or_create("sess-3", "claude-sonnet").await;

        store.cleanup_expired().await;

        let sessions = store.sessions.read().await;
        assert_eq!(sessions.len(), 1);
        assert!(sessions.contains_key("sess-3"));
    }

    #[test]
    fn derive_session_id_is_deterministic() {
        let msgs = make_messages("Hello, world!");
        let id1 = SessionStore::derive_session_id(&msgs);
        let id2 = SessionStore::derive_session_id(&msgs);
        assert_eq!(id1, id2);
    }

    #[test]
    fn derive_session_id_differs_for_different_content() {
        let msgs_a = make_messages("Hello");
        let msgs_b = make_messages("Goodbye");
        let id_a = SessionStore::derive_session_id(&msgs_a);
        let id_b = SessionStore::derive_session_id(&msgs_b);
        assert_ne!(id_a, id_b);
    }
}
