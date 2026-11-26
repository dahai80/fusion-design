//! Sessions and version stack (in-memory).
//!
//! issue#31 D1 right pane: version list (v1/v2…) + rollback. Each
//! generate/iterate/restore pushes a version; rollback = moving the current
//! pointer back to an old version (subsequent iterations continue from that
//! version, producing v(n+1)).
//! Memory guardrails: versions per session ≤ MAX_VERSIONS, total sessions ≤
//! MAX_SESSIONS (LRU evicts oldest).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

const MAX_VERSIONS: usize = 20;
const MAX_SESSIONS: usize = 50;

/// A single version entry.
#[derive(Debug, Clone, serde::Serialize)]
pub struct VersionEntry {
    pub version: u32,
    /// Version origin label: initial / iteration instruction / restore hint.
    pub label: String,
    pub created_at: String,
    pub node_count: usize,
    /// Full PenDocument JSON (source of truth, consumed directly by frontend
    /// rendering/export).
    pub document: String,
}

/// A design session.
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub created_at: String,
    pub versions: Vec<VersionEntry>,
    /// current pointer (1-based version number).
    pub current: u32,
    /// Last-access timestamp (nanoseconds), basis for LRU eviction.
    pub last_used: u64,
}

impl Session {
    pub fn current_doc(&self) -> Option<&VersionEntry> {
        self.versions.iter().find(|v| v.version == self.current)
    }
}

