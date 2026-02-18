use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Authentication guard that manages pairing codes, token hashing, and brute-force lockout.
///
/// When `require_pairing` is true and no existing tokens are provided, a 6-digit pairing
/// code is generated. Clients submit this code via `try_pair()` to receive a cryptographic
/// token. The token's SHA-256 hash is stored; subsequent authentication uses constant-time
/// hash comparison via `is_authenticated()`.
///
/// After `max_attempts` consecutive failures, pairing is locked out for `lockout_duration`.
pub struct PairingGuard {
    require_pairing: bool,
    pairing_code: Option<String>,
    token_hashes: RwLock<Vec<String>>,
    failed_attempts: AtomicU32,
    max_attempts: u32,
    lockout_duration: Duration,
    lockout_until: RwLock<Option<Instant>>,
}

impl PairingGuard {
    /// Create a new PairingGuard.
    ///
    /// - If `require_pairing` is false, all auth checks are bypassed.
    /// - If `existing_tokens` is non-empty, their SHA-256 hashes are stored and no pairing
    ///   code is generated.
    /// - If `existing_tokens` is empty, a 6-digit pairing code is generated.
    pub fn new(require_pairing: bool, existing_tokens: &[String]) -> Self {
        let (pairing_code, token_hashes) = if !require_pairing {
            (None, Vec::new())
        } else if existing_tokens.is_empty() {
            let code = generate_pairing_code();
            tracing::info!("Pairing code: {}", code);
            (Some(code), Vec::new())
        } else {
            let hashes = existing_tokens.iter().map(|t| hash_token(t)).collect();
            (None, hashes)
        };

        Self {
            require_pairing,
            pairing_code,
            token_hashes: RwLock::new(token_hashes),
            failed_attempts: AtomicU32::new(0),
            max_attempts: 5,
            lockout_duration: Duration::from_secs(300),
            lockout_until: RwLock::new(None),
        }
    }

    /// Create a PairingGuard with custom lockout parameters.
    pub fn with_lockout(
        require_pairing: bool,
        existing_tokens: &[String],
        max_attempts: u32,
        lockout_duration: Duration,
    ) -> Self {
        let mut guard = Self::new(require_pairing, existing_tokens);
        guard.max_attempts = max_attempts;
        guard.lockout_duration = lockout_duration;
        guard
    }

    /// Returns the current pairing code, if one was generated.
    pub fn pairing_code(&self) -> Option<String> {
        self.pairing_code.clone()
    }

    /// Attempt to pair with the given code.
    ///
    /// Returns:
    /// - `Ok(Some(token))` — correct code; token is the raw secret the client should store.
    /// - `Ok(None)` — pairing not required (always succeeds).
    /// - `Err(lockout_secs)` — locked out; value is remaining seconds.
    pub fn try_pair(&self, code: &str) -> Result<Option<String>, u64> {
        if !self.require_pairing {
            return Ok(None);
        }

        // Check lockout
        {
            let lockout = self.lockout_until.read().unwrap();
            if let Some(until) = *lockout {
                let now = Instant::now();
                if now < until {
                    let remaining = until.duration_since(now).as_secs();
                    return Err(remaining.max(1));
                }
            }
        }

        // Compare code
        let expected = match &self.pairing_code {
            Some(c) => c,
            None => return Err(0), // No pairing code available (tokens already exist)
        };

        if code == expected {
            // Success — reset failed attempts, generate token, store hash
            self.failed_attempts.store(0, Ordering::SeqCst);
            let token = Uuid::new_v4().to_string();
            let hash = hash_token(&token);
            self.token_hashes.write().unwrap().push(hash);
            Ok(Some(token))
        } else {
            // Failure — increment counter, maybe trigger lockout
            let prev = self.failed_attempts.fetch_add(1, Ordering::SeqCst);
            let attempts = prev + 1;
            if attempts >= self.max_attempts {
                let until = Instant::now() + self.lockout_duration;
                *self.lockout_until.write().unwrap() = Some(until);
                self.failed_attempts.store(0, Ordering::SeqCst);
                return Err(self.lockout_duration.as_secs());
            }
            Err(0)
        }
    }

    /// Check whether a raw token is authenticated.
    ///
    /// Computes the SHA-256 hash of the presented token and compares it against all stored
    /// hashes using constant-time byte comparison to prevent timing attacks.
    ///
    /// When `require_pairing` is false, returns true for any token.
    pub fn is_authenticated(&self, token: &str) -> bool {
        if !self.require_pairing {
            return true;
        }

        let presented_hash = hash_token(token);
        let hashes = self.token_hashes.read().unwrap();
        hashes.iter().any(|stored| constant_time_eq(stored.as_bytes(), presented_hash.as_bytes()))
    }
}

// SAFETY: PairingGuard uses only Send+Sync primitives (RwLock, AtomicU32).
unsafe impl Send for PairingGuard {}
unsafe impl Sync for PairingGuard {}

/// Compute the hex-encoded SHA-256 hash of a token string.
fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let result = hasher.finalize();
    hex_encode(&result)
}

