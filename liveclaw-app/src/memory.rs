//! MemoryAdapter — bridges `adk-memory::MemoryStore` (full CRUD) to
//! `adk-core::Memory` (search-only).
//!
//! The Runner injects this into `InvocationContext.memory_service` so the agent
//! can search past memories during conversation.

use std::sync::Arc;

use async_trait::async_trait;

use adk_memory::{MemoryService, SearchRequest};

/// Bridges `adk-memory::MemoryService` to `adk-core::Memory` by delegating
/// `search()` to `MemoryService::search()` with a configurable result limit.
pub struct MemoryAdapter {
    store: Arc<dyn MemoryService>,
    recall_limit: usize,
}

impl MemoryAdapter {
    /// Create a new `MemoryAdapter` wrapping the given store.
    ///
    /// * `store` — any `MemoryService` implementation (e.g. `InMemoryMemoryService`)
    /// * `recall_limit` — maximum number of entries returned
    pub fn new(store: Arc<dyn MemoryService>, recall_limit: usize) -> Self {
        Self { store, recall_limit }
    }
}

#[async_trait]
impl adk_core::Memory for MemoryAdapter {
    async fn search(&self, query: &str) -> adk_core::Result<Vec<adk_core::MemoryEntry>> {
        let req = SearchRequest {
            query: query.to_string(),
            user_id: String::new(),
            app_name: String::new(),
        };
        let resp = self.store.search(req).await.map_err(|e| adk_core::AdkError::Agent(e.to_string()))?;
        Ok(resp
            .memories
            .into_iter()
            .take(self.recall_limit)
            .map(|e| adk_core::MemoryEntry {
                content: e.content,
                author: e.author,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adk_core::{Content, Memory};
    use std::sync::Mutex;

    // ---------------------------------------------------------------
    // Mock MemoryService for unit testing
    // ---------------------------------------------------------------

    #[derive(Debug, Clone)]
    struct SearchArgs {
        query: String,
    }

    struct MockMemoryService {
        entries: Vec<adk_memory::MemoryEntry>,
        last_search: Mutex<Option<SearchArgs>>,
    }

    impl MockMemoryService {
        fn new(entries: Vec<adk_memory::MemoryEntry>) -> Self {
            Self {
                entries,
                last_search: Mutex::new(None),
            }
        }

        fn last_search_args(&self) -> Option<SearchArgs> {
            self.last_search.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl MemoryService for MockMemoryService {
        async fn add_session(
            &self,
            _app_name: &str,
            _user_id: &str,
            _session_id: &str,
            _entries: Vec<adk_memory::MemoryEntry>,
        ) -> adk_core::Result<()> {
            Ok(())
        }

        async fn search(&self, req: SearchRequest) -> adk_core::Result<adk_memory::SearchResponse> {
            *self.last_search.lock().unwrap() = Some(SearchArgs {
                query: req.query.clone(),
            });
            Ok(adk_memory::SearchResponse {
                memories: self.entries.clone(),
            })
        }
    }

    fn make_entry(text: &str, author: &str) -> adk_memory::MemoryEntry {
        adk_memory::MemoryEntry {
            content: Content::new("user").with_text(text),
            author: author.to_string(),
            timestamp: chrono::Utc::now(),
        }
    }

    // ---------------------------------------------------------------
    // Unit tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn search_delegates_query_to_memory_service() {
        let store = Arc::new(MockMemoryService::new(vec![make_entry("hello world", "user")]));
        let adapter = MemoryAdapter::new(store.clone(), 5);

        let results = adapter.search("hello").await.unwrap();

        let args = store.last_search_args().unwrap();
        assert_eq!(args.query, "hello");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].author, "user");
    }

    #[tokio::test]
    async fn search_returns_empty_when_store_is_empty() {
        let store = Arc::new(MockMemoryService::new(vec![]));
        let adapter = MemoryAdapter::new(store, 10);

        let results = adapter.search("anything").await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn search_converts_multiple_entries() {
        let entries = vec![
            make_entry("first", "alice"),
            make_entry("second", "bob"),
            make_entry("third", "charlie"),
        ];
        let store = Arc::new(MockMemoryService::new(entries));
        let adapter = MemoryAdapter::new(store, 20);

        let results = adapter.search("query").await.unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].author, "alice");
        assert_eq!(results[1].author, "bob");
        assert_eq!(results[2].author, "charlie");
    }

    #[tokio::test]
    async fn recall_limit_caps_results() {
        let entries = vec![
            make_entry("a", "x"),
            make_entry("b", "y"),
            make_entry("c", "z"),
        ];
        let store = Arc::new(MockMemoryService::new(entries));
        let adapter = MemoryAdapter::new(store, 2);

        let results = adapter.search("test").await.unwrap();
        assert_eq!(results.len(), 2);
    }
}
