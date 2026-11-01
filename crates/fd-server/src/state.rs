//! 会话与版本栈（内存态）。
//!
//! issue#31 D1 右栏：版本列表（v1/v2…）+ 回退。每次生成/迭代/还原压入一版，
//! 回退 = 把 current 指针拨回旧版（后续迭代从该版继续，产生 v(n+1)）。
//! 内存护栏：单会话版本数 ≤ MAX_VERSIONS，总会话数 ≤ MAX_SESSIONS（LRU 淘汰最旧）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

const MAX_VERSIONS: usize = 20;
const MAX_SESSIONS: usize = 50;

/// 单个版本条目。
#[derive(Debug, Clone, serde::Serialize)]
pub struct VersionEntry {
    pub version: u32,
    /// 版本来源标签：initial / 迭代指令 / restore hint。
    pub label: String,
    pub created_at: String,
    pub node_count: usize,
    /// 完整 PenDocument JSON（真相源，前端渲染/导出直接消费）。
    pub document: String,
}

/// 设计会话。
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub created_at: String,
    pub versions: Vec<VersionEntry>,
    /// current 指针（1-based 版本号）。
    pub current: u32,
    /// 最近访问时间戳（秒），LRU 淘汰依据。
    pub last_used: u64,
}

impl Session {
    pub fn current_doc(&self) -> Option<&VersionEntry> {
        self.versions.iter().find(|v| v.version == self.current)
    }
}

/// 全局会话存储。
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

    /// 生成短会话 id：`s-<unix16hex>-<seq>`。无 uuid 依赖，单调可读。
    fn next_id(&self) -> String {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        format!("s-{:x}-{seq}", ts)
    }

    fn now_iso() -> String {
        // 简易 RFC3339（本地时区偏移未知场景用 UTC Z 后缀足够展示用）。
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

    /// 新建会话并压入首版。
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

    /// 追加新版本。超 MAX_VERSIONS 淘汰最旧非当前版（保 current 语义稳定）。
    pub fn push_version(&self, id: &str, document: String, label: &str) -> anyhow::Result<u32> {
        let mut guard = self.inner.lock().unwrap();
        let sess = guard
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("session {id} 不存在（或已被回收）"))?;
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
            // 保留 current 指向的版本，淘汰最旧的其他版本。
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

    /// 回退：current 拨到 version（必须存在）。
    pub fn rollback(&self, id: &str, version: u32) -> anyhow::Result<()> {
        let mut guard = self.inner.lock().unwrap();
        let sess = guard
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("session {id} 不存在"))?;
        if !sess.versions.iter().any(|v| v.version == version) {
            anyhow::bail!("session {id} 无版本 v{version}");
        }
        sess.current = version;
        sess.last_used = unix_now();
        Ok(())
    }

    /// 列出会话 id（按 last_used 降序）。
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
        // LRU：淘汰 last_used 最旧者直至回到上限内。
        while map.len() >= MAX_SESSIONS {
            let oldest = map
                .values()
                .min_by_key(|s| s.last_used)
                .map(|s| s.id.clone());
            match oldest {
                Some(id) => {
                    let _ = map.remove(&id);
                    tracing::warn!(session = %id, "会话数达上限，LRU 淘汰最旧会话");
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

/// LRU 时戳：纳秒级。秒级粒度下同秒创建的会话 last_used 全同，
/// min_by_key 退化为任意淘汰，LRU 语义破坏——故用 as_nanos。
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// 统计 PenDocument JSON 节点数（解析失败回退 0，前端展示容错）。
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

/// Unix 天 → 公历（Howard Hinnant civil_from_days，无 chrono 依赖）。
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
        assert!(id.starts_with("s-"), "会话 id 前缀 s-，实际 {id}");
        let v2 = store.push_version(&id, doc(5), "加暗黑模式").unwrap();
        assert_eq!(v2, 2);
        let s = store.get(&id).unwrap();
        assert_eq!(s.current, 2);
        assert_eq!(s.versions.len(), 2);
        assert_eq!(s.versions[0].node_count, 3);
        assert_eq!(s.versions[1].node_count, 5);

        // 回退到 v1 后再迭代 → v3（当前链从 v1 分叉为新版本号）。
        store.rollback(&id, 1).unwrap();
        let s = store.get(&id).unwrap();
        assert_eq!(s.current, 1);
        let v3 = store.push_version(&id, doc(4), "换个方向").unwrap();
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
            "版本数超上限：{}",
            s.versions.len()
        );
        assert!(
            s.versions.iter().any(|v| v.version == s.current),
            "current 版本必须保留"
        );
        assert_eq!(s.current, MAX_VERSIONS as u32 + 5, "版本号单调递增不回收");
    }

    #[test]
    fn session_lru_eviction() {
        let store = SessionStore::new();
        let first = store.create(doc(1), "a");
        // 造满 + 多一个，first 应被 LRU 淘汰（first last_used 最旧）。
        for _ in 0..MAX_SESSIONS {
            store.create(doc(1), "x");
        }
        assert!(
            store.get(&first).is_none(),
            "最旧会话应被 LRU 淘汰（上限 {MAX_SESSIONS}）"
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
        // 2026-11-01 = 20758 天（自 1970-01-01）。
        assert_eq!(civil_from_days(20758), (2026, 11, 1));
    }
}
