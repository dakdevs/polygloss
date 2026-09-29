//! The file tree's shape (design §11.5): the changed files' paths as a tree
//! of directories and files, with chains of single-child directories
//! compacted into one node (`src/app/ui`). Pure data, no GPUI.
//!
//! Children keep the order in which their first file appears in the diff
//! (git's path order), so walking the tree visits files in the viewport's
//! order and `n`/`p` agree between the tree and the diff.

use std::collections::HashMap;

/// A node's index in [`TreeModel::nodes`].
pub type NodeId = usize;

/// What a node is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// A directory (possibly a compacted chain of them).
    Dir,
    /// The diff's file at this index.
    File(u32),
}

/// One node of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub kind: NodeKind,
    /// What the row shows: a file's or directory's name, or a compacted
    /// chain (`app/ui`).
    pub name: String,
    /// The full path (for a compacted chain, of its deepest directory).
    pub path: String,
    /// Child nodes, in diff order.
    pub children: Vec<NodeId>,
    /// Every file at or below this node, in diff order.
    pub files: Vec<u32>,
}

impl Node {
    pub fn is_dir(&self) -> bool {
        self.kind == NodeKind::Dir
    }

    /// The node's id in the gpui-kit tree: `d:<path>` for directories,
    /// `f:<index>` for files.
    pub fn item_id(&self) -> String {
        ItemId::of(self).to_string()
    }
}

/// A tree item's id, parsed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ItemId {
    Dir(String),
    File(u32),
}

impl ItemId {
    pub fn of(node: &Node) -> ItemId {
        match node.kind {
            NodeKind::Dir => ItemId::Dir(node.path.clone()),
            NodeKind::File(idx) => ItemId::File(idx),
        }
    }

    /// Parses `d:<path>` / `f:<index>`.
    pub fn parse(id: &str) -> Option<ItemId> {
        if let Some(path) = id.strip_prefix("d:") {
            return Some(ItemId::Dir(path.to_owned()));
        }
        id.strip_prefix("f:")?.parse().ok().map(ItemId::File)
    }
}

impl std::fmt::Display for ItemId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ItemId::Dir(path) => write!(f, "d:{path}"),
            ItemId::File(idx) => write!(f, "f:{idx}"),
        }
    }
}

/// The compacted tree of some of a diff's files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeModel {
    nodes: Vec<Node>,
    roots: Vec<NodeId>,
    /// Files in tree order (depth first).
    file_order: Vec<u32>,
    /// Directory path → node.
    dirs: HashMap<String, NodeId>,
}

impl TreeModel {
    /// The tree of `files` (`(index, path)` in diff order; paths use `/`).
    pub fn build<'a>(files: impl IntoIterator<Item = (u32, &'a str)>) -> TreeModel {
        let mut b = Builder::default();
        for (idx, path) in files {
            b.insert(idx, path);
        }
        b.finish()
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    /// The top-level nodes, in diff order.
    pub fn roots(&self) -> &[NodeId] {
        &self.roots
    }

    /// Every file in tree order.
    pub fn file_order(&self) -> &[u32] {
        &self.file_order
    }

    /// The directory node whose (deepest) path is `path`.
    pub fn dir(&self, path: &str) -> Option<&Node> {
        self.dirs.get(path).map(|&id| &self.nodes[id])
    }

    /// Every directory node's path, in tree order.
    pub fn dir_paths(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(self.dirs.len());
        let mut stack: Vec<NodeId> = self.roots.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            let node = &self.nodes[id];
            if node.is_dir() {
                out.push(node.path.clone());
                stack.extend(node.children.iter().rev().copied());
            }
        }
        out
    }

    /// The directories above file `idx`, outermost first (their paths).
    pub fn ancestors_of_file(&self, idx: u32) -> Vec<String> {
        fn walk(model: &TreeModel, id: NodeId, idx: u32, path: &mut Vec<String>) -> bool {
            let node = &model.nodes[id];
            match node.kind {
                NodeKind::File(i) => i == idx,
                NodeKind::Dir => {
                    // Skip subtrees that do not hold the file.
                    if !node.files.contains(&idx) {
                        return false;
                    }
                    path.push(node.path.clone());
                    if node.children.iter().any(|&c| walk(model, c, idx, path)) {
                        return true;
                    }
                    path.pop();
                    false
                }
            }
        }
        let mut path = Vec::new();
        for &root in &self.roots {
            if walk(self, root, idx, &mut path) {
                break;
            }
        }
        path
    }
}

