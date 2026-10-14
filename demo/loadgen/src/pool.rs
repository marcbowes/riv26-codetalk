//! Real ids collected from the API, shared by every request task.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, RwLock};

pub struct IdPool {
    users: Vec<Arc<str>>,
    posts: RwLock<Vec<Arc<str>>>,
    seen: Mutex<HashSet<Arc<str>>>,
    cap: usize,
}

impl IdPool {
    pub fn new(users: Vec<String>, posts: Vec<String>, cap: usize) -> Self {
        let pool = Self {
            users: users.into_iter().map(Arc::from).collect(),
            posts: RwLock::new(Vec::with_capacity(posts.len().min(cap))),
            seen: Mutex::new(HashSet::new()),
            cap: cap.max(1),
        };
        pool.add_posts(posts);
        pool
    }

    pub fn random_user(&self) -> Option<&str> {
        if self.users.is_empty() {
            None
        } else {
            Some(&self.users[fastrand::usize(..self.users.len())])
        }
    }

    pub fn random_post(&self) -> Option<Arc<str>> {
        let posts = self.posts.read().unwrap_or_else(|e| e.into_inner());
        if posts.is_empty() {
            None
        } else {
            Some(posts[fastrand::usize(..posts.len())].clone())
        }
    }

    pub fn add_post(&self, id: String) -> bool {
        self.add_posts([id]) == 1
    }

    /// Add new ids; once full, new ids replace random old ones so fresh posts
    /// keep getting read. Returns how many were new.
    pub fn add_posts(&self, ids: impl IntoIterator<Item = String>) -> usize {
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        let mut fresh: Vec<Arc<str>> = Vec::new();
        for id in ids {
            if id.is_empty() || seen.contains(id.as_str()) {
                continue;
            }
            let id: Arc<str> = Arc::from(id);
            seen.insert(id.clone());
            fresh.push(id);
        }
        if fresh.is_empty() {
            return 0;
        }
        let added = fresh.len();
        let mut posts = self.posts.write().unwrap_or_else(|e| e.into_inner());
        for id in fresh {
            if posts.len() < self.cap {
                posts.push(id);
            } else {
                let slot = fastrand::usize(..posts.len());
                let old = std::mem::replace(&mut posts[slot], id);
                seen.remove(&old);
            }
        }
        added
    }

    pub fn post_count(&self) -> usize {
        self.posts.read().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn user_count(&self) -> usize {
        self.users.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(range: std::ops::Range<usize>) -> Vec<String> {
        range.map(|i| format!("p{i}")).collect()
    }

    #[test]
    fn dedupes_and_caps() {
        let pool = IdPool::new(vec!["u1".into()], ids(0..10), 15);
        assert_eq!(pool.post_count(), 10);
        assert_eq!(pool.add_posts(ids(5..12)), 2);
        assert_eq!(pool.post_count(), 12);
        assert!(!pool.add_post("p3".into()));
        // Past the cap, new ids replace old ones.
        assert_eq!(pool.add_posts(ids(100..110)), 10);
        assert_eq!(pool.post_count(), 15);
        assert_eq!(pool.random_user(), Some("u1"));
    }

    #[test]
    fn empty_pool_returns_none() {
        let pool = IdPool::new(vec![], vec![], 10);
        assert!(pool.random_post().is_none());
        assert!(pool.random_user().is_none());
    }
}
