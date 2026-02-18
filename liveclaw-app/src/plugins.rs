//! Plugin builders for LiveClaw.
//!
//! Assembles PII redaction, memory auto-save, and guardrail plugins into a
//! [`PluginManager`] that the Runner executes at the appropriate lifecycle
//! points.

use std::sync::Arc;

use adk_memory::{MemoryEntry, MemoryService};
use adk_plugin::{Plugin, PluginConfig, PluginManager};

// ---------------------------------------------------------------------------
// PII patterns
// ---------------------------------------------------------------------------

/// Returns compiled regex patterns for PII detection.
///
/// Patterns cover: email, phone, SSN, credit card, and IP address.
pub fn build_pii_patterns() -> Vec<regex::Regex> {
    vec![
        // Email
        regex::Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Z|a-z]{2,}\b").unwrap(),
        // Phone (e.g. 555-123-4567, 555.123.4567, 5551234567)
        regex::Regex::new(r"\b\d{3}[-.]?\d{3}[-.]?\d{4}\b").unwrap(),
        // SSN (e.g. 123-45-6789)
        regex::Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").unwrap(),
        // Credit card (e.g. 4111-1111-1111-1111, 4111 1111 1111 1111)
        regex::Regex::new(r"\b\d{4}[-\s]?\d{4}[-\s]?\d{4}[-\s]?\d{4}\b").unwrap(),
        // IP address
        regex::Regex::new(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b").unwrap(),
    ]
}

/// Apply PII redaction patterns to a text string, replacing matches with
/// `[REDACTED]`.
///
/// Exposed as a helper so tests (and the plugin closure) can call it directly.
pub fn redact_pii(text: &str, patterns: &[regex::Regex]) -> String {
    let mut redacted = text.to_string();
    for pattern in patterns {
        redacted = pattern.replace_all(&redacted, "[REDACTED]").to_string();
    }
    redacted
}

// ---------------------------------------------------------------------------
// Individual plugin builders
// ---------------------------------------------------------------------------

/// PII Redaction Plugin — intercepts events via `on_event` hook,
/// redacts PII from text content before the event is persisted.
pub fn build_pii_redaction_plugin(patterns: Vec<regex::Regex>) -> Plugin {
    Plugin::new(PluginConfig {
        name: "pii_redactor".to_string(),
        on_event: Some(Box::new(move |_ctx, event| {
            let patterns = patterns.clone();
            Box::pin(async move {
                if let Some(content) = event.content() {
                    let mut needs_redaction = false;
                    for part in &content.parts {
                        if let Some(text) = part.text() {
                            if patterns.iter().any(|p| p.is_match(text)) {
                                needs_redaction = true;
                                break;
                            }
                        }
                    }
                    if needs_redaction {
                        let mut new_event = event.clone();
                        let mut new_content = content.clone();
                        new_content.parts = content.parts.iter().map(|part| {
                            if let Some(text) = part.text() {
                                adk_core::Part::Text { text: redact_pii(text, &patterns) }
                            } else {
                                part.clone()
                            }
                        }).collect();
                        new_event.set_content(new_content);
                        return Ok(Some(new_event));
                    }
                }
                Ok(None) // Pass through unchanged
            })
        })),
        ..Default::default()
    })
}

/// Memory Auto-Save Plugin — stores conversation summary after session ends
/// via `after_run` hook.
pub fn build_memory_autosave_plugin(store: Arc<dyn MemoryService>) -> Plugin {
    Plugin::new(PluginConfig {
        name: "memory_autosave".to_string(),
        after_run: Some(Box::new(move |ctx| {
            let store = store.clone();
            Box::pin(async move {
                let summary = ctx.session_id().to_string();
                if !summary.is_empty() {
                    let entry = MemoryEntry {
                        content: adk_core::Content::new("assistant").with_text(&summary),
                        author: "system".to_string(),
                        timestamp: chrono::Utc::now(),
                    };
                    let _ = store.add_session("liveclaw", "system", &summary, vec![entry]).await;
                }
            })
        })),
        ..Default::default()
    })
}

/// Guardrail Plugin — validates user messages for harmful content via
/// `on_user_message` hook. Keyword matching is case-insensitive.
pub fn build_guardrail_plugin(blocked_keywords: Vec<String>) -> Plugin {
    Plugin::new(PluginConfig {
        name: "guardrail".to_string(),
        on_user_message: Some(Box::new(move |_ctx, message| {
            let keywords = blocked_keywords.clone();
            Box::pin(async move {
                let text: String = message.parts.iter()
                    .filter_map(|p| p.text())
                    .collect::<Vec<_>>()
                    .join(" ");
                for keyword in &keywords {
                    if text.to_lowercase().contains(&keyword.to_lowercase()) {
                        return Err(adk_core::AdkError::Agent(format!(
                            "Blocked content detected: {}",
                            keyword
                        )));
                    }
                }
                Ok(None)
            })
        })),
        ..Default::default()
    })
}

// ---------------------------------------------------------------------------
// Plugin manager assembly
// ---------------------------------------------------------------------------