/// Hex-encode a byte slice (lowercase).
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Generate a 6-digit pairing code derived from a UUID v4.
///
/// Takes the first 8 hex chars of a UUID, parses as u32, and mods by 1_000_000
/// to produce a zero-padded 6-digit decimal string.
fn generate_pairing_code() -> String {
    let uuid = Uuid::new_v4();
    let hex_str: String = uuid.to_string().chars().filter(|c| c.is_ascii_hexdigit()).take(8).collect();
    let num = u32::from_str_radix(&hex_str, 16).unwrap_or(0);
    format!("{:06}", num % 1_000_000)
}

/// Constant-time comparison of two byte slices.
///
/// Returns true only if both slices have the same length and identical contents.
/// Iterates over all bytes regardless of mismatches to prevent timing side-channels.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_pairing_code_is_6_digits() {
        let guard = PairingGuard::new(true, &[]);
        let code = guard.pairing_code().expect("should have a code");
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_no_code_when_tokens_exist() {
        let guard = PairingGuard::new(true, &["existing-token".to_string()]);
        assert!(guard.pairing_code().is_none());
    }

    #[test]
    fn test_no_code_when_pairing_disabled() {
        let guard = PairingGuard::new(false, &[]);
        assert!(guard.pairing_code().is_none());
    }

    #[test]
    fn test_successful_pairing() {
        let guard = PairingGuard::new(true, &[]);
        let code = guard.pairing_code().unwrap();
        let result = guard.try_pair(&code);
        assert!(result.is_ok());
        let token = result.unwrap().expect("should return a token");
        assert!(!token.is_empty());
    }

    #[test]
    fn test_pair_then_authenticate() {
        let guard = PairingGuard::new(true, &[]);
        let code = guard.pairing_code().unwrap();
        let token = guard.try_pair(&code).unwrap().unwrap();
        assert!(guard.is_authenticated(&token));
    }

    #[test]
    fn test_wrong_token_not_authenticated() {
        let guard = PairingGuard::new(true, &[]);
        let code = guard.pairing_code().unwrap();
        let _token = guard.try_pair(&code).unwrap().unwrap();
        assert!(!guard.is_authenticated("wrong-token"));
    }

    #[test]
    fn test_incorrect_code_rejected() {
        let guard = PairingGuard::new(true, &[]);
        let result = guard.try_pair("000000");
        // Either Err(0) for a non-lockout rejection, or the code happened to be 000000 (unlikely)
        if guard.pairing_code().unwrap() != "000000" {
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), 0);
        }
    }

    #[test]
    fn test_lockout_after_max_attempts() {
        let guard = PairingGuard::with_lockout(true, &[], 3, Duration::from_secs(60));
        let code = guard.pairing_code().unwrap();
        let wrong = if code == "999999" { "000000" } else { "999999" };

        // First 2 attempts: rejected but not locked out
        for _ in 0..2 {
            let r = guard.try_pair(wrong);
            assert_eq!(r, Err(0));
        }

        // 3rd attempt triggers lockout
        let r = guard.try_pair(wrong);
        assert!(r.is_err());
        assert_eq!(r.unwrap_err(), 60); // lockout duration in seconds

        // Subsequent attempt while locked out
        let r = guard.try_pair(&code);
        assert!(r.is_err());
        assert!(r.unwrap_err() > 0);
    }

    #[test]
    fn test_pairing_disabled_bypasses_auth() {
        let guard = PairingGuard::new(false, &[]);
        assert!(guard.is_authenticated("any-token"));
        assert!(guard.is_authenticated(""));
        assert!(guard.is_authenticated("literally-anything"));
    }

    #[test]
    fn test_pairing_disabled_try_pair_returns_none() {
        let guard = PairingGuard::new(false, &[]);
        let result = guard.try_pair("anything");
        assert_eq!(result, Ok(None));
    }

    #[test]
    fn test_existing_token_authenticates() {
        let token = "my-secret-token";
        let guard = PairingGuard::new(true, &[token.to_string()]);
        assert!(guard.is_authenticated(token));
        assert!(!guard.is_authenticated("wrong-token"));
    }

    #[test]
    fn test_multiple_tokens() {
        let guard = PairingGuard::new(true, &[]);
        let code = guard.pairing_code().unwrap();

        let token1 = guard.try_pair(&code).unwrap().unwrap();
        let token2 = guard.try_pair(&code).unwrap().unwrap();

        assert!(guard.is_authenticated(&token1));
        assert!(guard.is_authenticated(&token2));
        assert!(!guard.is_authenticated("unknown"));
    }

    #[test]
    fn test_hash_token_deterministic() {
        let h1 = hash_token("test");
        let h2 = hash_token("test");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_constant_time_eq_same() {
        assert!(constant_time_eq(b"hello", b"hello"));
    }

    #[test]
    fn test_constant_time_eq_different() {
        assert!(!constant_time_eq(b"hello", b"world"));
    }

    #[test]
    fn test_constant_time_eq_different_lengths() {
        assert!(!constant_time_eq(b"short", b"longer"));
    }

    #[test]
    fn test_generate_pairing_code_format() {
        for _ in 0..100 {
            let code = generate_pairing_code();
            assert_eq!(code.len(), 6, "code '{}' is not 6 chars", code);
            assert!(code.chars().all(|c| c.is_ascii_digit()), "code '{}' has non-digits", code);
        }
    }
}
