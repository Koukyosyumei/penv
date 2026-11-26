use crate::simple_cache::{MissingGenerator, SimpleCache};

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum Key {
    Object(u32),
    Integer(Option<i32>),
}
use Key::{Integer, Object};
type Cache = SimpleCache<Key, u8, String>;
type One = (u8, fn(&Key) -> String);
type Two = (u8, fn(&Key, &Key) -> String);

const OBJ1: Key = Object(1);
const OBJ2: Key = Object(2);
const G1: One = (1, |_| "System.Object".into());
const G2: One = (2, |key| format!("{} this is unit test", object_hash(key)));
const G3: Two = (3, |key, _| format!("{}System.Object", object_hash(key)));
const G4: Two = (4, |key, _| {
    format!("{}System.Object this is another test", object_hash(key))
});
const NULL_ONE: One = (5, |key| format!("{}{}", has_value(key), text_hash()));
const NULL_TWO: Two = (6, |key, _| {
    format!("{}{}{}", has_value(key), has_value(key), text_hash())
});

fn text_hash() -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    "abcde".hash(&mut hasher);
    hasher.finish()
}

fn object_hash(key: &Key) -> u32 {
    let Object(id) = key else { unreachable!() };
    *id
}

fn has_value(key: &Key) -> &'static str {
    if matches!(key, Integer(Some(_))) {
        "True"
    } else {
        "False"
    }
}

fn counts(cache: &Cache) -> (u64, u64) {
    (cache.hits(), cache.misses())
}

fn hit(cache: &Cache, before: (u64, u64)) {
    assert_eq!(counts(cache), (before.0 + 1, before.1));
}
fn miss(cache: &Cache, before: (u64, u64)) {
    assert_eq!(counts(cache), (before.0, before.1 + 1));
}

#[test]
fn should_not_miss_cache() {
    let mut cache = Cache::default();
    let result = cache.get(OBJ1, Some(G1)).unwrap().clone();
    let before = counts(&cache);
    let after = cache.get(OBJ1, Some(G1)).unwrap().clone();
    hit(&cache, before);
    assert_eq!(result, after);
}

#[test]
fn should_not_miss_cache_with_two_parameters() {
    let mut cache = Cache::default();
    let result = cache.get_two(OBJ1, OBJ2, Some(G3)).unwrap().clone();
    let before = counts(&cache);
    let after = cache.get_two(OBJ1, OBJ2, Some(G3)).unwrap().clone();
    hit(&cache, before);
    assert_eq!(result, after);
}

#[test]
fn should_not_mix_parameters_and_generators() {
    let mut cache = Cache::default();
    for (key, generator) in [(OBJ2, G1), (OBJ2, G2), (OBJ1, G2)] {
        let before = counts(&cache);
        cache.get(key, Some(generator)).unwrap();
        miss(&cache, before);
    }
    for generator in [G3, G4] {
        let before = counts(&cache);
        cache.get_two(OBJ1, OBJ2, Some(generator)).unwrap();
        miss(&cache, before);
    }
}

#[test]
fn should_handle_nulls_as_parameter() {
    let mut cache = Cache::default();
    let result = cache.get(Integer(None), Some(NULL_ONE)).unwrap().clone();
    let before = counts(&cache);
    let after = cache.get(Integer(None), Some(NULL_ONE)).unwrap().clone();
    assert_eq!(result, after);
    hit(&cache, before);
}

#[test]
fn should_handle_nulls_as_parameters() {
    let mut cache = Cache::default();
    let result = cache
        .get_two(Integer(None), Integer(None), Some(NULL_TWO))
        .unwrap()
        .clone();
    let before = counts(&cache);
    let after = cache
        .get_two(Integer(None), Integer(None), Some(NULL_TWO))
        .unwrap()
        .clone();
    assert_eq!(result, after);
    hit(&cache, before);
}

#[test]
fn should_handle_nullable_as_parameter() {
    let mut cache = Cache::default();
    let result = cache.get(Integer(None), Some(NULL_ONE)).unwrap().clone();
    let before = counts(&cache);
    let after = cache.get(Integer(Some(0)), Some(NULL_ONE)).unwrap().clone();
    assert_ne!(result, after);
    miss(&cache, before);
}

#[test]
fn should_handle_nullable_as_parameters() {
    let mut cache = Cache::default();
    let mut previous = cache
        .get_two(Integer(None), Integer(None), Some(NULL_TWO))
        .unwrap()
        .clone();
    for (first, second) in [(Some(0), None), (None, Some(0))] {
        let before = counts(&cache);
        let result = cache
            .get_two(Integer(first), Integer(second), Some(NULL_TWO))
            .unwrap()
            .clone();
        assert_ne!(previous, result);
        miss(&cache, before);
        previous = result;
    }
}

#[test]
fn should_not_handle_null_as_a_generator() {
    let mut cache = Cache::default();
    assert_eq!(cache.get(OBJ1, None), Err(MissingGenerator));
    assert_eq!(cache.get_two(OBJ1, OBJ2, None), Err(MissingGenerator));
}

#[test]
fn should_not_mix_order_of_parameteres() {
    let mut cache = Cache::default();
    let result = cache.get_two(OBJ1, OBJ2, Some(G3)).unwrap().clone();
    let before = counts(&cache);
    let after = cache.get_two(OBJ2, OBJ1, Some(G3)).unwrap().clone();
    assert_ne!(before.1, cache.misses());
    assert_eq!(before.0, cache.hits());
    assert_ne!(result, after);
}

#[test]
fn should_clear_cache() {
    let mut cache = Cache::default();
    for (key, generator) in [(OBJ1, G1), (OBJ2, G1), (OBJ1, G2), (OBJ2, G2)] {
        cache.get(key, Some(generator)).unwrap();
    }
    assert_eq!(cache.len(), 4);
    cache.clear();
    assert_eq!(cache.len(), 0);
}
