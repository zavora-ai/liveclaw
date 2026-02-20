//! Storage services used by LiveClaw runtime wiring.
//!
//! This module provides:
//! - file-backed `MemoryService` for durable memory recall across restarts
//! - file-backed `ArtifactService` for transcript/audio artifact persistence

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use adk_artifact::{
    ArtifactService, DeleteRequest, ListRequest, ListResponse, LoadRequest, LoadResponse,
    SaveRequest, SaveResponse, VersionsRequest, VersionsResponse,
};
use adk_core::{Content, Part};
use adk_memory::{MemoryEntry, MemoryService, SearchRequest, SearchResponse};
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct MemoryKey {
    app_name: String,
    user_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedMemoryEntry {
    content: Content,
    author: String,
    timestamp_millis: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedSessionMemory {
    app_name: String,
    user_id: String,
    session_id: String,
    entries: Vec<PersistedMemoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PersistedMemoryFile {
    sessions: Vec<PersistedSessionMemory>,
}

type MemoryStore = HashMap<MemoryKey, HashMap<String, Vec<PersistedMemoryEntry>>>;

/// File-backed memory service with simple token-overlap search.
pub struct FileMemoryService {
    path: PathBuf,
    store: Arc<RwLock<MemoryStore>>,
}

impl FileMemoryService {
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let store = Self::load_store(&path)?;
        Ok(Self {
            path,
            store: Arc::new(RwLock::new(store)),
        })
    }

    fn load_store(path: &Path) -> Result<MemoryStore> {
        if !path.exists() {
            return Ok(HashMap::new());
        }

        let bytes = fs::read(path)
            .with_context(|| format!("Failed to read memory store file '{}'", path.display()))?;
        if bytes.is_empty() {
            return Ok(HashMap::new());
        }

        let persisted: PersistedMemoryFile = serde_json::from_slice(&bytes)
            .with_context(|| format!("Failed to parse memory store '{}'", path.display()))?;

        let mut store: MemoryStore = HashMap::new();
        for session in persisted.sessions {
            let key = MemoryKey {
                app_name: session.app_name,
                user_id: session.user_id,
            };
            store
                .entry(key)
                .or_default()
                .insert(session.session_id, session.entries);
        }
        Ok(store)
    }

    fn persist_store(&self, store: &MemoryStore) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create memory store directory '{}'",
                    parent.display()
                )
            })?;
        }

        let mut sessions = Vec::new();
        for (key, by_session) in store {
            for (session_id, entries) in by_session {
                sessions.push(PersistedSessionMemory {
                    app_name: key.app_name.clone(),
                    user_id: key.user_id.clone(),
                    session_id: session_id.clone(),
                    entries: entries.clone(),
                });
            }
        }

        let serialized = serde_json::to_vec_pretty(&PersistedMemoryFile { sessions })
            .context("Failed to serialize memory store")?;

        let tmp_path = self.path.with_extension("tmp");
        fs::write(&tmp_path, serialized).with_context(|| {
            format!(
                "Failed to write temporary memory store '{}'",
                tmp_path.display()
            )
        })?;
        fs::rename(&tmp_path, &self.path).with_context(|| {
            format!(
                "Failed to atomically replace memory store '{}'",
                self.path.display()
            )
        })?;
        Ok(())
    }

    fn extract_words(text: &str) -> HashSet<String> {
        text.split_whitespace()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_lowercase())
            .collect()
    }

    fn extract_words_from_content(content: &Content) -> HashSet<String> {
        let mut words = HashSet::new();
        for part in &content.parts {
            if let Part::Text { text } = part {
                words.extend(Self::extract_words(text));
            }
        }
        words
    }
}

#[async_trait]
impl MemoryService for FileMemoryService {
    async fn add_session(
        &self,
        app_name: &str,
        user_id: &str,
        session_id: &str,
        entries: Vec<MemoryEntry>,
    ) -> adk_core::Result<()> {
        let persisted_entries = entries
            .into_iter()
            .map(|entry| PersistedMemoryEntry {
                content: entry.content,
                author: entry.author,
                timestamp_millis: entry.timestamp.timestamp_millis(),
            })
            .collect::<Vec<_>>();

        let mut guard = self
            .store
            .write()
            .map_err(|_| adk_core::AdkError::Agent("memory store lock poisoned".to_string()))?;
        let key = MemoryKey {
            app_name: app_name.to_string(),
            user_id: user_id.to_string(),
        };
        guard
            .entry(key)
            .or_default()
            .insert(session_id.to_string(), persisted_entries);

        self.persist_store(&guard)
            .map_err(|e| adk_core::AdkError::Agent(e.to_string()))?;
        Ok(())
    }

