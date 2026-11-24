use std::collections::VecDeque;

pub struct CircularBuffer<T> {
    items: VecDeque<T>,
    capacity: usize,
}

impl<T> CircularBuffer<T> {
    pub fn new(capacity: usize) -> Self {
        assert_ne!(capacity, 0);
        Self {
            items: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn enqueue(&mut self, value: T) -> bool {
        let overflow = self.len() == self.capacity;
        if overflow {
            self.try_dequeue();
        }
        self.items.push_back(value);
        !overflow
    }

    pub fn try_dequeue(&mut self) -> Option<T> {
        self.items.pop_front()
    }

    pub fn try_peek(&self) -> Option<&T> {
        self.items.front()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn is_wrapped(&self) -> bool {
        !self.items.as_slices().1.is_empty()
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.items.iter()
    }

    pub fn copy_to(&self, output: &mut [T], index: usize)
    where
        T: Clone,
    {
        if self.is_empty() {
            return;
        }
        for (destination, value) in output[index..][..self.len()].iter_mut().zip(self.iter()) {
            destination.clone_from(value);
        }
    }
}
