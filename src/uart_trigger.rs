use std::{cell::Cell, rc::Rc};

pub struct UartTrigger {
    pending: usize,
    text: Vec<u8>,
    suffix: Vec<u8>,
    pub fired: Rc<Cell<bool>>,
}

impl UartTrigger {
    pub fn new(text: Vec<u8>, pending: usize) -> Self {
        debug_assert!(!text.is_empty() && pending != 0);
        Self {
            pending,
            text,
            suffix: Vec::new(),
            fired: Rc::default(),
        }
    }

    /// Return true once the required occurrence has been received.
    pub fn receive(&mut self, bytes: &[u8]) -> bool {
        self.suffix.extend_from_slice(bytes);
        for window in self.suffix.windows(self.text.len()) {
            if window == self.text {
                self.pending -= 1;
                if self.pending == 0 {
                    self.fired.set(true);
                    return true;
                }
            }
        }
        let discard = self.suffix.len().saturating_sub(self.text.len() - 1);
        self.suffix.drain(..discard);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uart_trigger_counts_occurrences_across_chunks() {
        let mut trigger = UartTrigger::new(b"H2U ".to_vec(), 3);
        assert!(!trigger.receive(b"H2"));
        assert!(!trigger.receive(b"U H2U H"));
        assert!(!trigger.fired.get());
        assert!(trigger.receive(b"2U "));
        assert!(trigger.fired.get());
    }
}
