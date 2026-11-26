use crate::{
    symbol::{Symbol, SymbolBinding, SymbolType},
    symbol_lookup::SymbolLookup,
};

fn symbols(rows: &[(u64, u64, &str)]) -> Vec<Symbol> {
    rows.iter()
        .map(|&(start, end, name)| Symbol::new(start, end, name).unwrap())
        .collect()
}

fn query(lookup: &SymbolLookup, addresses: &[u64]) -> Vec<Symbol> {
    addresses
        .iter()
        .map(|&address| lookup.by_address(address).unwrap().as_ref().clone())
        .collect()
}

#[test]
fn should_merge_trim_end_and_beginning() {
    let first = symbols(&[(0, 50, "一"), (50, 100, "三")]);
    let second = symbols(&[(30, 70, "二")]);
    let expected = symbols(&[(0, 30, "一"), (30, 70, "二"), (70, 100, "三")]);
    let addresses = [25, 35, 75];
    let mut lookup = SymbolLookup::default();
    lookup.insert_symbols(first);
    lookup.insert_symbols(second);
    assert_eq!(query(&lookup, &addresses), expected);
}

#[test]
fn should_sorted_intervals_have_expected_layout() {
    let first = symbols(&[
        (0, 100, "一"),
        (1, 85, "四"),
        (2, 70, "国"),
        (80, 85, "猫"),
        (3, 60, "中"),
        (70, 75, "私"),
        (4, 15, "五"),
        (20, 35, "三"),
    ]);
    let second = symbols(&[
        (5, 25, "糞"),
        (40, 82, "二"),
        (50, 55, "ICantSpeekJapaneese"),
        (45, 50, "ICantSpeekKorean"),
    ]);
    let expected = symbols(&[
        (0, 5, "一"),
        (1, 5, "四"),
        (2, 5, "国"),
        (3, 5, "中"),
        (4, 5, "五"),
        (5, 25, "糞"),
        (25, 40, "中"),
        (25, 35, "三"),
        (40, 82, "二"),
        (45, 50, "ICantSpeekKorean"),
        (50, 55, "ICantSpeekJapaneese"),
        (82, 100, "一"),
        (82, 85, "猫"),
    ]);
    let addresses = [0, 1, 2, 3, 4, 10, 37, 30, 42, 47, 53, 90, 83];
    let mut lookup = SymbolLookup::default();
    lookup.insert_symbols(first);
    lookup.insert_symbols(second);
    assert_eq!(query(&lookup, &addresses), expected);
}

#[test]
fn should_trim_end_and_beginning2() {
    let first = symbols(&[(30, 70, "二")]);
    let second = symbols(&[(0, 40, "一"), (60, 100, "三")]);
    let expected = symbols(&[(0, 40, "一"), (40, 60, "二"), (60, 100, "三")]);
    let addresses = [25, 59, 60];
    let mut lookup = SymbolLookup::default();
    lookup.insert_symbols(first);
    lookup.insert_symbols(second);
    assert_eq!(query(&lookup, &addresses), expected);
}

#[test]
fn should_trim_cake_ends() {
    let first = symbols(&[
        (0, 100, "一"),
        (1, 85, "四"),
        (2, 70, "国"),
        (3, 60, "中"),
        (4, 60, "五"),
        (20, 50, "三"),
    ]);
    let second = symbols(&[(50, 100, "猫")]);
    let expected = symbols(&[
        (0, 50, "一"),
        (1, 50, "四"),
        (2, 50, "国"),
        (3, 50, "中"),
        (4, 50, "五"),
        (20, 50, "三"),
        (50, 100, "猫"),
    ]);
    let addresses = [0, 1, 2, 3, 15, 35, 99];
    let mut lookup = SymbolLookup::default();
    lookup.insert_symbols(first);
    lookup.insert_symbols(second);
    assert_eq!(query(&lookup, &addresses), expected);
}

#[test]
fn should_trim_cake_beginnings() {
    let first = symbols(&[
        (0, 100, "一"),
        (1, 85, "四"),
        (2, 70, "国"),
        (3, 60, "中"),
        (20, 50, "三"),
    ]);
    let second = symbols(&[(0, 30, "猫")]);
    let expected = symbols(&[
        (0, 30, "猫"),
        (30, 100, "一"),
        (30, 85, "四"),
        (30, 70, "国"),
        (30, 60, "中"),
        (30, 50, "三"),
    ]);
    let addresses = [10, 95, 80, 65, 55, 45];
    let mut lookup = SymbolLookup::default();
    lookup.insert_symbols(first);
    lookup.insert_symbols(second);
    assert_eq!(query(&lookup, &addresses), expected);
}

