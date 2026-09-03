//! Arena-based file tree shared by the scanner, the snapshot store and the UI.

use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::path::{Path, PathBuf};

pub type NodeId = u32;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    /// File or directory name. The root node stores the full root path.
    pub name: String,
    pub parent: Option<NodeId>,
    /// Files: byte length. Directories: sum of all descendants.
    pub size: u64,
    /// Modification time as unix seconds.
    pub mtime: i64,
    pub is_dir: bool,
    /// Sorted by size descending after a scan.
    pub children: Vec<NodeId>,
    /// Directory could not be read (access denied, etc.).
    pub error: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub root: NodeId,
    pub file_count: u64,
    pub dir_count: u64,
    pub error_count: u32,
}

impl Tree {
    #[inline]
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }

    pub fn root_path(&self) -> PathBuf {
        PathBuf::from(&self.nodes[self.root as usize].name)
    }

    pub fn total_size(&self) -> u64 {
        self.nodes.get(self.root as usize).map(|n| n.size).unwrap_or(0)
    }

    /// Full filesystem path of a node.
    pub fn path(&self, id: NodeId) -> PathBuf {
        let mut parts: Vec<&str> = Vec::new();
        let mut cur = id;
        loop {
            let n = &self.nodes[cur as usize];
            match n.parent {
                Some(p) => {
                    parts.push(&n.name);
                    cur = p;
                }
                None => break,
            }
        }
        let mut p = self.root_path();
        for s in parts.iter().rev() {
            p.push(s);
        }
        p
    }

    /// Ancestors from the root down to (and including) `id`.
    pub fn ancestors(&self, id: NodeId) -> Vec<NodeId> {
        let mut v = Vec::new();
        let mut cur = Some(id);
        while let Some(c) = cur {
            v.push(c);
            cur = self.nodes[c as usize].parent;
        }
        v.reverse();
        v
    }

    pub fn is_descendant_of(&self, id: NodeId, ancestor: NodeId) -> bool {
        let mut cur = Some(id);
        while let Some(c) = cur {
            if c == ancestor {
                return true;
            }
            cur = self.nodes[c as usize].parent;
        }
        false
    }

    /// Resolve a full path back to a node. Component comparison is
    /// case-insensitive (Windows semantics).
    pub fn find_by_path(&self, path: &Path) -> Option<NodeId> {
        let root = self.root_path();
        let rel = path.strip_prefix(&root).ok()?;
        let mut cur = self.root;
        for comp in rel.components() {
            let want = comp.as_os_str().to_string_lossy();
            let n = &self.nodes[cur as usize];
            let next = n
                .children
                .iter()
                .copied()
                .find(|&c| self.nodes[c as usize].name.eq_ignore_ascii_case(&want))?;
            cur = next;
        }
        Some(cur)
    }

    /// Like `find_by_path`, but falls back to the nearest existing ancestor
    /// and finally the root.
    pub fn resolve_nearest(&self, path: &Path) -> NodeId {
        let mut p = path;
        loop {
            if let Some(id) = self.find_by_path(p) {
                return id;
            }
            match p.parent() {
                Some(pp) => p = pp,
                None => return self.root,
            }
        }
    }

    /// (files, dirs) contained in the subtree, excluding the node itself.
    pub fn count_items(&self, id: NodeId) -> (u64, u64) {
        let mut files = 0;
        let mut dirs = 0;
        let mut stack = vec![id];
        while let Some(c) = stack.pop() {
            let n = &self.nodes[c as usize];
            for &ch in &n.children {
                if self.nodes[ch as usize].is_dir {
                    dirs += 1;
                    stack.push(ch);
                } else {
                    files += 1;
                }
            }
        }
        (files, dirs)
    }

    pub fn sort_children_by_size(&mut self) {
        for i in 0..self.nodes.len() {
            let mut ch = std::mem::take(&mut self.nodes[i].children);
            ch.sort_by_key(|&c| Reverse(self.nodes[c as usize].size));
            self.nodes[i].children = ch;
        }
    }

    /// Detach a subtree, fix ancestor sizes and counts, then compact the
    /// arena. Node ids are invalidated; callers must re-resolve by path.
    pub fn remove_subtree(&mut self, id: NodeId) -> bool {
        let Some(parent) = self.nodes[id as usize].parent else {
            return false; // never remove the root
        };
        let removed_size = self.nodes[id as usize].size;
        let (files, dirs) = self.count_items(id);
        let is_dir = self.nodes[id as usize].is_dir;
        let is_err = self.nodes[id as usize].error;

        self.nodes[parent as usize].children.retain(|&c| c != id);
        let mut cur = Some(parent);
        while let Some(c) = cur {
            let n = &mut self.nodes[c as usize];
            n.size = n.size.saturating_sub(removed_size);
            cur = n.parent;
        }
        self.file_count = self
            .file_count
            .saturating_sub(files + if is_dir { 0 } else { 1 });
        self.dir_count = self
            .dir_count
            .saturating_sub(dirs + if is_dir { 1 } else { 0 });
        if is_err {
            self.error_count = self.error_count.saturating_sub(1);
        }
        self.compact();
        true
    }

    /// Rebuild the arena keeping only nodes reachable from the root.
    fn compact(&mut self) {
        let old = std::mem::take(&mut self.nodes);
        let mut new: Vec<Node> = Vec::with_capacity(old.len());
        let mut stack: Vec<(NodeId, Option<NodeId>)> = vec![(self.root, None)];
        while let Some((oid, new_parent)) = stack.pop() {
            let n = &old[oid as usize];
            let nid = new.len() as NodeId;
            new.push(Node {
                name: n.name.clone(),
                parent: new_parent,
                size: n.size,
                mtime: n.mtime,
                is_dir: n.is_dir,
                children: Vec::with_capacity(n.children.len()),
                error: n.error,
            });
            if let Some(p) = new_parent {
                new[p as usize].children.push(nid);
            }
            for &c in n.children.iter().rev() {
                stack.push((c, Some(nid)));
            }
        }
        self.nodes = new;
        self.root = 0;
        self.sort_children_by_size();
    }
}