/// Assembles all LiveClaw plugins into a [`PluginManager`].
pub fn build_plugin_manager(
    memory_store: Arc<dyn MemoryService>,
    pii_patterns: Vec<regex::Regex>,
    blocked_keywords: Vec<String>,
) -> PluginManager {
    let plugins = vec![
        build_pii_redaction_plugin(pii_patterns),
        build_memory_autosave_plugin(memory_store),
        build_guardrail_plugin(blocked_keywords),
    ];
    PluginManager::new(plugins)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // === build_pii_patterns tests ===

    #[test]
    fn pii_patterns_returns_five_patterns() {
        let patterns = build_pii_patterns();
        assert_eq!(patterns.len(), 5);
    }

    // === PII redaction tests ===

    #[test]
    fn redacts_email_address() {
        let patterns = build_pii_patterns();
        let input = "Contact me at alice@example.com for details";
        let result = redact_pii(input, &patterns);
        assert!(!result.contains("alice@example.com"));
        assert!(result.contains("[REDACTED]"));
    }

    #[test]
    fn redacts_phone_number_with_dashes() {
        let patterns = build_pii_patterns();
        let input = "Call me at 555-123-4567 please";
        let result = redact_pii(input, &patterns);
        assert!(!result.contains("555-123-4567"));
        assert!(result.contains("[REDACTED]"));
    }

    #[test]
    fn redacts_phone_number_with_dots() {
        let patterns = build_pii_patterns();
        let input = "My number is 555.123.4567";
        let result = redact_pii(input, &patterns);
        assert!(!result.contains("555.123.4567"));
        assert!(result.contains("[REDACTED]"));
    }

    #[test]
    fn redacts_phone_number_no_separators() {
        let patterns = build_pii_patterns();
        let input = "Phone: 5551234567";
        let result = redact_pii(input, &patterns);
        assert!(!result.contains("5551234567"));
        assert!(result.contains("[REDACTED]"));
    }

    #[test]
    fn redacts_ssn() {
        let patterns = build_pii_patterns();
        let input = "SSN is 123-45-6789";
        let result = redact_pii(input, &patterns);
        assert!(!result.contains("123-45-6789"));
        assert!(result.contains("[REDACTED]"));
    }

    #[test]
    fn redacts_credit_card_with_dashes() {
        let patterns = build_pii_patterns();
        let input = "Card: 4111-1111-1111-1111";
        let result = redact_pii(input, &patterns);
        assert!(!result.contains("4111-1111-1111-1111"));
        assert!(result.contains("[REDACTED]"));
    }

    #[test]
    fn redacts_credit_card_with_spaces() {
        let patterns = build_pii_patterns();
        let input = "Card: 4111 1111 1111 1111";
        let result = redact_pii(input, &patterns);
        assert!(!result.contains("4111 1111 1111 1111"));
        assert!(result.contains("[REDACTED]"));
    }

    #[test]
    fn redacts_credit_card_no_separators() {
        let patterns = build_pii_patterns();
        let input = "Card: 4111111111111111";
        let result = redact_pii(input, &patterns);
        assert!(!result.contains("4111111111111111"));
        assert!(result.contains("[REDACTED]"));
    }

    #[test]
    fn redacts_ip_address() {
        let patterns = build_pii_patterns();
        let input = "Server at 192.168.1.100";
        let result = redact_pii(input, &patterns);
        assert!(!result.contains("192.168.1.100"));
        assert!(result.contains("[REDACTED]"));
    }

    #[test]
    fn clean_text_passes_through_unchanged() {
        let patterns = build_pii_patterns();
        let input = "Hello, how are you today?";
        let result = redact_pii(input, &patterns);
        assert_eq!(result, input);
    }

    #[test]
    fn redacts_multiple_pii_in_same_text() {
        let patterns = build_pii_patterns();
        let input = "Email alice@example.com, phone 555-123-4567, IP 10.0.0.1";
        let result = redact_pii(input, &patterns);
        assert!(!result.contains("alice@example.com"));
        assert!(!result.contains("555-123-4567"));
        assert!(!result.contains("10.0.0.1"));
        assert_eq!(result.matches("[REDACTED]").count(), 3);
    }

    // === Guardrail keyword tests ===

    /// Helper: check if a message would be blocked by the guardrail.
    fn is_blocked(keywords: &[&str], message: &str) -> bool {
        let lower_msg = message.to_lowercase();
        keywords
            .iter()
            .any(|kw| lower_msg.contains(&kw.to_lowercase()))
    }

    #[test]
    fn guardrail_blocks_exact_keyword() {
        assert!(is_blocked(&["hack"], "I want to hack the system"));
    }

    #[test]
    fn guardrail_blocks_case_insensitive() {
        assert!(is_blocked(&["hack"], "I want to HACK the system"));
        assert!(is_blocked(&["HACK"], "let me hack this"));
    }

    #[test]
    fn guardrail_blocks_keyword_as_substring() {
        assert!(is_blocked(&["hack"], "hacking is fun"));
    }

    #[test]
    fn guardrail_passes_clean_message() {
        assert!(!is_blocked(&["hack", "exploit"], "Hello, how are you?"));
    }

    #[test]
    fn guardrail_empty_keywords_passes_everything() {
        assert!(!is_blocked(&[], "hack exploit anything"));
    }

    #[test]
    fn guardrail_empty_message_passes() {
        assert!(!is_blocked(&["hack"], ""));
    }
}