#[test]
fn should_cut_into_cake() {
    let first = symbols(&[
        (0, 100, "一"),
        (1, 85, "四"),
        (2, 70, "国"),
        (3, 60, "中"),
        (20, 50, "三"),
    ]);
    let second = symbols(&[(35, 40, "猫")]);
    let expected = symbols(&[
        (0, 35, "一"),
        (1, 35, "四"),
        (2, 35, "国"),
        (3, 35, "中"),
        (20, 35, "三"),
        (35, 40, "猫"),
        (40, 100, "一"),
        (40, 85, "四"),
        (40, 70, "国"),
        (40, 60, "中"),
        (40, 50, "三"),
    ]);
    let addresses = [0, 1, 2, 10, 25, 37, 90, 80, 65, 55, 45];
    let mut lookup = SymbolLookup::default();
    lookup.insert_symbols(first);
    lookup.insert_symbols(second);
    assert_eq!(query(&lookup, &addresses), expected);
}

#[test]
fn should_trim_cake_to_top_symbol() {
    let first = symbols(&[
        (0, 100, "一"),
        (1, 85, "四"),
        (2, 70, "国"),
        (3, 60, "中"),
        (4, 60, "五"),
        (20, 50, "三"),
    ]);
    let second = symbols(&[(0, 30, "猫"), (40, 100, "糞")]);
    let expected = symbols(&[(0, 30, "猫"), (30, 40, "三"), (40, 100, "糞")]);
    let addresses = [10, 37, 80];
    let mut lookup = SymbolLookup::default();
    lookup.insert_symbols(first);
    lookup.insert_symbols(second);
    assert_eq!(query(&lookup, &addresses), expected);
}

#[test]
fn should_trim_and_overshadow() {
    let first = symbols(&[
        (0, 10, "一"),
        (1, 8, "四"),
        (20, 40, "国"),
        (30, 40, "中"),
        (40, 60, "五"),
        (45, 55, "三"),
    ]);
    let second = symbols(&[(5, 50, "猫"), (10, 15, "糞")]);
    let expected = symbols(&[
        (0, 5, "一"),
        (1, 5, "四"),
        (5, 50, "猫"),
        (10, 15, "糞"),
        (50, 60, "五"),
        (50, 55, "三"),
    ]);
    let addresses = [0, 1, 45, 12, 59, 52];
    let mut lookup = SymbolLookup::default();
    lookup.insert_symbols(first);
    lookup.insert_symbols(second);
    assert_eq!(query(&lookup, &addresses), expected);
}

#[test]
fn should_split_cake() {
    let first = symbols(&[(0, 5, "一"), (1, 4, "四")]);
    let second = symbols(&[(2, 2, "猫")]);
    let expected = symbols(&[
        (0, 2, "一"),
        (1, 2, "四"),
        (2, 2, "猫"),
        (2, 4, "四"),
        (2, 5, "一"),
    ]);
    let addresses = [0, 1, 2, 3, 4];
    let mut lookup = SymbolLookup::default();
    lookup.insert_symbols(first);
    lookup.insert_symbols(second);
    assert_eq!(query(&lookup, &addresses), expected);
}

#[test]
fn should_trim_and_over_shadow_on_abase() {
    let first = symbols(&[
        (0, 100, "一"),
        (1, 8, "四"),
        (20, 40, "国"),
        (30, 40, "中"),
        (40, 60, "五"),
        (45, 55, "三"),
    ]);
    let second = symbols(&[(5, 50, "猫"), (10, 15, "糞")]);
    let expected = symbols(&[
        (0, 5, "一"),
        (1, 5, "四"),
        (5, 50, "猫"),
        (10, 15, "糞"),
        (50, 100, "一"),
        (50, 60, "五"),
        (50, 55, "三"),
    ]);
    let addresses = [0, 3, 5, 14, 60, 59, 51];
    let mut lookup = SymbolLookup::default();
    lookup.insert_symbols(first);
    lookup.insert_symbols(second);
    assert_eq!(query(&lookup, &addresses), expected);
}

#[test]
fn should_split_cake_after_second_tower() {
    let first = symbols(&[
        (0, 100, "一"),
        (1, 8, "四"),
        (20, 40, "国"),
        (30, 40, "中"),
        (50, 60, "五"),
        (50, 55, "三"),
    ]);
    let second = symbols(&[(42, 47, "猫"), (43, 43, "糞")]);
    let expected = symbols(&[
        (0, 42, "一"),
        (1, 8, "四"),
        (20, 40, "国"),
        (30, 40, "中"),
        (42, 47, "猫"),
        (43, 43, "糞"),
        (47, 100, "一"),
        (50, 60, "五"),
        (50, 55, "三"),
    ]);
    let addresses = [41, 5, 25, 35, 45, 43, 48, 58, 50];
    let mut lookup = SymbolLookup::default();
    lookup.insert_symbols(first);
    lookup.insert_symbols(second);
    assert_eq!(query(&lookup, &addresses), expected);
}

#[test]
fn should_have_sliced_symbol_in_name_lookup() {
    let first = symbols(&[
        (20, 40, "国"),
        (30, 40, "中"),
        (50, 60, "五"),
        (50, 55, "三"),
    ]);
    let second = symbols(&[(30, 35, "中")]);
    let expected = symbols(&[
        (20, 30, "国"),
        (30, 35, "中"),
        (35, 40, "中"),
        (50, 60, "五"),
        (50, 55, "三"),
    ]);
    let addresses = [20, 30, 35, 58, 50];
    let mut lookup = SymbolLookup::default();
    lookup.insert_symbols(first);
    lookup.insert_symbols(second);
    assert_eq!(query(&lookup, &addresses), expected);
    let mut found = lookup.by_name(&expected[1].name);
    assert_eq!(found.next().unwrap().as_ref(), &expected[1]);
    assert_eq!(found.next().unwrap().as_ref(), &expected[2]);
}

