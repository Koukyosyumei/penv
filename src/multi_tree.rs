use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId(usize);

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    MissingNode,
    MissingChild,
    Cycle,
}

struct Node<T, C> {
    value: T,
    children: Vec<(NodeId, C)>,
}

/// Values are interned within the tree. Traversal visits shared children once
/// for each incoming edge, and removal retains nodes reachable elsewhere.
pub struct MultiTree<T, C> {
    nodes: Vec<Option<Node<T, C>>>,
    by_value: HashMap<T, NodeId>,
}

impl<T: Clone + Eq + Hash, C> MultiTree<T, C> {
    pub fn new(root: T) -> Self {
        Self {
            by_value: HashMap::from([(root.clone(), NodeId(0))]),
            nodes: vec![Some(Node {
                value: root,
                children: Vec::new(),
            })],
        }
    }

    pub fn root(&self) -> NodeId {
        NodeId(0)
    }

    pub fn node(&self, value: &T) -> Result<NodeId, Error> {
        self.by_value.get(value).copied().ok_or(Error::MissingNode)
    }

    pub fn value(&self, id: NodeId) -> Option<&T> {
        self.nodes.get(id.0)?.as_ref().map(|node| &node.value)
    }

    pub fn add_child(&mut self, parent: NodeId, value: T, connection: C) -> Result<NodeId, Error> {
        if self.value(parent).is_none() {
            return Err(Error::MissingNode);
        }
        let child = if let Some(&id) = self.by_value.get(&value) {
            if self.reachable_from(id).contains(&parent) {
                return Err(Error::Cycle);
            }
            id
        } else {
            let id = NodeId(self.nodes.len());
            self.by_value.insert(value.clone(), id);
            self.nodes.push(Some(Node {
                value,
                children: Vec::new(),
            }));
            id
        };
        self.nodes[parent.0]
            .as_mut()
            .unwrap()
            .children
            .push((child, connection));
        Ok(child)
    }

    pub fn remove_child(&mut self, parent: NodeId, child: &T) -> Result<(), Error> {
        let child = self.node(child)?;
        let children = &mut self
            .nodes
            .get_mut(parent.0)
            .and_then(Option::as_mut)
            .ok_or(Error::MissingNode)?
            .children;
        let before = children.len();
        children.retain(|&(id, _)| id != child);
        if before == children.len() {
            return Err(Error::MissingChild);
        }
        let reachable = self.reachable_from(self.root());
        self.by_value.retain(|_, id| reachable.contains(id));
        for (index, node) in self.nodes.iter_mut().enumerate() {
            if !reachable.contains(&NodeId(index)) {
                *node = None;
            }
        }
        Ok(())
    }

    fn reachable_from(&self, root: NodeId) -> HashSet<NodeId> {
        let mut seen = HashSet::new();
        let mut pending = vec![root];
        while let Some(id) = pending.pop() {
            if seen.insert(id) {
                pending.extend(
                    self.nodes[id.0]
                        .as_ref()
                        .unwrap()
                        .children
                        .iter()
                        .map(|&(id, _)| id),
                );
            }
        }
        seen
    }

    pub fn values(&self) -> impl Iterator<Item = &T> {
        let mut pending = vec![self.root()];
        std::iter::from_fn(move || {
            let node = self.nodes[pending.pop()?.0].as_ref().unwrap();
            pending.extend(node.children.iter().rev().map(|&(id, _)| id));
            Some(&node.value)
        })
    }
}
