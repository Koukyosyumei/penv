use std::collections::{HashMap, hash_map::Entry};
use std::hash::Hash;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MissingGenerator;

#[derive(Hash, Eq, PartialEq)]
enum Arguments<K> {
    One(K),
    Two(K, K),
}

pub struct SimpleCache<K, G, V> {
    entries: HashMap<(G, Arguments<K>), V>,
    hits: u64,
    misses: u64,
}

impl<K: Eq + Hash, G: Eq + Hash, V> Default for SimpleCache<K, G, V> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            hits: 0,
            misses: 0,
        }
    }
}

impl<K: Eq + Hash, G: Eq + Hash, V> SimpleCache<K, G, V> {
    pub fn get(
        &mut self,
        argument: K,
        generator: Option<(G, fn(&K) -> V)>,
    ) -> Result<&V, MissingGenerator> {
        let (id, generate) = generator.ok_or(MissingGenerator)?;
        Ok(self.lookup(id, Arguments::One(argument), |args| {
            let Arguments::One(argument) = args else {
                unreachable!()
            };
            generate(argument)
        }))
    }

    pub fn get_two(
        &mut self,
        first: K,
        second: K,
        generator: Option<(G, fn(&K, &K) -> V)>,
    ) -> Result<&V, MissingGenerator> {
        let (id, generate) = generator.ok_or(MissingGenerator)?;
        Ok(self.lookup(id, Arguments::Two(first, second), |args| {
            let Arguments::Two(first, second) = args else {
                unreachable!()
            };
            generate(first, second)
        }))
    }

    fn lookup(
        &mut self,
        id: G,
        arguments: Arguments<K>,
        generate: impl FnOnce(&Arguments<K>) -> V,
    ) -> &V {
        match self.entries.entry((id, arguments)) {
            Entry::Occupied(entry) => {
                self.hits += 1;
                entry.into_mut()
            }
            Entry::Vacant(entry) => {
                self.misses += 1;
                let value = generate(&entry.key().1);
                entry.insert(value)
            }
        }
    }

    pub fn hits(&self) -> u64 {
        self.hits
    }
    pub fn misses(&self) -> u64 {
        self.misses
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn clear(&mut self) {
        self.hits = 0;
        self.misses = 0;
        self.entries.clear();
    }
}
