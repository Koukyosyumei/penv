use std::sync::{Arc, Mutex, Weak};

type Pair<L, R> = (Weak<L>, Weak<R>);

pub struct WeakMultiTable<L, R> {
    pairs: Mutex<Vec<Pair<L, R>>>,
}

impl<L, R> Default for WeakMultiTable<L, R> {
    fn default() -> Self {
        Self {
            pairs: Mutex::new(Vec::new()),
        }
    }
}

impl<L: Eq, R: Eq> WeakMultiTable<L, R> {
    pub fn add(&self, left: &Arc<L>, right: &Arc<R>) {
        self.pairs
            .lock()
            .unwrap()
            .push((Arc::downgrade(left), Arc::downgrade(right)));
    }

    pub fn for_left(&self, left: &L) -> Vec<Arc<R>> {
        distinct(
            self.snapshot()
                .into_iter()
                .filter_map(|(key, value)| (*key == *left).then_some(value)),
        )
    }

    pub fn for_right(&self, right: &R) -> Vec<Arc<L>> {
        distinct(
            self.snapshot()
                .into_iter()
                .filter_map(|(value, key)| (*key == *right).then_some(value)),
        )
    }

    pub fn remove_pair(&self, left: &L, right: &R) {
        self.pairs.lock().unwrap().retain(|(l, r)| {
            l.upgrade()
                .zip(r.upgrade())
                .is_some_and(|(l, r)| *l != *left || *r != *right)
        });
    }

    fn snapshot(&self) -> Vec<(Arc<L>, Arc<R>)> {
        let mut snapshot = Vec::new();
        self.pairs.lock().unwrap().retain(|(left, right)| {
            if let Some(pair) = left.upgrade().zip(right.upgrade()) {
                snapshot.push(pair);
                true
            } else {
                false
            }
        });
        snapshot
    }
}

fn distinct<T: Eq>(items: impl Iterator<Item = Arc<T>>) -> Vec<Arc<T>> {
    let mut result = Vec::new();
    for item in items {
        if !result.contains(&item) {
            result.push(item);
        }
    }
    result
}
