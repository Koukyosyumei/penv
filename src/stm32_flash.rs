pub struct KeyLock {
    keys: [u32; 2],
    pub locked: bool,
    first_key: bool,
}

impl KeyLock {
    pub fn new(keys: [u32; 2]) -> Self {
        Self {
            keys,
            locked: true,
            first_key: false,
        }
    }

    pub fn consume(&mut self, value: u32) {
        if !self.locked {
            return;
        }
        if self.first_key && value == self.keys[1] {
            self.locked = false;
        }
        self.first_key = value == self.keys[0];
    }

    pub fn lock(&mut self) {
        self.locked = true;
        self.first_key = false;
    }
}