#[test]
fn should_place_zero_lenght_symbol_before_cake() {
    let first = symbols(&[(50, 100, "一"), (50, 75, "三")]);
    let second = symbols(&[(50, 50, "二")]);
    let expected = symbols(&[(50, 100, "一"), (50, 75, "三"), (50, 50, "二")]);
    let addresses = [90, 55, 50];
    let mut lookup = SymbolLookup::default();
    lookup.insert_symbols(first);
    lookup.insert_symbols(second);
    assert_eq!(query(&lookup, &addresses), expected);
}

#[test]
fn should_cover_zero_lenght_symbol() {
    let first = symbols(&[(50, 50, "二")]);
    let second = symbols(&[(50, 100, "一"), (50, 75, "三")]);
    let expected = symbols(&[(50, 100, "一"), (50, 75, "三")]);
    let addresses = [90, 55];
    let mut lookup = SymbolLookup::default();
    lookup.insert_symbols(first);
    lookup.insert_symbols(second);
    assert_eq!(query(&lookup, &addresses), expected);
    assert!(lookup.by_name("二").next().is_none());
}

#[test]
fn should_zero_lenght_symbol_cut_after_cake() {
    let first = symbols(&[(0, 50, "一"), (10, 50, "三")]);
    let second = symbols(&[(50, 50, "二")]);
    let expected = symbols(&[(0, 50, "一"), (10, 50, "三"), (50, 50, "二")]);
    let addresses = [0, 15, 50];
    let mut lookup = SymbolLookup::default();
    lookup.insert_symbols(first);
    lookup.insert_symbols(second);
    assert_eq!(query(&lookup, &addresses), expected);
}

#[test]
fn should_not_destroy_other_symbol_lookup_cache() {
    let first = symbols(&[
        (0, 100, "一"),
        (1, 85, "四"),
        (2, 70, "国"),
        (80, 85, "猫"),
        (3, 60, "中"),
        (70, 75, "私"),
        (4, 15, "五"),
        (20, 35, "三"),
    ]);
    let second = symbols(&[
        (5, 25, "糞"),
        (40, 82, "二"),
        (50, 55, "ICantSpeekJapaneese"),
        (45, 50, "ICantSpeekKorean"),
    ]);
    let expected = symbols(&[
        (0, 5, "一"),
        (1, 5, "四"),
        (2, 5, "国"),
        (3, 5, "中"),
        (4, 5, "五"),
        (5, 25, "糞"),
        (25, 40, "中"),
        (25, 35, "三"),
        (40, 82, "二"),
        (45, 50, "ICantSpeekKorean"),
        (50, 55, "ICantSpeekJapaneese"),
        (82, 100, "一"),
        (82, 85, "猫"),
    ]);
    let addresses = [0, 1, 2, 3, 4, 10, 37, 30, 42, 47, 53, 90, 83];
    let mut lookup = SymbolLookup::default();
    let mut independent = SymbolLookup::default();
    independent.insert_symbols(first.clone());
    lookup.insert_symbols(first.clone());
    lookup.insert_symbols(second);
    assert_eq!(query(&lookup, &addresses), expected);
    assert_eq!(
        query(
            &independent,
            &first.iter().map(Symbol::start).collect::<Vec<_>>()
        ),
        first
    );
}

#[test]
fn should_perserve_symbol() {
    let first = symbols(&[(0, 100, "一"), (20, 35, "三"), (100, 105, "猫")]);
    let second = symbols(&[(15, 25, "私")]);
    let mut lookup = SymbolLookup::default();
    lookup.insert_symbols(first.clone());
    lookup.insert_symbols(second);
    assert!(lookup.by_name(&first[1].name).next().is_some());
}

#[test]
fn should_merge_overlapping_thumb_symbols() {
    let first = [Symbol::with_thumb_metadata(
        10,
        20,
        "一",
        SymbolType::NotSpecified,
        SymbolBinding::Global,
    )
    .unwrap()];
    let second = [Symbol::with_thumb_metadata(
        5,
        15,
        "悪魔",
        SymbolType::NotSpecified,
        SymbolBinding::Global,
    )
    .unwrap()];
    let mut lookup = SymbolLookup::default();
    lookup.insert_symbols(first);
    lookup.insert_symbols(second);
}

#[test]
fn should_favor_unempty() {
    let input = symbols(&[(0, 10, "Unempty"), (0, 10, ""), (10, 20, "Other")]);
    let expected = symbols(&[(0, 10, "Unempty"), (10, 20, "Other")]);
    let addresses = [0, 10];
    let mut lookup = SymbolLookup::default();
    lookup.insert_symbols(input);
    assert_eq!(query(&lookup, &addresses), expected);
}
