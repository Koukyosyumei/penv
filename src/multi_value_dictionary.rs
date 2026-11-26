/// Per-key collections with Migrant/Renode's singleton-first iteration order.
pub struct MultiValueDictionary<K, V> {
    single: Vec<(K, V)>,
    multiple: Vec<(K, Vec<V>)>,
}

impl<K, V> Default for MultiValueDictionary<K, V> {
    fn default() -> Self {
        Self {
            single: Vec::new(),
            multiple: Vec::new(),
        }
    }
}

impl<K: Eq, V: Eq> MultiValueDictionary<K, V> {
    fn single_index(&self, key: &K) -> Option<usize> {
        self.single
            .iter()
            .position(|(candidate, _)| candidate == key)
    }

    fn multiple_index(&self, key: &K) -> Option<usize> {
        self.multiple
            .iter()
            .position(|(candidate, _)| candidate == key)
    }

    pub fn add(&mut self, key: K, value: V) {
        if let Some(index) = self.multiple_index(&key) {
            self.multiple[index].1.push(value);
        } else if let Some(index) = self.single_index(&key) {
            let (key, previous) = self.single.remove(index);
            self.multiple.push((key, vec![previous, value]));
        } else {
            self.single.push((key, value));
        }
    }

    pub fn add_range(&mut self, key: K, values: impl IntoIterator<Item = V>) {
        if let Some(index) = self.multiple_index(&key) {
            self.multiple[index].1.extend(values);
        } else {
            let previous = self
                .single_index(&key)
                .map(|index| self.single.remove(index).1);
            self.multiple
                .push((key, previous.into_iter().chain(values).collect()));
        }
    }

    pub fn get(&self, key: &K) -> Option<&[V]> {
        self.single_index(key)
            .map(|index| std::slice::from_ref(&self.single[index].1))
            .or_else(|| {
                self.multiple_index(key)
                    .map(|index| self.multiple[index].1.as_slice())
            })
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.get(key).is_some()
    }

    pub fn contains_value(&self, value: &V) -> bool {
        self.values().any(|candidate| candidate == value)
    }

    pub fn contains(&self, key: &K, value: &V) -> bool {
        self.get(key).is_some_and(|items| items.contains(value))
    }

    pub fn clear(&mut self) {
        self.single.clear();
        self.multiple.clear();
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.single
            .iter()
            .map(|(_, value)| value)
            .chain(self.multiple.iter().flat_map(|(_, values)| values))
    }
}
