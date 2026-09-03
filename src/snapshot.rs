//! Snapshot persistence: MessagePack + zstd files under the local app-data
//! directory, a small "recent roots" index, and snapshot diffing.

use crate::model::{NodeId, Tree};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

pub const VERSION: u32 = 1;
const MAX_RECENT: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub root: PathBuf,
    pub created_at: i64,
    pub validated_at: Option<i64>,
    pub tree: Tree,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentEntry {
    pub root: PathBuf,
    pub created_at: i64,
    pub file_count: u64,
    pub total_size: u64,
}

impl From<&Snapshot> for RecentEntry {
    fn from(s: &Snapshot) -> Self {
        RecentEntry {
            root: s.root.clone(),
            created_at: s.validated_at.unwrap_or(s.created_at),
            file_count: s.tree.file_count,
            total_size: s.tree.total_size(),
        }
    }
}

pub fn storage_dir() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "weight-folders")?;
    let dir = dirs.data_local_dir().join("snapshots");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Case-insensitive, separator-normalized key for a root path.
pub fn normalize_root(p: &Path) -> String {
    let s = p.to_string_lossy().replace('/', "\\").to_lowercase();
    let trimmed = s.trim_end_matches('\\');
    if trimmed.is_empty() {
        s
    } else {
        trimmed.to_string()
    }
}

pub fn snapshot_path(root: &Path) -> Option<PathBuf> {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    normalize_root(root).hash(&mut h);
    Some(storage_dir()?.join(format!("{:016x}.snap", h.finish())))
}

pub fn exists(root: &Path) -> bool {
    snapshot_path(root).map(|p| p.is_file()).unwrap_or(false)
}

pub fn encode(snap: &Snapshot) -> Result<Vec<u8>, String> {
    let raw = rmp_serde::to_vec(snap).map_err(|e| e.to_string())?;
    zstd::encode_all(raw.as_slice(), 3).map_err(|e| e.to_string())
}

pub fn decode(bytes: &[u8]) -> Result<Snapshot, String> {
    let raw = zstd::decode_all(bytes).map_err(|e| e.to_string())?;
    let snap: Snapshot = rmp_serde::from_slice(&raw).map_err(|e| e.to_string())?;
    if snap.version != VERSION {
        return Err(format!("snapshot version {} not supported", snap.version));
    }
    Ok(snap)
}

pub fn load(root: &Path) -> Option<Snapshot> {
    let path = snapshot_path(root)?;
    let bytes = std::fs::read(path).ok()?;
    match decode(&bytes) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("ignoring snapshot for {}: {e}", root.display());
            None
        }
    }
}

