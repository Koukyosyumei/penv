use crate::multi_tree::{Error, MultiTree};

type Tree = MultiTree<i32, Option<String>>;

fn multiset(tree: &Tree, expected: &[i32]) {
    let mut values: Vec<_> = tree.values().copied().collect();
    values.sort_unstable();
    assert_eq!(values, expected);
}

fn branching() -> Tree {
    let mut tree = Tree::new(1);
    let two = tree.add_child(tree.root(), 2, None).unwrap();
    tree.add_child(two, 3, None).unwrap();
    tree.add_child(two, 4, None).unwrap();
    tree.add_child(tree.root(), 5, None).unwrap();
    tree
}

#[test]
fn should_traverse_simple_tree() {
    let mut tree = Tree::new(1);
    tree.add_child(tree.root(), 2, None).unwrap();
    tree.add_child(tree.root(), 3, None).unwrap();
    multiset(&tree, &[1, 2, 3]);
}

#[test]
fn should_traverse_another_simple_tree() {
    multiset(&branching(), &[1, 2, 3, 4, 5]);
}

#[test]
fn should_traverse_multi_tree() {
    let mut tree = Tree::new(1);
    let two = tree.add_child(tree.root(), 2, None).unwrap();
    tree.add_child(two, 3, None).unwrap();
    tree.add_child(tree.root(), 3, None).unwrap();
    multiset(&tree, &[1, 2, 3, 3]);
}

#[test]
fn should_create_one_node_for_the_same_value() {
    let mut tree = Tree::new(1);
    let two = tree.add_child(tree.root(), 2, None).unwrap();
    let three = tree.add_child(two, 3, None).unwrap();
    let another = tree.add_child(tree.root(), 3, None).unwrap();
    assert_eq!(three, another);
}

#[test]
fn should_find_root() {
    let tree = Tree::new(1);
    assert_eq!(tree.value(tree.node(&1).unwrap()), Some(&1));
}

#[test]
fn should_remove_subtree() {
    let mut tree = branching();
    tree.remove_child(tree.root(), &2).unwrap();
    multiset(&tree, &[1, 5]);
}

#[test]
fn should_remove_subtree_from_dictionary() {
    let mut tree = branching();
    tree.remove_child(tree.root(), &2).unwrap();
    for value in [2, 3, 4] {
        assert_eq!(tree.node(&value), Err(Error::MissingNode));
    }
    tree.node(&5).unwrap();
}