    async fn search(&self, req: SearchRequest) -> adk_core::Result<SearchResponse> {
        let query_words = Self::extract_words(&req.query);
        if query_words.is_empty() {
            return Ok(SearchResponse {
                memories: Vec::new(),
            });
        }

        let key = MemoryKey {
            app_name: req.app_name,
            user_id: req.user_id,
        };

        let guard = self
            .store
            .read()
            .map_err(|_| adk_core::AdkError::Agent("memory store lock poisoned".to_string()))?;
        let sessions = match guard.get(&key) {
            Some(value) => value,
            None => {
                return Ok(SearchResponse {
                    memories: Vec::new(),
                });
            }
        };

        let mut memories = Vec::new();
        for entries in sessions.values() {
            for entry in entries {
                let words = Self::extract_words_from_content(&entry.content);
                if query_words.iter().any(|word| words.contains(word)) {
                    let timestamp = Utc
                        .timestamp_millis_opt(entry.timestamp_millis)
                        .single()
                        .unwrap_or_else(Utc::now);
                    memories.push(MemoryEntry {
                        content: entry.content.clone(),
                        author: entry.author.clone(),
                        timestamp,
                    });
                }
            }
        }

        Ok(SearchResponse { memories })
    }
}

fn sanitize_component(input: &str) -> String {
    input
        .replace(['/', '\\', ':'], "_")
        .replace("..", "_")
        .trim()
        .to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedArtifactPart {
    part: Part,
}

/// File-backed artifact service implementing adk-artifact contracts.
pub struct FileArtifactService {
    root: PathBuf,
}

impl FileArtifactService {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root).with_context(|| {
            format!(
                "Failed to create artifact storage root '{}'",
                root.display()
            )
        })?;
        Ok(Self { root })
    }

    fn session_dir(&self, app_name: &str, user_id: &str, session_id: &str) -> PathBuf {
        self.root
            .join(sanitize_component(app_name))
            .join(sanitize_component(user_id))
            .join(sanitize_component(session_id))
    }

    fn file_dir(&self, req: &LoadRequest) -> PathBuf {
        self.session_dir(&req.app_name, &req.user_id, &req.session_id)
            .join(sanitize_component(&req.file_name))
    }

    fn versions_for_path(path: &Path) -> adk_core::Result<Vec<i64>> {
        if !path.exists() {
            return Ok(Vec::new());
        }

        let mut versions = Vec::new();
        let entries = fs::read_dir(path).map_err(|e| adk_core::AdkError::Agent(e.to_string()))?;
        for entry in entries {
            let entry = entry.map_err(|e| adk_core::AdkError::Agent(e.to_string()))?;
            if !entry
                .file_type()
                .map_err(|e| adk_core::AdkError::Agent(e.to_string()))?
                .is_file()
            {
                continue;
            }
            if let Some(stem) = entry.path().file_stem().and_then(|s| s.to_str()) {
                if let Ok(version) = stem.parse::<i64>() {
                    versions.push(version);
                }
            }
        }
        versions.sort_unstable();
        Ok(versions)
    }
}

#[async_trait]
impl ArtifactService for FileArtifactService {
    async fn save(&self, req: SaveRequest) -> adk_core::Result<SaveResponse> {
        let load_req = LoadRequest {
            app_name: req.app_name.clone(),
            user_id: req.user_id.clone(),
            session_id: req.session_id.clone(),
            file_name: req.file_name.clone(),
            version: req.version,
        };
        let file_dir = self.file_dir(&load_req);
        fs::create_dir_all(&file_dir).map_err(|e| adk_core::AdkError::Agent(e.to_string()))?;

        let versions = Self::versions_for_path(&file_dir)?;
        let version = req
            .version
            .unwrap_or_else(|| versions.last().copied().unwrap_or(0) + 1);

        let file_path = file_dir.join(format!("{}.json", version));
        let payload = PersistedArtifactPart { part: req.part };
        let serialized = serde_json::to_vec_pretty(&payload)
            .map_err(|e| adk_core::AdkError::Agent(e.to_string()))?;
        fs::write(&file_path, serialized).map_err(|e| adk_core::AdkError::Agent(e.to_string()))?;

        Ok(SaveResponse { version })
    }

    async fn load(&self, req: LoadRequest) -> adk_core::Result<LoadResponse> {
        let file_dir = self.file_dir(&req);
        let versions = Self::versions_for_path(&file_dir)?;
        let version = match req.version {
            Some(v) => v,
            None => versions
                .last()
                .copied()
                .ok_or_else(|| adk_core::AdkError::Agent("artifact not found".to_string()))?,
        };

        let file_path = file_dir.join(format!("{}.json", version));
        let bytes = fs::read(&file_path).map_err(|e| adk_core::AdkError::Agent(e.to_string()))?;
        let payload: PersistedArtifactPart =
            serde_json::from_slice(&bytes).map_err(|e| adk_core::AdkError::Agent(e.to_string()))?;
        Ok(LoadResponse { part: payload.part })
    }

    async fn delete(&self, req: DeleteRequest) -> adk_core::Result<()> {
        let load_req = LoadRequest {
            app_name: req.app_name,
            user_id: req.user_id,
            session_id: req.session_id,
            file_name: req.file_name,
            version: req.version,
        };
        let file_dir = self.file_dir(&load_req);

        if let Some(version) = load_req.version {
            let file_path = file_dir.join(format!("{}.json", version));
            if file_path.exists() {
                fs::remove_file(file_path).map_err(|e| adk_core::AdkError::Agent(e.to_string()))?;
            }
            return Ok(());
        }

        if file_dir.exists() {
            fs::remove_dir_all(file_dir).map_err(|e| adk_core::AdkError::Agent(e.to_string()))?;
        }
        Ok(())
    }