pub fn fmt_size(bytes: u64) -> String {
    humansize::format_size(bytes, humansize::BINARY)
}

pub fn fmt_time(unix_secs: i64) -> String {
    chrono::DateTime::from_timestamp(unix_secs, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "?".into())
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn sample() -> Tree {
        // root(100) -> a(60){ f1(40), f2(20) }, b(40){ f3(40) }
        let mut t = Tree::default();
        let mk = |name: &str, parent, size, is_dir| Node {
            name: name.into(),
            parent,
            size,
            mtime: 0,
            is_dir,
            children: vec![],
            error: false,
        };
        t.nodes.push(mk("C:\\root", None, 100, true)); // 0
        t.nodes.push(mk("a", Some(0), 60, true)); // 1
        t.nodes.push(mk("b", Some(0), 40, true)); // 2
        t.nodes.push(mk("f1", Some(1), 40, false)); // 3
        t.nodes.push(mk("f2", Some(1), 20, false)); // 4
        t.nodes.push(mk("f3", Some(2), 40, false)); // 5
        t.nodes[0].children = vec![1, 2];
        t.nodes[1].children = vec![3, 4];
        t.nodes[2].children = vec![5];
        t.file_count = 3;
        t.dir_count = 2;
        t
    }

    #[test]
    fn path_and_find() {
        let t = sample();
        let p = t.path(4);
        assert_eq!(p, PathBuf::from("C:\\root\\a\\f2"));
        assert_eq!(t.find_by_path(&p), Some(4));
        assert_eq!(t.find_by_path(Path::new("C:\\root\\A\\F2")), Some(4));
        assert_eq!(t.find_by_path(Path::new("C:\\root\\zzz")), None);
        assert_eq!(t.find_by_path(Path::new("C:\\root")), Some(0));
        assert_eq!(t.resolve_nearest(Path::new("C:\\root\\a\\nope\\x")), 1);
        assert_eq!(t.resolve_nearest(Path::new("D:\\elsewhere")), 0);
    }

    #[test]
    fn remove_subtree_updates_sizes_and_compacts() {
        let mut t = sample();
        assert!(t.remove_subtree(3)); // remove f1 (40)
        assert_eq!(t.total_size(), 60);
        let a = t.find_by_path(Path::new("C:\\root\\a")).unwrap();
        assert_eq!(t.node(a).size, 20);
        assert_eq!(t.nodes.len(), 5);
        assert_eq!(t.file_count, 2);
        assert!(!t.remove_subtree(t.root));
        let b = t.find_by_path(Path::new("C:\\root\\b")).unwrap();
        assert!(t.remove_subtree(b));
        assert_eq!(t.total_size(), 20);
        assert_eq!(t.dir_count, 1);
        assert_eq!(t.file_count, 1);
        // every child still points back to its parent
        for (i, n) in t.nodes.iter().enumerate() {
            for &c in &n.children {
                assert_eq!(t.nodes[c as usize].parent, Some(i as NodeId));
            }
        }
    }

    #[test]
    fn counts() {
        let t = sample();
        assert_eq!(t.count_items(0), (3, 2));
        assert_eq!(t.count_items(1), (2, 0));
        assert_eq!(t.ancestors(4), vec![0, 1, 4]);
        assert!(t.is_descendant_of(4, 1));
        assert!(!t.is_descendant_of(5, 1));
    }
}