/// Global session store.
pub struct SessionStore {
    inner: Mutex<HashMap<String, Session>>,
    seq: AtomicU64,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            seq: AtomicU64::new(1),
        }
    }

    /// Generates a short session id: `s-<unix16hex>-<seq>`. No uuid dependency,
    /// monotonic and readable.
    fn next_id(&self) -> String {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        format!("s-{:x}-{seq}", ts)
    }

    fn now_iso() -> String {
        // Simple RFC3339 (UTC Z suffix is enough for display when the local
        // timezone offset is unknown).
        let d = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let secs = d.as_secs();
        let days = secs / 86400;
        let (y, mo, da) = civil_from_days(days as i64);
        let rem = secs % 86400;
        format!(
            "{y:04}-{mo:02}-{da:02}T{:02}:{:02}:{:02}Z",
            rem / 3600,
            (rem % 3600) / 60,
            rem % 60
        )
    }

    /// Creates a session and pushes the first version.
    pub fn create(&self, document: String, label: &str) -> String {
        let id = self.next_id();
        let node_count = count_nodes(&document);
        let entry = VersionEntry {
            version: 1,
            label: label.to_string(),
            created_at: Self::now_iso(),
            node_count,
            document,
        };
        let mut guard = self.inner.lock().unwrap();
        Self::evict_if_full(&mut guard);
        guard.insert(
            id.clone(),
            Session {
                id: id.clone(),
                created_at: entry.created_at.clone(),
                versions: vec![entry],
                current: 1,
                last_used: unix_now(),
            },
        );
        id
    }

    /// Appends a new version. Beyond MAX_VERSIONS, evicts the oldest
    /// non-current version (keeps current semantics stable).
    pub fn push_version(&self, id: &str, document: String, label: &str) -> anyhow::Result<u32> {
        let mut guard = self.inner.lock().unwrap();
        let sess = guard
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("session {id} does not exist (or was evicted)"))?;
        sess.last_used = unix_now();
        let next = sess.versions.last().map(|v| v.version).unwrap_or(0) + 1;
        let entry = VersionEntry {
            version: next,
            label: label.to_string(),
            created_at: Self::now_iso(),
            node_count: count_nodes(&document),
            document,
        };
        sess.versions.push(entry);
        while sess.versions.len() > MAX_VERSIONS {
            // Keep the version current points to; evict the oldest other versions.
            let cur = sess.current;
            if let Some(pos) = sess.versions.iter().position(|v| v.version != cur) {
                sess.versions.remove(pos);
            } else {
                break;
            }
        }
        sess.current = next;
        Ok(next)
    }

    pub fn get(&self, id: &str) -> Option<Session> {
        let mut guard = self.inner.lock().unwrap();
        let sess = guard.get_mut(id)?;
        sess.last_used = unix_now();
        Some(sess.clone())
    }

    /// Rollback: move current to version (must exist).
    pub fn rollback(&self, id: &str, version: u32) -> anyhow::Result<()> {
        let mut guard = self.inner.lock().unwrap();
        let sess = guard
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("session {id} does not exist"))?;
        if !sess.versions.iter().any(|v| v.version == version) {
            anyhow::bail!("session {id} has no version v{version}");
        }
        sess.current = version;
        sess.last_used = unix_now();
        Ok(())
    }

    /// Lists session ids (descending by last_used).
    pub fn list_ids(&self) -> Vec<String> {
        let guard = self.inner.lock().unwrap();
        let mut v: Vec<&Session> = guard.values().collect();
        v.sort_by(|a, b| b.last_used.cmp(&a.last_used));
        v.into_iter().map(|s| s.id.clone()).collect()
    }

    fn evict_if_full(map: &mut HashMap<String, Session>) {
        if map.len() < MAX_SESSIONS {
            return;
        }
        // LRU: evict the oldest last_used until back within the cap.
        while map.len() >= MAX_SESSIONS {
            let oldest = map
                .values()
                .min_by_key(|s| s.last_used)
                .map(|s| s.id.clone());
            match oldest {
                Some(id) => {
                    let _ = map.remove(&id);
                    tracing::warn!(session = %id, "session count at cap, LRU-evicting oldest session");
                }
                None => break,
            }
        }
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

/// LRU timestamp: nanosecond precision. With second-level granularity, sessions
/// created in the same second share identical last_used and min_by_key degrades
/// to arbitrary eviction, breaking LRU semantics — hence as_nanos.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Counts nodes in a PenDocument JSON (falls back to 0 on parse failure,
/// tolerated for frontend display).
fn count_nodes(document_json: &str) -> usize {
    serde_json::from_str::<serde_json::Value>(document_json)
        .ok()
        .and_then(|v| {
            v.get("pages").and_then(|p| p.as_array()).map(|pages| {
                pages
                    .iter()
                    .filter_map(|p| p.get("nodes").and_then(|n| n.as_array()))
                    .map(|n| n.len())
                    .sum()
            })
        })
        .unwrap_or(0)
}

/// Unix days -> Gregorian date (Howard Hinnant civil_from_days, no chrono dependency).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(n: usize) -> String {
        serde_json::json!({
            "pages": [{
                "id": "p1", "name": "page", "width": 1440.0, "height": 900.0,
                "nodes": (0..n).map(|i| serde_json::json!({
                    "id": format!("n{i}"), "kind": "rect",
                    "x": 0.0, "y": 0.0, "w": 10.0, "h": 10.0
                })).collect::<Vec<_>>()
            }]
        })
        .to_string()
    }

    #[test]
    fn create_push_rollback_flow() {
        let store = SessionStore::new();
        let id = store.create(doc(3), "initial");
        assert!(id.starts_with("s-"), "session id prefix s-, got {id}");
        let v2 = store.push_version(&id, doc(5), "add dark mode").unwrap();
        assert_eq!(v2, 2);
        let s = store.get(&id).unwrap();
        assert_eq!(s.current, 2);
        assert_eq!(s.versions.len(), 2);
        assert_eq!(s.versions[0].node_count, 3);
        assert_eq!(s.versions[1].node_count, 5);

        // Rollback to v1 then iterate -> v3 (current chain forks from v1 into a new version number).
        store.rollback(&id, 1).unwrap();
        let s = store.get(&id).unwrap();
        assert_eq!(s.current, 1);
        let v3 = store.push_version(&id, doc(4), "try another direction").unwrap();
        assert_eq!(v3, 3);
        let s = store.get(&id).unwrap();
        assert_eq!(s.current, 3);
        assert_eq!(s.versions.len(), 3);
    }

    #[test]
    fn rollback_unknown_version_fails() {
        let store = SessionStore::new();
        let id = store.create(doc(1), "initial");
        assert!(store.rollback(&id, 99).is_err());
        assert!(store.rollback("s-none", 1).is_err());
    }

    #[test]
    fn version_cap_evicts_oldest_keeps_current() {
        let store = SessionStore::new();
        let id = store.create(doc(1), "initial");
        for i in 2..=(MAX_VERSIONS as u32 + 5) {
            store
                .push_version(&id, doc(1), &format!("iter-{i}"))
                .unwrap();
        }
        let s = store.get(&id).unwrap();
        assert!(
            s.versions.len() <= MAX_VERSIONS,
            "version count over cap: {}",
            s.versions.len()
        );
        assert!(
            s.versions.iter().any(|v| v.version == s.current),
            "current version must be retained"
        );
        assert_eq!(
            s.current,
            MAX_VERSIONS as u32 + 5,
            "version numbers grow monotonically, never recycled"
        );
    }

    #[test]
    fn session_lru_eviction() {
        let store = SessionStore::new();
        let first = store.create(doc(1), "a");
        // Fill to cap + one more; first should be LRU-evicted (oldest last_used).
        for _ in 0..MAX_SESSIONS {
            store.create(doc(1), "x");
        }
        assert!(
            store.get(&first).is_none(),
            "oldest session should be LRU-evicted (cap {MAX_SESSIONS})"
        );
        assert!(store.list_ids().len() <= MAX_SESSIONS);
    }

    #[test]
    fn count_nodes_tolerates_bad_json() {
        assert_eq!(count_nodes("not json"), 0);
        assert_eq!(count_nodes(&doc(7)), 7);
    }

    #[test]
    fn civil_from_days_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2026-11-01 = 20758 days (since 1970-01-01).
        assert_eq!(civil_from_days(20758), (2026, 11, 1));
    }
}