/// An uncompacted directory while building.
#[derive(Default)]
struct RawDir {
    /// Child directory by name.
    dirs: HashMap<String, usize>,
    /// Children in first-appearance order.
    order: Vec<RawChild>,
}

enum RawChild {
    Dir(usize, String),
    File(u32, String),
}

#[derive(Default)]
struct Builder {
    /// `raw[0]` is the root.
    raw: Vec<RawDir>,
}

impl Builder {
    fn insert(&mut self, idx: u32, path: &str) {
        if self.raw.is_empty() {
            self.raw.push(RawDir::default());
        }
        let mut parts = path.split('/').filter(|p| !p.is_empty()).peekable();
        let mut dir = 0;
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                self.raw[dir]
                    .order
                    .push(RawChild::File(idx, part.to_owned()));
                return;
            }
            dir = match self.raw[dir].dirs.get(part) {
                Some(&d) => d,
                None => {
                    let d = self.raw.len();
                    self.raw.push(RawDir::default());
                    self.raw[dir].dirs.insert(part.to_owned(), d);
                    self.raw[dir].order.push(RawChild::Dir(d, part.to_owned()));
                    d
                }
            };
        }
        // An empty path: list it at the top.
        self.raw[0].order.push(RawChild::File(idx, String::new()));
    }

    fn finish(mut self) -> TreeModel {
        let mut model = TreeModel::default();
        if self.raw.is_empty() {
            return model;
        }
        let top = std::mem::take(&mut self.raw[0].order);
        for child in top {
            let id = self.emit(child, "", &mut model);
            model.roots.push(id);
        }
        model
    }

    /// Adds `child` (under `parent_path`) and its subtree to `model`,
    /// compacting single-child directory chains.
    fn emit(&mut self, child: RawChild, parent_path: &str, model: &mut TreeModel) -> NodeId {
        let join = |a: &str, b: &str| {
            if a.is_empty() {
                b.to_owned()
            } else {
                format!("{a}/{b}")
            }
        };
        match child {
            RawChild::File(idx, name) => {
                let id = model.nodes.len();
                model.nodes.push(Node {
                    kind: NodeKind::File(idx),
                    path: join(parent_path, &name),
                    name,
                    children: Vec::new(),
                    files: vec![idx],
                });
                model.file_order.push(idx);
                id
            }
            RawChild::Dir(mut d, first) => {
                let mut name = first;
                let mut path = join(parent_path, &name);
                // Compact: a directory whose only child is a directory.
                while self.raw[d].order.len() == 1
                    && matches!(self.raw[d].order[0], RawChild::Dir(..))
                {
                    let Some(RawChild::Dir(next, part)) = self.raw[d].order.pop() else {
                        unreachable!()
                    };
                    name = format!("{name}/{part}");
                    path = format!("{path}/{part}");
                    d = next;
                }
                let id = model.nodes.len();
                model.nodes.push(Node {
                    kind: NodeKind::Dir,
                    name,
                    path: path.clone(),
                    children: Vec::new(),
                    files: Vec::new(),
                });
                model.dirs.insert(path.clone(), id);
                let first_file = model.file_order.len();
                let children = std::mem::take(&mut self.raw[d].order);
                let ids: Vec<NodeId> = children
                    .into_iter()
                    .map(|c| self.emit(c, &path, model))
                    .collect();
                let files = model.file_order[first_file..].to_vec();
                let node = &mut model.nodes[id];
                node.children = ids;
                node.files = files;
                id
            }
        }
    }
}