pub fn save(snap: &Snapshot) -> Result<(), String> {
    let path = snapshot_path(&snap.root).ok_or("no storage directory")?;
    let bytes = encode(snap)?;
    let tmp = path.with_extension("snap.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

fn recent_path() -> Option<PathBuf> {
    Some(storage_dir()?.join("recent.json"))
}

pub fn load_recent() -> Vec<RecentEntry> {
    let Some(p) = recent_path() else { return vec![] };
    std::fs::read(p)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn touch_recent(entry: RecentEntry) {
    let key = normalize_root(&entry.root);
    let mut list = load_recent();
    list.retain(|e| normalize_root(&e.root) != key);
    list.insert(0, entry);
    list.truncate(MAX_RECENT);
    if let Some(p) = recent_path() {
        if let Ok(json) = serde_json::to_vec_pretty(&list) {
            let _ = std::fs::write(p, json);
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffSummary {
    pub added: u64,
    pub removed: u64,
    pub changed: u64,
    pub delta_bytes: i64,
}

impl DiffSummary {
    pub fn is_empty(&self) -> bool {
        self.added == 0 && self.removed == 0 && self.changed == 0
    }
}

fn subtree_items(tree: &Tree, id: NodeId) -> u64 {
    let (f, d) = tree.count_items(id);
    f + d + 1
}

/// Compare two trees of the same root. Counts files and directories that were
/// added, removed, or changed (size or mtime).
pub fn diff_summary(old: &Tree, new: &Tree) -> DiffSummary {
    let mut s = DiffSummary::default();
    if old.nodes.is_empty() || new.nodes.is_empty() {
        return s;
    }
    diff_dir(old, old.root, new, new.root, &mut s);
    s
}

fn diff_dir(old: &Tree, oid: NodeId, new: &Tree, nid: NodeId, s: &mut DiffSummary) {
    let omap: HashMap<&str, NodeId> = old
        .node(oid)
        .children
        .iter()
        .map(|&c| (old.node(c).name.as_str(), c))
        .collect();
    let mut seen: Vec<bool> = vec![false; old.node(oid).children.len()];
    let opos: HashMap<NodeId, usize> = old
        .node(oid)
        .children
        .iter()
        .enumerate()
        .map(|(i, &c)| (c, i))
        .collect();

    for &nc in &new.node(nid).children {
        let nn = new.node(nc);
        match omap.get(nn.name.as_str()) {
            Some(&oc) => {
                seen[opos[&oc]] = true;
                let on = old.node(oc);
                if nn.is_dir && on.is_dir {
                    diff_dir(old, oc, new, nc, s);
                } else if nn.is_dir != on.is_dir {
                    s.removed += subtree_items(old, oc);
                    s.added += subtree_items(new, nc);
                    s.delta_bytes += nn.size as i64 - on.size as i64;
                } else if nn.size != on.size || nn.mtime != on.mtime {
                    s.changed += 1;
                    s.delta_bytes += nn.size as i64 - on.size as i64;
                }
            }
            None => {
                s.added += subtree_items(new, nc);
                s.delta_bytes += nn.size as i64;
            }
        }
    }
    for (i, &oc) in old.node(oid).children.iter().enumerate() {
        if !seen[i] {
            s.removed += subtree_items(old, oc);
            s.delta_bytes -= old.node(oc).size as i64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Node;

    fn sample() -> Tree {
        let mut t = Tree::default();
        let mk = |name: &str, parent, size, is_dir| Node {
            name: name.into(),
            parent,
            size,
            mtime: 1,
            is_dir,
            children: vec![],
            error: false,
        };
        t.nodes.push(mk("C:\\root", None, 100, true));
        t.nodes.push(mk("a", Some(0), 60, true));
        t.nodes.push(mk("b", Some(0), 40, true));
        t.nodes.push(mk("f1", Some(1), 40, false));
        t.nodes.push(mk("f2", Some(1), 20, false));
        t.nodes.push(mk("f3", Some(2), 40, false));
        t.nodes[0].children = vec![1, 2];
        t.nodes[1].children = vec![3, 4];
        t.nodes[2].children = vec![5];
        t.file_count = 3;
        t.dir_count = 2;
        t
    }

    #[test]
    fn round_trip() {
        let snap = Snapshot {
            version: VERSION,
            root: PathBuf::from("C:\\root"),
            created_at: 123,
            validated_at: None,
            tree: sample(),
        };
        let bytes = encode(&snap).unwrap();
        let back = decode(&bytes).unwrap();
        assert_eq!(back.root, snap.root);
        assert_eq!(back.tree.nodes.len(), 6);
        assert_eq!(back.tree.total_size(), 100);
        assert_eq!(back.tree.node(1).children, vec![3, 4]);
    }

    #[test]
    fn version_mismatch_rejected() {
        let mut snap = Snapshot {
            version: VERSION,
            root: PathBuf::from("C:\\root"),
            created_at: 0,
            validated_at: None,
            tree: sample(),
        };
        snap.version = 99;
        let bytes = encode(&snap).unwrap();
        assert!(decode(&bytes).is_err());
    }

    #[test]
    fn diff_counts() {
        let old = sample();
        assert!(diff_summary(&old, &old).is_empty());

        let mut new = sample();
        // grow f2, remove f3, add f4 under b
        new.nodes[4].size = 25;
        new.nodes[1].size = 65;
        new.nodes[2].children = vec![];
        new.nodes.push(Node {
            name: "f4".into(),
            parent: Some(2),
            size: 7,
            mtime: 1,
            is_dir: false,
            children: vec![],
            error: false,
        });
        new.nodes[2].children = vec![6];
        let d = diff_summary(&old, &new);
        assert_eq!(d.added, 1);
        assert_eq!(d.removed, 1);
        assert_eq!(d.changed, 1);
        assert_eq!(d.delta_bytes, 5 - 40 + 7);
    }

    #[test]
    fn normalize_keys() {
        assert_eq!(normalize_root(Path::new("C:\\Foo\\")), "c:\\foo");
        assert_eq!(normalize_root(Path::new("c:/foo")), "c:\\foo");
        assert_eq!(normalize_root(Path::new("C:\\")), "c:");
    }
}
