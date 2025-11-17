use rand::Rng;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// Default session lifetime: 24 hours
pub const DEFAULT_SESSION_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

/// Session ID length in bytes (32 bytes = 64 hex chars)
const SESSION_ID_BYTES: usize = 32;

/// Session data associated with each session ID
#[derive(Debug, Clone)]
pub struct SessionData {
    /// User ID associated with this session
    pub user_id: String,
    /// When the session was created
    pub created_at: Instant,
    /// When the session expires
    pub expires_at: Instant,
}

impl SessionData {
    /// Creates a new session data
    fn new(user_id: String, lifetime: Duration) -> Self {
        let now = Instant::now();
        Self {
            user_id,
            created_at: now,
            expires_at: now + lifetime,
        }
    }

    /// Checks if the session is expired
    fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}

/// In-memory session store
#[derive(Debug, Clone)]
pub struct SessionStore {
    sessions: Arc<RwLock<HashMap<String, SessionData>>>,
    session_lifetime: Duration,
}

impl SessionStore {
    /// Creates a new session store
    pub fn new() -> Self {
        Self::with_lifetime(DEFAULT_SESSION_LIFETIME)
    }

    /// Creates a new session store with custom lifetime
    pub fn with_lifetime(lifetime: Duration) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            session_lifetime: lifetime,
        }
    }

    /// Generates a cryptographically random session ID
    fn generate_session_id() -> String {
        let mut rng = rand::thread_rng();
        let bytes: Vec<u8> = (0..SESSION_ID_BYTES).map(|_| rng.gen()).collect();
        hex::encode(bytes)
    }

    /// Creates a new session for the given user
    pub fn create_session(&self, user_id: String) -> String {
        let session_id = Self::generate_session_id();
        let session_data = SessionData::new(user_id.clone(), self.session_lifetime);

        debug!("Creating session {} for user: {}", session_id, user_id);

        let mut sessions = self.sessions.write().unwrap();
        sessions.insert(session_id.clone(), session_data);

        session_id
    }

    /// Gets the user ID for a session if it exists and is not expired
    pub fn get_session(&self, session_id: &str) -> Option<String> {
        let sessions = self.sessions.read().unwrap();

        match sessions.get(session_id) {
            Some(session_data) => {
                if session_data.is_expired() {
                    debug!("Session {} is expired", session_id);
                    None
                } else {
                    Some(session_data.user_id.clone())
                }
            }
            None => None,
        }
    }

    /// Validates a session and returns the user ID if valid
    pub fn validate_session(&self, session_id: &str) -> Option<String> {
        self.get_session(session_id)
    }

    /// Deletes a session (logout)
    pub fn delete_session(&self, session_id: &str) -> bool {
        debug!("Deleting session: {}", session_id);
        let mut sessions = self.sessions.write().unwrap();
        sessions.remove(session_id).is_some()
    }

    /// Cleans up expired sessions
    pub fn cleanup_expired(&self) -> usize {
        let mut sessions = self.sessions.write().unwrap();
        let initial_count = sessions.len();

        sessions.retain(|session_id, session_data| {
            if session_data.is_expired() {
                debug!("Removing expired session: {}", session_id);
                false
            } else {
                true
            }
        });

        let removed = initial_count - sessions.len();
        if removed > 0 {
            debug!("Cleaned up {} expired sessions", removed);
        }
        removed
    }

    /// Returns the number of active sessions
    pub fn active_session_count(&self) -> usize {
        let sessions = self.sessions.read().unwrap();
        sessions
            .values()
            .filter(|session_data| !session_data.is_expired())
            .count()
    }

    /// Returns the total number of sessions (including expired)
    pub fn total_session_count(&self) -> usize {
        let sessions = self.sessions.read().unwrap();
        sessions.len()
    }

    /// Refreshes a session's expiry time (extends it)
    pub fn refresh_session(&self, session_id: &str) -> bool {
        let mut sessions = self.sessions.write().unwrap();

        if let Some(session_data) = sessions.get_mut(session_id) {
            if !session_data.is_expired() {
                session_data.expires_at = Instant::now() + self.session_lifetime;
                debug!("Refreshed session: {}", session_id);
                return true;
            } else {
                warn!("Attempted to refresh expired session: {}", session_id);
            }
        }

        false
    }

    /// Deletes all sessions for a specific user
    pub fn delete_user_sessions(&self, user_id: &str) -> usize {
        debug!("Deleting all sessions for user: {}", user_id);
        let mut sessions = self.sessions.write().unwrap();
        let initial_count = sessions.len();

        sessions.retain(|_, session_data| session_data.user_id != user_id);

        let removed = initial_count - sessions.len();
        if removed > 0 {
            debug!("Removed {} sessions for user: {}", removed, user_id);
        }
        removed
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_session_creation_and_validation() {
        let store = SessionStore::new();
        let session_id = store.create_session("testuser".to_string());

        assert_eq!(session_id.len(), SESSION_ID_BYTES * 2); // hex encoding doubles length
        assert_eq!(store.get_session(&session_id), Some("testuser".to_string()));
        assert_eq!(store.active_session_count(), 1);
    }

    #[test]
    fn test_session_deletion() {
        let store = SessionStore::new();
        let session_id = store.create_session("testuser".to_string());

        assert!(store.delete_session(&session_id));
        assert_eq!(store.get_session(&session_id), None);
        assert_eq!(store.active_session_count(), 0);
    }

    #[test]
    fn test_session_expiry() {
        let store = SessionStore::with_lifetime(Duration::from_millis(100));
        let session_id = store.create_session("testuser".to_string());

        assert_eq!(store.get_session(&session_id), Some("testuser".to_string()));

        // Wait for session to expire
        thread::sleep(Duration::from_millis(150));

        assert_eq!(store.get_session(&session_id), None);
    }

    #[test]
    fn test_cleanup_expired() {
        let store = SessionStore::with_lifetime(Duration::from_millis(100));
        let _session1 = store.create_session("user1".to_string());
        let _session2 = store.create_session("user2".to_string());

        assert_eq!(store.total_session_count(), 2);

        // Wait for sessions to expire
        thread::sleep(Duration::from_millis(150));

        let removed = store.cleanup_expired();
        assert_eq!(removed, 2);
        assert_eq!(store.total_session_count(), 0);
    }

    #[test]
    fn test_session_refresh() {
        let store = SessionStore::with_lifetime(Duration::from_millis(200));
        let session_id = store.create_session("testuser".to_string());

        // Wait half the lifetime
        thread::sleep(Duration::from_millis(100));

        // Refresh the session
        assert!(store.refresh_session(&session_id));

        // Wait another half lifetime (should still be valid due to refresh)
        thread::sleep(Duration::from_millis(100));

        assert_eq!(store.get_session(&session_id), Some("testuser".to_string()));
    }

    #[test]
    fn test_delete_user_sessions() {
        let store = SessionStore::new();
        let _session1 = store.create_session("user1".to_string());
        let _session2 = store.create_session("user1".to_string());
        let session3 = store.create_session("user2".to_string());

        assert_eq!(store.total_session_count(), 3);

        let removed = store.delete_user_sessions("user1");
        assert_eq!(removed, 2);
        assert_eq!(store.total_session_count(), 1);
        assert_eq!(store.get_session(&session3), Some("user2".to_string()));
    }

    #[test]
    fn test_unique_session_ids() {
        let store = SessionStore::new();
        let session1 = store.create_session("user1".to_string());
        let session2 = store.create_session("user2".to_string());

        assert_ne!(session1, session2);
    }
}
