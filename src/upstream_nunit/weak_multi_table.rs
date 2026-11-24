use crate::weak_multi_table::WeakMultiTable;
use std::sync::Arc;

#[derive(PartialEq, Eq)]
struct Object(u8);

#[test]
fn should_handle_many_right_values_for_left() {
    let [first, second, third] = [0, 1, 2].map(|id| Arc::new(Object(id)));
    let table = WeakMultiTable::default();
    table.add(&first, &second);
    table.add(&first, &third);
    assert_eq!(table.for_left(&first).len(), 2);
}

#[test]
fn should_handle_many_left_values_for_right() {
    let [first, second, third] = [0, 1, 2].map(|id| Arc::new(Object(id)));
    let table = WeakMultiTable::default();
    table.add(&second, &first);
    table.add(&third, &first);
    assert_eq!(table.for_right(&first).len(), 2);
}

#[test]
fn should_handle_remove_of_unexisting_item() {
    let table = WeakMultiTable::<i32, i32>::default();
    assert_eq!(table.for_left(&0).len(), 0);
    table.remove_pair(&0, &0);
    assert_eq!(table.for_left(&0).len(), 0);
}
