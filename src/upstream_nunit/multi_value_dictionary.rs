use crate::multi_value_dictionary::MultiValueDictionary;

const FIRST: [&str; 4] = ["test1", "test2", "test3", "test4"];
const SECOND: [&str; 4] = ["dojpa1", "dojpa2", "dojpa3", "dojpa4"];

fn two_keys() -> MultiValueDictionary<&'static str, &'static str> {
    let mut dict = MultiValueDictionary::default();
    dict.add_range("key2", SECOND);
    dict.add_range("key1", FIRST);
    dict
}

#[test]
fn should_add_range_and_retrieve_via_iterator() {
    let dict = two_keys();
    assert!(dict.values().copied().eq(SECOND.into_iter().chain(FIRST)));
}

fn single_writes() -> MultiValueDictionary<&'static str, &'static str> {
    let mut dict = MultiValueDictionary::default();
    dict.add("key", "value");
    dict
}

#[test]
fn should_add_and_try_get_value() {
    let mut dict = single_writes();
    let first = dict.get(&"key").unwrap().first().unwrap();
    assert!(std::ptr::eq::<str>(*first, "value"));
    for value in FIRST {
        dict.add("key2", value);
    }
    assert_eq!(dict.get(&"key2"), Some(FIRST.as_slice()));
}

#[test]
fn should_not_get_value() {
    let mut dict = single_writes();
    for value in FIRST {
        dict.add("key2", value);
    }
    assert_eq!(dict.get(&"海亀"), None);
}

#[test]
fn should_clear() {
    let mut dict = two_keys();
    dict.add("key3", "海亀");
    dict.clear();
    for key in ["海亀", "key1", "key2", "key3"] {
        assert_eq!(dict.get(&key), None);
    }
}

#[test]
fn should_contain_value() {
    let mut dict = MultiValueDictionary::default();
    dict.add_range(100, SECOND);
    dict.add_range(-5, FIRST);
    dict.add(1337, "海亀");
    for value in ["dojpa2", "海亀", "test4", "海亀", "test1", "dojpa4"] {
        assert!(dict.contains_value(&value));
    }
}

#[test]
fn should_contain_key() {
    let mut dict = two_keys();
    dict.add("key3", "海亀");
    for key in ["key1", "key3", "key2", "key1", "key2", "key3"] {
        assert!(dict.contains_key(&key));
    }
}

#[test]
fn should_contain_key_value() {
    let mut dict = two_keys();
    dict.add("key3", "海亀");
    for (key, value) in [("key2", "dojpa4"), ("key3", "海亀"), ("key1", "test1")] {
        assert!(dict.contains(&key, &value));
    }
}
