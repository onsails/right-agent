//! In-memory prefetch cache for Hindsight auto-recall results.
//!
//! Keyed by arbitrary string (worker uses `"{chat_id}:{thread_id}"`,
//! cron uses job_name). No TTL — entries are overwritten after each turn.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone, Default)]
pub struct PrefetchCache {
    inner: Arc<RwLock<HashMap<String, String>>>,
}

impl PrefetchCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn put(&self, key: &str, content: String) {
        self.inner.write().await.insert(key.to_owned(), content);
    }

    pub async fn get(&self, key: &str) -> Option<String> {
        self.inner.read().await.get(key).cloned()
    }

    pub async fn clear(&self) {
        self.inner.write().await.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn put_and_get() {
        let cache = PrefetchCache::new();
        cache.put("42:0", "recalled memory".into()).await;
        assert_eq!(cache.get("42:0").await.as_deref(), Some("recalled memory"));
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let cache = PrefetchCache::new();
        assert!(cache.get("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn clear_invalidates_all() {
        let cache = PrefetchCache::new();
        cache.put("a", "1".into()).await;
        cache.put("b", "2".into()).await;
        cache.clear().await;
        assert!(cache.get("a").await.is_none());
        assert!(cache.get("b").await.is_none());
    }

    #[tokio::test]
    async fn overwrite_entry() {
        let cache = PrefetchCache::new();
        cache.put("k", "old".into()).await;
        cache.put("k", "new".into()).await;
        assert_eq!(cache.get("k").await.as_deref(), Some("new"));
    }

    #[tokio::test]
    async fn concurrent_access() {
        let cache = PrefetchCache::new();
        let c1 = cache.clone();
        let c2 = cache.clone();
        let w = tokio::spawn(async move { c1.put("k", "val".into()).await });
        let r = tokio::spawn(async move {
            let _ = c2.get("k").await;
        });
        w.await.unwrap();
        r.await.unwrap();
        assert_eq!(cache.get("k").await.as_deref(), Some("val"));
    }
}
