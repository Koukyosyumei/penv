use crate::address_range::{AddressRange, RangeError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Symbol {
    pub name: String,
    range: AddressRange,
    end: u64,
}

impl Symbol {
    /// The end address is exclusive. A zero-length symbol occupies its start address.
    pub fn new(start: u64, end: u64, name: impl Into<String>) -> Result<Self, RangeError> {
        let size = end.checked_sub(start).ok_or(RangeError::InvalidEnd)?;
        let range = AddressRange::by(start, size.max(1))?;
        Ok(Self {
            name: name.into(),
            range,
            end,
        })
    }

    pub fn start(&self) -> u64 {
        self.range.start
    }

    pub fn end(&self) -> u64 {
        self.end
    }

    pub fn contains_address(&self, address: u64) -> bool {
        self.range.contains(address)
    }

    pub fn overlaps(&self, other: &Self) -> bool {
        self.range.intersect(other.range).is_some()
    }

    pub fn contains(&self, other: &Self) -> bool {
        self.range.contains_range(other.range)
    }
}
