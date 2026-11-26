//! Little-endian register parts with independent read and write tracking.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    UnsupportedWidth,
    IncorrectLength,
    PartOutOfRange,
    Capacity,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Untouched,
    Written,
    Read,
}

pub struct TrackedRegisterBytes {
    bytes: Vec<u8>,
    states: Vec<State>,
}

impl TrackedRegisterBytes {
    pub fn new(parts: usize, access_width: usize) -> Result<Self, Error> {
        if access_width != 4 {
            return Err(Error::UnsupportedWidth);
        }
        let size = parts.checked_mul(4).ok_or(Error::Capacity)?;
        let mut bytes = Vec::new();
        let mut states = Vec::new();
        bytes.try_reserve_exact(size).map_err(|_| Error::Capacity)?;
        states
            .try_reserve_exact(parts)
            .map_err(|_| Error::Capacity)?;
        bytes.resize(size, 0);
        states.resize(parts, State::Untouched);
        Ok(Self { bytes, states })
    }

    pub fn reset(&mut self) {
        self.bytes.fill(0);
        self.states.fill(State::Untouched);
    }

    pub fn set_part(&mut self, part: usize, value: u32) -> Result<(), Error> {
        if part >= self.states.len() {
            return Err(Error::PartOutOfRange);
        }
        if self.all_written() {
            self.states.fill(State::Untouched);
        }
        let offset = part * 4;
        self.bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        self.states[part] = State::Written;
        Ok(())
    }

    pub fn part(&mut self, part: usize) -> Result<u32, Error> {
        let state = self.states.get_mut(part).ok_or(Error::PartOutOfRange)?;
        *state = State::Read;
        let offset = part * 4;
        Ok(u32::from_le_bytes(
            self.bytes[offset..offset + 4].try_into().unwrap(),
        ))
    }

    pub fn set_array(&mut self, bytes: Vec<u8>, track: bool) -> Result<(), Error> {
        if bytes.len() != self.bytes.len() {
            return Err(Error::IncorrectLength);
        }
        self.bytes = bytes;
        if track {
            self.states.fill(State::Written);
        }
        Ok(())
    }

    pub fn retrieve(&mut self, track: bool) -> &[u8] {
        if !self.all_written() {
            return &[];
        }
        if track {
            self.states.fill(State::Read);
        }
        &self.bytes
    }

    pub fn all_written(&self) -> bool {
        self.states.iter().all(|&state| state == State::Written)
    }

    pub fn all_read(&self) -> bool {
        self.states.iter().all(|&state| state == State::Read)
    }
}