    async fn list(&self, req: ListRequest) -> adk_core::Result<ListResponse> {
        let session_dir = self.session_dir(&req.app_name, &req.user_id, &req.session_id);
        if !session_dir.exists() {
            return Ok(ListResponse {
                file_names: Vec::new(),
            });
        }

        let mut file_names = Vec::new();
        let entries =
            fs::read_dir(session_dir).map_err(|e| adk_core::AdkError::Agent(e.to_string()))?;
        for entry in entries {
            let entry = entry.map_err(|e| adk_core::AdkError::Agent(e.to_string()))?;
            if entry
                .file_type()
                .map_err(|e| adk_core::AdkError::Agent(e.to_string()))?
                .is_dir()
            {
                if let Some(name) = entry.file_name().to_str() {
                    file_names.push(name.to_string());
                }
            }
        }
        file_names.sort();
        Ok(ListResponse { file_names })
    }

    async fn versions(&self, req: VersionsRequest) -> adk_core::Result<VersionsResponse> {
        let load_req = LoadRequest {
            app_name: req.app_name,
            user_id: req.user_id,
            session_id: req.session_id,
            file_name: req.file_name,
            version: None,
        };
        let file_dir = self.file_dir(&load_req);
        let versions = Self::versions_for_path(&file_dir)?;
        Ok(VersionsResponse { versions })
    }
}

/// Build a memory service from a backend string.
///
/// Supported values:
/// - `in_memory`
/// - `file` (uses `./memory_store.json`)
/// - `file:/path/to/store.json`
pub fn build_memory_service(backend: &str) -> Result<Arc<dyn MemoryService>> {
    let normalized = backend.trim();
    if normalized.is_empty() || normalized.eq_ignore_ascii_case("in_memory") {
        return Ok(Arc::new(adk_memory::InMemoryMemoryService::new()));
    }

    if normalized.eq_ignore_ascii_case("file") || normalized.eq_ignore_ascii_case("json_file") {
        return Ok(Arc::new(FileMemoryService::new("./memory_store.json")?));
    }

    if let Some(rest) = normalized.strip_prefix("file:") {
        if rest.trim().is_empty() {
            bail!("Memory backend 'file:' requires a path");
        }
        return Ok(Arc::new(FileMemoryService::new(rest.trim())?));
    }

    bail!(
        "Unsupported memory backend '{}'. Use in_memory, file, or file:/path/to/store.json",
        backend
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(prefix: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "liveclaw-{}-{}-{}",
            prefix,
            std::process::id(),
            nanos
        ))
    }

    #[tokio::test]
    async fn file_memory_persists_across_restarts() {
        let path = temp_path("memory").with_extension("json");

        let service_a = FileMemoryService::new(&path).unwrap();
        service_a
            .add_session(
                "liveclaw",
                "user-1",
                "session-a",
                vec![MemoryEntry {
                    content: Content::new("assistant")
                        .with_text("favorite_language is rust and coffee is black"),
                    author: "assistant".to_string(),
                    timestamp: Utc::now(),
                }],
            )
            .await
            .unwrap();

        let service_b = FileMemoryService::new(&path).unwrap();
        let results = service_b
            .search(SearchRequest {
                query: "favorite_language".to_string(),
                user_id: "user-1".to_string(),
                app_name: "liveclaw".to_string(),
            })
            .await
            .unwrap();

        assert_eq!(results.memories.len(), 1);
        let text = results.memories[0]
            .content
            .parts
            .first()
            .and_then(Part::text)
            .unwrap_or_default();
        assert!(text.contains("favorite_language"));

        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn file_artifact_round_trip_persists_across_restarts() {
        let root = temp_path("artifact");
        let service_a = FileArtifactService::new(&root).unwrap();

        service_a
            .save(SaveRequest {
                app_name: "liveclaw".to_string(),
                user_id: "user-1".to_string(),
                session_id: "session-a".to_string(),
                file_name: "transcript-000001.txt".to_string(),
                part: Part::Text {
                    text: "hello transcript".to_string(),
                },
                version: None,
            })
            .await
            .unwrap();

        let service_b = FileArtifactService::new(&root).unwrap();
        let listed = service_b
            .list(ListRequest {
                app_name: "liveclaw".to_string(),
                user_id: "user-1".to_string(),
                session_id: "session-a".to_string(),
            })
            .await
            .unwrap();
        assert_eq!(listed.file_names, vec!["transcript-000001.txt".to_string()]);

        let loaded = service_b
            .load(LoadRequest {
                app_name: "liveclaw".to_string(),
                user_id: "user-1".to_string(),
                session_id: "session-a".to_string(),
                file_name: "transcript-000001.txt".to_string(),
                version: None,
            })
            .await
            .unwrap();
        match loaded.part {
            Part::Text { text } => assert_eq!(text, "hello transcript"),
            other => panic!("Unexpected artifact part: {:?}", other),
        }

        let _ = fs::remove_dir_all(root);
    }
}
