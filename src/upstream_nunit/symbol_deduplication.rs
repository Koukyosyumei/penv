use crate::symbol::Symbol;
use crate::symbol_lookup::SymbolLookup;
use std::sync::Arc;

fn symbols<const N: usize>(values: [(u64, u64, &str); N]) -> [Symbol; N] {
    values.map(|(start, end, name)| Symbol::new(start, end, name).unwrap())
}

#[test]
fn should_deduplicate_identical_symbols() {
    let inputs = [
        (0, 100, "一"),
        (1, 85, "四"),
        (2, 70, "国"),
        (80, 85, "猫"),
        (3, 60, "中"),
        (70, 75, "私"),
        (4, 15, "五"),
        (20, 35, "三"),
    ];
    let original = symbols(inputs);
    let copies = symbols(inputs);
    let mut first = SymbolLookup::default();
    let mut second = SymbolLookup::default();
    first.insert_symbols(original.iter().cloned());
    second.insert_symbols(copies.iter().cloned());
    for (i, (symbol, copy)) in original.iter().zip(&copies).enumerate() {
        assert!(
            Arc::ptr_eq(
                first.by_name(&symbol.name).next().unwrap(),
                second.by_name(&copy.name).next().unwrap()
            ),
            "Symbol {i}, has not been deduplicated."
        );
    }
}

#[test]
fn should_not_deduplicate_different_symbols() {
    let original = symbols([(0, 100, "一"), (20, 35, "三")]);
    let other = symbols([(20, 35, "一")]);
    let mut first = SymbolLookup::default();
    let mut second = SymbolLookup::default();
    first.insert_symbols(original.iter().cloned());
    second.insert_symbols(other);
    let name = &original[0].name;
    assert!(
        !Arc::ptr_eq(
            first.by_name(name).next().unwrap(),
            second.by_name(name).next().unwrap()
        ),
        "Symbol {name} has been deduplicated while it shouldn't."
    );
    let symbol = &original[1];
    assert!(
        !Arc::ptr_eq(
            first.by_address(symbol.start()).unwrap(),
            second.by_address(symbol.start()).unwrap()
        ),
        "Symbol {} has been deduplicated while it shouldn't.",
        symbol.name
    );
}

#[test]
fn should_deduplicate_after_merge() {
    let original = symbols([(0, 100, "一"), (20, 35, "三"), (100, 105, "猫")]);
    let incoming = symbols([(15, 25, "私")]);
    let mut first = SymbolLookup::default();
    let mut second = SymbolLookup::default();
    first.insert_symbols(original.iter().cloned());
    second.insert_symbols(original.iter().cloned());
    first.insert_symbols(incoming);

    let name = &original[2].name;
    assert!(
        Arc::ptr_eq(
            first.by_name(name).next().unwrap(),
            second.by_name(name).next().unwrap()
        ),
        "Symbol {name} has NOT been deduplicated."
    );
    let name = &original[1].name;
    assert!(
        !Arc::ptr_eq(
            first.by_name(name).next().unwrap(),
            second.by_name(name).next().unwrap()
        ),
        "Symbol {name} has been deduplicated while it shouldn't."
    );
}
