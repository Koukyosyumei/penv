#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AddressRange {
    pub start: u64,
    pub end: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RangeError {
    InvalidEnd,
    FullAddressSpace,
    InvalidSize(u64),
    InvalidShift,
}

impl AddressRange {
    pub fn by(start: u64, size: u64) -> Result<Self, RangeError> {
        let end = size
            .checked_sub(1)
            .and_then(|last| start.checked_add(last))
            .ok_or(RangeError::InvalidSize(size))?;
        Ok(Self { start, end })
    }

    pub fn to(start: u64, end: u64) -> Result<Self, RangeError> {
        if (start, end) == (0, u64::MAX) {
            return Err(RangeError::FullAddressSpace);
        }
        if start > end {
            return Err(RangeError::InvalidEnd);
        }
        Ok(Self { start, end })
    }

    pub fn contains(self, address: u64) -> bool {
        (self.start..=self.end).contains(&address)
    }

    pub fn contains_range(self, other: Self) -> bool {
        self.start <= other.start && other.end <= self.end
    }

    pub fn intersect(self, other: Self) -> Option<Self> {
        let start = self.start.max(other.start);
        let end = self.end.min(other.end);
        (start <= end).then_some(Self { start, end })
    }

    pub fn shift_by(self, offset: i64) -> Result<Self, RangeError> {
        let start = self
            .start
            .checked_add_signed(offset)
            .ok_or(RangeError::InvalidShift)?;
        let end = self
            .end
            .checked_add_signed(offset)
            .ok_or(RangeError::InvalidShift)?;
        Ok(Self { start, end })
    }

    pub fn subtract(self, other: Self) -> impl Iterator<Item = Self> {
        let Some(overlap) = self.intersect(other) else {
            return [Some(self), None].into_iter().flatten();
        };
        [
            (self.start < overlap.start).then(|| Self {
                start: self.start,
                end: overlap.start - 1,
            }),
            (overlap.end < self.end).then(|| Self {
                start: overlap.end + 1,
                end: self.end,
            }),
        ]
        .into_iter()
        .flatten()
    }
}

#[derive(Default)]
pub struct MinimalRanges {
    ranges: Vec<AddressRange>,
}

impl MinimalRanges {
    pub fn add(&mut self, range: AddressRange) -> Result<(), RangeError> {
        let first = self
            .ranges
            .partition_point(|item| item.end.saturating_add(1) < range.start);
        let last = self
            .ranges
            .partition_point(|item| item.start <= range.end.saturating_add(1));
        let covered = &self.ranges[first..last];
        let start = covered
            .first()
            .map_or(range.start, |item| item.start.min(range.start));
        let end = covered
            .last()
            .map_or(range.end, |item| item.end.max(range.end));
        let merged = AddressRange::to(start, end)?;
        self.ranges.splice(first..last, [merged]);
        Ok(())
    }

    pub fn remove(&mut self, range: AddressRange) -> bool {
        let first = self.ranges.partition_point(|item| item.end < range.start);
        let last = self.ranges.partition_point(|item| item.start <= range.end);
        if first == last {
            return false;
        }
        let remaining: Vec<_> = self.ranges[first..last]
            .iter()
            .flat_map(|item| item.subtract(range))
            .collect();
        self.ranges.splice(first..last, remaining);
        true
    }

    pub fn ranges(&self) -> &[AddressRange] {
        &self.ranges
    }
}
