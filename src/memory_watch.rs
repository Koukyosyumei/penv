use std::{cmp::Ordering, str::FromStr};

#[derive(Clone, Copy)]
pub(crate) enum Comparison {
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

impl FromStr for Comparison {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "eq" => Ok(Self::Equal),
            "ne" => Ok(Self::NotEqual),
            "lt" => Ok(Self::Less),
            "le" => Ok(Self::LessOrEqual),
            "gt" => Ok(Self::Greater),
            "ge" => Ok(Self::GreaterOrEqual),
            _ => Err("unknown memory comparison"),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Group {
    Any,
    All,
    Independently,
}

impl FromStr for Group {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "any" => Ok(Self::Any),
            "all" => Ok(Self::All),
            "independently" => Ok(Self::Independently),
            _ => Err("unknown memory comparison group"),
        }
    }
}

#[derive(Clone)]
pub(crate) struct MemoryWatch {
    targets: Vec<(u64, bool)>,
    width: usize,
    signed: bool,
    comparison: Comparison,
    expected: u64,
    group: Group,
}

impl MemoryWatch {
    pub(crate) fn new(
        addresses: Vec<u64>,
        width: usize,
        signed: bool,
        comparison: Comparison,
        expected: u64,
        group: Group,
    ) -> Result<Self, &'static str> {
        if addresses.is_empty()
            || !matches!(width, 1 | 2 | 4 | 8)
            || addresses
                .iter()
                .any(|address| address.checked_add(width as u64 - 1).is_none())
        {
            return Err("invalid memory watch addresses or access width");
        }
        Ok(Self {
            targets: addresses
                .into_iter()
                .map(|address| (address, false))
                .collect(),
            width,
            signed,
            comparison,
            expected,
            group,
        })
    }

    pub(crate) fn store(&mut self, address: u64, width: usize, value: u64) {
        for (target, matched) in &mut self.targets {
            let Some(offset) = target
                .checked_sub(address)
                .and_then(|offset| usize::try_from(offset).ok())
            else {
                continue;
            };
            if width
                .checked_sub(self.width)
                .is_none_or(|maximum| offset > maximum)
                || (matches!(self.group, Group::Independently) && *matched)
            {
                continue;
            }
            let value = value >> (offset * 8);
            let shift = 64 - (self.width * 8);
            let order = if self.signed {
                ((value << shift) as i64).cmp(&((self.expected << shift) as i64))
            } else {
                (value << shift).cmp(&(self.expected << shift))
            };
            *matched = match self.comparison {
                Comparison::Equal => order == Ordering::Equal,
                Comparison::NotEqual => order != Ordering::Equal,
                Comparison::Less => order == Ordering::Less,
                Comparison::LessOrEqual => order != Ordering::Greater,
                Comparison::Greater => order == Ordering::Greater,
                Comparison::GreaterOrEqual => order != Ordering::Less,
            };
        }
    }

    pub(crate) fn matched(&self) -> bool {
        if matches!(self.group, Group::Any) {
            self.targets.iter().any(|&(_, matched)| matched)
        } else {
            self.targets.iter().all(|&(_, matched)| matched)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_and_independent_watches_distinguish_overwritten_values() {
        for (group, expected) in [(Group::All, false), (Group::Independently, true)] {
            let mut watch =
                MemoryWatch::new(vec![4, 8], 1, true, Comparison::Less, 0, group).unwrap();
            watch.store(0, 8, 0xff_0000_0000);
            watch.store(4, 4, 0);
            watch.store(8, 4, 0xff);
            assert_eq!(watch.matched(), expected);
        }
    }
}
