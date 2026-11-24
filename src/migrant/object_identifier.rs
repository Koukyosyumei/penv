use std::{collections::HashMap, rc::Rc};

/// Identity table for one serialization session. Holding strong references
/// prevents a previously assigned allocation address from being reused.
pub struct ObjectIdentifier<T> {
    ids: HashMap<*const T, usize>,
    objects: Vec<Rc<T>>,
}

impl<T> Default for ObjectIdentifier<T> {
    fn default() -> Self {
        Self {
            ids: HashMap::new(),
            objects: Vec::new(),
        }
    }
}

impl<T> ObjectIdentifier<T> {
    pub fn id(&mut self, object: &Rc<T>) -> (usize, bool) {
        use std::collections::hash_map::Entry;
        match self.ids.entry(Rc::as_ptr(object)) {
            Entry::Occupied(existing) => (*existing.get(), false),
            Entry::Vacant(entry) => {
                let id = self.objects.len();
                self.objects.push(Rc::clone(object));
                entry.insert(id);
                (id, true)
            }
        }
    }

    pub fn get(&self, id: usize) -> Option<&Rc<T>> {
        self.objects.get(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_objects_keep_their_id_and_round_trip_by_identity() {
        let mut identifier = ObjectIdentifier::default();
        let first = Rc::new(11);
        let second = Rc::new(11);
        assert_eq!(identifier.id(&first), (0, true));
        assert_eq!(identifier.id(&second), (1, true));
        assert_eq!(identifier.id(&first), (0, false));
        let original = Rc::downgrade(&first);
        drop(first);
        assert!(Rc::ptr_eq(
            &original.upgrade().unwrap(),
            identifier.get(0).unwrap()
        ));
    }
}
