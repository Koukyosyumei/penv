use crate::symbol::Symbol;
use crate::symbol_lookup::SymbolLookup;

fn sized_symbol(name: &str, start: u64, size: u64) -> Symbol {
    Symbol::new(start, start + size, name).unwrap()
}

fn lookup_for(inputs: &[(&str, u64, u64)]) -> SymbolLookup {
    let mut lookup = SymbolLookup::default();
    lookup.insert_symbols(
        inputs
            .iter()
            .map(|&(name, start, size)| sized_symbol(name, start, size)),
    );
    lookup
}

fn name_at(lookup: &SymbolLookup, address: u64) -> Option<&str> {
    lookup
        .by_address(address)
        .map(|symbol| symbol.name.as_str())
}

fn assert_names(lookup: &SymbolLookup, expected: &[(u64, &str)]) {
    for &(address, name) in expected {
        assert_eq!(name_at(lookup, address), Some(name), "address {address:#x}");
    }
}

#[test]
fn should_find_one_symbol() {
    assert_names(&lookup_for(&[("Test", 0x100, 0x10)]), &[(0x105, "Test")]);
}

#[test]
fn should_find_one_symbol_boundary() {
    assert_names(&lookup_for(&[("Test", 0x100, 0x10)]), &[(0x100, "Test")]);
}

#[test]
fn should_not_find_one_symbol_boundary() {
    assert_eq!(name_at(&lookup_for(&[("Test", 0x100, 0x10)]), 0x110), None);
}

#[test]
fn should_not_find_one_symbol_miss() {
    assert_eq!(name_at(&lookup_for(&[("Test", 0x100, 0x10)]), 0x120), None);
}

#[test]
fn should_trim_big_symbol() {
    let lookup = lookup_for(&[("Large", 0, 10), ("Small", 9, 1)]);
    assert_eq!(name_at(&lookup, 10), None);
    assert_names(&lookup, &[(9, "Small"), (8, "Large"), (0, "Large")]);
}

#[test]
fn should_find_ten_symbols() {
    let symbols: Vec<_> = (1..=10)
        .map(|i| sized_symbol(&i.to_string(), i * 10, 5))
        .collect();
    let mut lookup = SymbolLookup::default();
    lookup.insert_symbols(symbols.iter().cloned());
    for symbol in symbols {
        assert_eq!(
            name_at(&lookup, symbol.start() + 2),
            Some(symbol.name.as_str())
        );
    }
}

#[test]
fn should_find_simply_nested_symbol() {
    let lookup = lookup_for(&[("Big", 0, 100), ("Small", 50, 10)]);
    assert_names(
        &lookup,
        &[(10, "Big"), (51, "Small"), (60, "Big"), (61, "Big")],
    );
}

#[test]
fn should_find_recursively_nested_symbol() {
    let lookup = lookup_for(&[("Big", 0, 100), ("Average", 40, 20), ("Small", 50, 5)]);
    assert_names(
        &lookup,
        &[
            (10, "Big"),
            (41, "Average"),
            (51, "Small"),
            (55, "Average"),
            (60, "Big"),
        ],
    );
}

#[test]
fn should_not_find_hole() {
    let lookup = lookup_for(&[("Alice", 0, 50), ("Bob", 51, 50)]);
    assert_eq!(name_at(&lookup, 50), None);
}

#[test]
fn should_find_double_cut() {
    let lookup = lookup_for(&[("Big", 0, 100), ("Alice", 10, 20), ("Bob", 70, 20)]);
    assert_names(
        &lookup,
        &[
            (5, "Big"),
            (15, "Alice"),
            (35, "Big"),
            (75, "Bob"),
            (95, "Big"),
        ],
    );
}

#[test]
fn should_find_nested_and_double_cut() {
    let lookup = lookup_for(&[
        ("Big", 0, 100),
        ("Alice", 10, 30),
        ("Small1", 20, 5),
        ("Small2", 30, 5),
        ("Bob", 60, 30),
        ("Small3", 70, 5),
    ]);
    assert_names(
        &lookup,
        &[
            (5, "Big"),
            (15, "Alice"),
            (22, "Small1"),
            (26, "Alice"),
            (31, "Small2"),
            (36, "Alice"),
            (41, "Big"),
            (61, "Bob"),
            (71, "Small3"),
            (76, "Bob"),
            (91, "Big"),
        ],
    );
}

#[test]
fn should_work_with_long_series_and_nesting() {
    let lookup = lookup_for(&[
        ("Large", 0, 10),
        ("First", 3, 3),
        ("Small", 4, 1),
        ("Last", 6, 1),
    ]);
    assert_eq!(name_at(&lookup, 10), None);
    assert_names(
        &lookup,
        &[
            (9, "Large"),
            (6, "Last"),
            (5, "First"),
            (4, "Small"),
            (3, "First"),
            (2, "Large"),
        ],
    );
}

#[test]
fn should_not_crash_on_insert_doubled_symbol() {
    let inputs = [
        ("一", 0, 100),
        ("二", 0, 100),
        ("三", 100, 15),
        ("四", 100, 15),
        ("国", 10, 15),
        ("五", 10, 15),
    ];
    let lookup = lookup_for(&inputs);
    for (name, start, size) in inputs {
        let _ = lookup.by_name(name).count();
        lookup.by_address(start).unwrap();
        let _ = lookup.by_address(start + size);
    }
}

#[test]
fn should_find_zero_length_symbol() {
    let symbols = [
        (0, 75, "一"),
        (50, 75, "三"),
        (75, 75, "二"),
        (75, 100, "一"),
        (75, 99, "三"),
    ]
    .map(|(start, end, name)| Symbol::new(start, end, name).unwrap());
    let mut lookup = SymbolLookup::default();
    lookup.insert_symbols(symbols.iter().cloned());
    let actual = [5, 60, 75, 99, 76].map(|address| lookup.by_address(address).unwrap().as_ref());
    assert_eq!(symbols.each_ref(), actual);
}
