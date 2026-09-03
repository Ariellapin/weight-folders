//! Parallel recursive directory scanner built on rayon's work-stealing pool.

use crate::model::{Node, NodeId, Tree};
use rayon::prelude::*;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

/// Live counters polled by the UI while a scan runs.
#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub bytes: AtomicU64,
    pub errors: AtomicU64,
    pub current: Mutex<String>,
}

impl Progress {
    pub fn current(&self) -> String {
        self.current.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

#[derive(Debug)]
pub enum ScanError {
    Cancelled,
    Io(String),
}

struct FileEntry {
    name: String,
    size: u64,
    mtime: i64,
}

struct DirResult {
    name: String,
    mtime: i64,
    error: bool,
    size: u64,
    files: Vec<FileEntry>,
    subdirs: Vec<DirResult>,
}

fn mtime_of(md: &fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Scan `root` recursively. Blocks until finished; run it on a worker thread.
pub fn scan(root: &Path, progress: Arc<Progress>, cancel: Arc<AtomicBool>) -> Result<Tree, ScanError> {
    let meta = fs::metadata(root).map_err(|e| ScanError::Io(format!("{}: {e}", root.display())))?;
    if !meta.is_dir() {
        return Err(ScanError::Io(format!("{} is not a directory", root.display())));
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .stack_size(16 << 20)
        .build()
        .map_err(|e| ScanError::Io(e.to_string()))?;

    let root_name = root.display().to_string();
    let result = pool.install(|| scan_dir(root, root_name, mtime_of(&meta), &progress, &cancel))?;

    let est = (progress.files.load(Ordering::Relaxed) + progress.dirs.load(Ordering::Relaxed)) as usize;
    let mut tree = Tree {
        nodes: Vec::with_capacity(est + 1),
        ..Default::default()
    };
    flatten(&mut tree, result, None);
    tree.root = 0;
    tree.dir_count = tree.dir_count.saturating_sub(1); // don't count the root itself
    tree.sort_children_by_size();
    Ok(tree)
}

fn scan_dir(
    path: &Path,
    name: String,
    mtime: i64,
    progress: &Progress,
    cancel: &AtomicBool,
) -> Result<DirResult, ScanError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(ScanError::Cancelled);
    }
    let mut res = DirResult {
        name,
        mtime,
        error: false,
        size: 0,
        files: Vec::new(),
        subdirs: Vec::new(),
    };

    let rd = match fs::read_dir(path) {
        Ok(rd) => rd,
        Err(_) => {
            res.error = true;
            progress.errors.fetch_add(1, Ordering::Relaxed);
            progress.dirs.fetch_add(1, Ordering::Relaxed);
            return Ok(res);
        }
    };

    if let Ok(mut c) = progress.current.try_lock() {
        *c = path.display().to_string();
    }

    let mut subdirs: Vec<(String, i64)> = Vec::new();
    let mut bytes = 0u64;
    for entry in rd.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        // Symlinks, junctions and mount points are skipped to avoid loops
        // and double counting.
        if ft.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(md) = entry.metadata() else { continue };
        let mtime = mtime_of(&md);
        if ft.is_dir() {
            subdirs.push((name, mtime));
        } else {
            let size = md.len();
            bytes += size;
            res.files.push(FileEntry { name, size, mtime });
        }
    }

    progress.files.fetch_add(res.files.len() as u64, Ordering::Relaxed);
    progress.bytes.fetch_add(bytes, Ordering::Relaxed);
    progress.dirs.fetch_add(1, Ordering::Relaxed);

    let subs: Result<Vec<DirResult>, ScanError> = subdirs
        .into_par_iter()
        .map(|(n, mt)| scan_dir(&path.join(&n), n, mt, progress, cancel))
        .collect();
    res.subdirs = subs?;

    res.size = bytes + res.subdirs.iter().map(|d| d.size).sum::<u64>();
    Ok(res)
}

fn flatten(tree: &mut Tree, d: DirResult, parent: Option<NodeId>) -> NodeId {
    let id = tree.nodes.len() as NodeId;
    tree.nodes.push(Node {
        name: d.name,
        parent,
        size: d.size,
        mtime: d.mtime,
        is_dir: true,
        children: Vec::new(),
        error: d.error,
    });
    tree.dir_count += 1;
    if d.error {
        tree.error_count += 1;
    }
    let mut children = Vec::with_capacity(d.files.len() + d.subdirs.len());
    for f in d.files {
        let fid = tree.nodes.len() as NodeId;
        tree.nodes.push(Node {
            name: f.name,
            parent: Some(id),
            size: f.size,
            mtime: f.mtime,
            is_dir: false,
            children: Vec::new(),
            error: false,
        });
        tree.file_count += 1;
        children.push(fid);
    }
    for s in d.subdirs {
        let sid = flatten(tree, s, Some(id));
        children.push(sid);
    }
    tree.nodes[id as usize].children = children;
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_tree(tag: &str) -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!(
            "weight-folders-test-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("a/deep")).unwrap();
        fs::create_dir_all(base.join("b")).unwrap();
        fs::create_dir_all(base.join("empty")).unwrap();
        let write = |p: &Path, n: usize| {
            let mut f = fs::File::create(p).unwrap();
            f.write_all(&vec![0u8; n]).unwrap();
        };
        write(&base.join("top.txt"), 100);
        write(&base.join("a/one.bin"), 1000);
        write(&base.join("a/deep/two.bin"), 2000);
        write(&base.join("b/three.bin"), 300);
        base
    }

    #[test]
    fn scans_sizes_and_counts() {
        let base = temp_tree("sizes");
        let progress = Arc::new(Progress::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let tree = scan(&base, progress.clone(), cancel).unwrap();
        assert_eq!(tree.total_size(), 3400);
        assert_eq!(tree.file_count, 4);
        assert_eq!(tree.dir_count, 4);
        assert_eq!(tree.error_count, 0);
        let a = tree.find_by_path(&base.join("a")).unwrap();
        assert_eq!(tree.node(a).size, 3000);
        assert!(tree.node(a).is_dir);
        // children sorted by size desc
        let root_children = &tree.node(tree.root).children;
        let sizes: Vec<u64> = root_children.iter().map(|&c| tree.node(c).size).collect();
        assert_eq!(sizes, vec![3000, 300, 100, 0]);
        assert_eq!(progress.files.load(Ordering::Relaxed), 4);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn cancel_aborts() {
        let base = temp_tree("cancel");
        let progress = Arc::new(Progress::default());
        let cancel = Arc::new(AtomicBool::new(true));
        assert!(matches!(scan(&base, progress, cancel), Err(ScanError::Cancelled)));
        let _ = fs::remove_dir_all(&base);
    }
}
