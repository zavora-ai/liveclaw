use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::RwLock;

/// Shell injection patterns to detect in tool arguments.
const SHELL_PATTERNS: &[&str] = &[";", "`", "$(", "|", "&&", "||", ">", "<", "\n"];

/// Check if a string contains shell injection patterns.
pub fn contains_shell_injection(input: &str) -> bool {
    SHELL_PATTERNS.iter().any(|pattern| input.contains(pattern))
}

/// Per-session rate limiter for tool executions.
pub struct RateLimiter {
    limit: u32,
    counts: RwLock<HashMap<String, AtomicU32>>,
}

impl RateLimiter {
    /// Create a new rate limiter with the given per-session limit.
    pub fn new(limit: u32) -> Self {
        Self {
            limit,
            counts: RwLock::new(HashMap::new()),
        }
    }

    /// Check if the session is under the rate limit and increment the counter.
    /// Returns `true` if the call is allowed (under limit), `false` if blocked.
    pub fn check_and_increment(&self, session_id: &str) -> bool {
        // First, try a read lock to check/increment an existing entry
        {
            let counts = self.counts.read().unwrap();
            if let Some(counter) = counts.get(session_id) {
                let current = counter.fetch_add(1, Ordering::SeqCst);
                if current >= self.limit {
                    // Undo the increment since we're over the limit
                    counter.fetch_sub(1, Ordering::SeqCst);
                    return false;
                }
                return true;
            }
        }

        // Session not found — take a write lock to insert
        let mut counts = self.counts.write().unwrap();
        // Double-check after acquiring write lock
        if let Some(counter) = counts.get(session_id) {
            let current = counter.fetch_add(1, Ordering::SeqCst);
            if current >= self.limit {
                counter.fetch_sub(1, Ordering::SeqCst);
                return false;
            }
            return true;
        }

        // Insert new counter starting at 1
        counts.insert(session_id.to_string(), AtomicU32::new(1));
        true
    }

    /// Reset the counter for a given session.
    pub fn reset(&self, session_id: &str) {
        let counts = self.counts.read().unwrap();
        if let Some(counter) = counts.get(session_id) {
            counter.store(0, Ordering::SeqCst);
        }
    }
}

// ---------------------------------------------------------------------------
// Access control (adk-auth)
// ---------------------------------------------------------------------------

use adk_auth::{AccessControl, FileAuditSink, Permission, Role};

/// Security configuration used by `build_access_control`.
///
/// The full version lives in `config.rs`; this is the interface needed here.
pub struct SecurityConfig {
    pub default_role: String,
    pub tool_allowlist: Vec<String>,
    pub rate_limit_per_session: u32,
    pub audit_log_path: String,
}

/// Build an [`AccessControl`] instance with three roles:
///
/// * **readonly** – no tool permissions.
/// * **supervised** – only the tools listed in `config.tool_allowlist`.
/// * **full** – all tools permitted via [`Permission::AllTools`].
///
/// A [`FileAuditSink`] is attached so every `check_and_audit` call is logged
/// in JSONL format to `config.audit_log_path`.
pub fn build_access_control(config: &SecurityConfig) -> AccessControl {
    // ReadOnly: no permissions added → every tool check will be denied.
    let readonly = Role::new("readonly");

    // Supervised: explicitly allow each tool in the allowlist.
    let mut supervised = Role::new("supervised");
    for tool_name in &config.tool_allowlist {
        supervised = supervised.allow(Permission::Tool(tool_name.clone()));
    }

    // Full: grant access to all tools.
    let full = Role::new("full").allow(Permission::AllTools);

    // Audit sink writes JSONL to the configured path.
    let sink = FileAuditSink::new(&config.audit_log_path).expect("Failed to create audit sink");

    AccessControl::builder()
        .role(readonly)
        .role(supervised)
        .role(full)
        .audit_sink(sink)
        .build()
        .expect("Failed to build AccessControl")
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Shell injection detection tests ---

    #[test]
    fn detects_semicolon() {
        assert!(contains_shell_injection("ls; rm -rf /"));
    }

    #[test]
    fn detects_backtick() {
        assert!(contains_shell_injection("echo `whoami`"));
    }

    #[test]
    fn detects_dollar_paren() {
        assert!(contains_shell_injection("echo $(id)"));
    }

    #[test]
    fn detects_pipe() {
        assert!(contains_shell_injection("cat /etc/passwd | grep root"));
    }

    #[test]
    fn detects_double_ampersand() {
        assert!(contains_shell_injection("true && rm -rf /"));
    }

    #[test]
    fn detects_double_pipe() {
        assert!(contains_shell_injection("false || echo pwned"));
    }

    #[test]
    fn detects_redirect_gt() {
        assert!(contains_shell_injection("echo bad > /etc/passwd"));
    }

    #[test]
    fn detects_redirect_lt() {
        assert!(contains_shell_injection("cat < /etc/shadow"));
    }

    #[test]
    fn detects_newline() {
        assert!(contains_shell_injection("safe\nunsafe"));
    }

    #[test]
    fn clean_string_not_flagged() {
        assert!(!contains_shell_injection("hello world"));
    }

    #[test]
    fn empty_string_not_flagged() {
        assert!(!contains_shell_injection(""));
    }

    #[test]
    fn normal_arguments_not_flagged() {
        assert!(!contains_shell_injection("search query with spaces"));
        assert!(!contains_shell_injection("file.txt"));
        assert!(!contains_shell_injection("user@example.com"));
    }

    // --- Rate limiter tests ---

    #[test]
    fn allows_up_to_limit() {
        let limiter = RateLimiter::new(3);
        assert!(limiter.check_and_increment("s1"));
        assert!(limiter.check_and_increment("s1"));
        assert!(limiter.check_and_increment("s1"));
    }

    #[test]
    fn blocks_after_limit() {
        let limiter = RateLimiter::new(3);
        assert!(limiter.check_and_increment("s1"));
        assert!(limiter.check_and_increment("s1"));
        assert!(limiter.check_and_increment("s1"));
        assert!(!limiter.check_and_increment("s1"));
        assert!(!limiter.check_and_increment("s1"));
    }

    #[test]
    fn reset_allows_again() {
        let limiter = RateLimiter::new(2);
        assert!(limiter.check_and_increment("s1"));
        assert!(limiter.check_and_increment("s1"));
        assert!(!limiter.check_and_increment("s1"));

        limiter.reset("s1");

        assert!(limiter.check_and_increment("s1"));
        assert!(limiter.check_and_increment("s1"));
        assert!(!limiter.check_and_increment("s1"));
    }

    #[test]
    fn independent_sessions() {
        let limiter = RateLimiter::new(2);
        assert!(limiter.check_and_increment("s1"));
        assert!(limiter.check_and_increment("s1"));
        assert!(!limiter.check_and_increment("s1"));

        // s2 should be independent
        assert!(limiter.check_and_increment("s2"));
        assert!(limiter.check_and_increment("s2"));
        assert!(!limiter.check_and_increment("s2"));
    }

    #[test]
    fn limit_of_one() {
        let limiter = RateLimiter::new(1);
        assert!(limiter.check_and_increment("s1"));
        assert!(!limiter.check_and_increment("s1"));
    }

    #[test]
    fn reset_nonexistent_session_is_noop() {
        let limiter = RateLimiter::new(5);
        limiter.reset("nonexistent"); // should not panic
    }
}
