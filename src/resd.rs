// A single RESD v1 block on channel 0.
pub struct Samples<T> {
    points: Vec<(u64, T)>,
    end: Option<u64>,
}

impl<T: Copy> Samples<T> {
    pub fn decode<const N: usize>(
        input: &[u8],
        expected_type: u16,
        decode_sample: fn([u8; N]) -> T,
    ) -> Result<Self, &'static str> {
        let mut input = input
            .strip_prefix(b"RESD\x01\0\0\0")
            .ok_or("expected a RESD v1 header")?;
        let mut samples = None;
        while !input.is_empty() {
            let [kind] = take(&mut input)?;
            let sample_type = u16::from_le_bytes(take(&mut input)?);
            let channel = u16::from_le_bytes(take(&mut input)?);
            let size = u64::from_le_bytes(take(&mut input)?);
            let mut block = take_bytes(&mut input, size)?;
            if sample_type != expected_type || channel != 0 {
                continue;
            }
            if samples.is_some() {
                return Err("multiple matching blocks on RESD channel 0 are unsupported");
            }
            let start = u64::from_le_bytes(take(&mut block)?);
            let period = match kind {
                1 => None,
                2 => {
                    let period = u64::from_le_bytes(take(&mut block)?);
                    if period == 0 {
                        return Err("zero RESD sample period");
                    }
                    Some(period)
                }
                _ => return Err("unsupported RESD block type"),
            };
            let metadata_size = u64::from_le_bytes(take(&mut block)?);
            take_bytes(&mut block, metadata_size)?;
            let mut points = Vec::<(u64, T)>::new();
            while !block.is_empty() {
                let offset = match period {
                    Some(period) => period.checked_mul(points.len() as u64),
                    None => Some(u64::from_le_bytes(take(&mut block)?)),
                };
                let timestamp = offset
                    .and_then(|offset| start.checked_add(offset))
                    .ok_or("RESD timestamp overflow")?;
                if points
                    .last()
                    .is_some_and(|&(previous, _)| timestamp < previous)
                {
                    return Err("RESD timestamps must be ordered");
                }
                points.push((timestamp, decode_sample(take(&mut block)?)));
            }
            let end = period
                .map(|period| {
                    period
                        .checked_mul(points.len() as u64)
                        .and_then(|duration| start.checked_add(duration))
                        .ok_or("RESD end timestamp overflow")
                })
                .transpose()?;
            samples = Some(Self { points, end });
        }
        samples
            .filter(|samples| !samples.points.is_empty())
            .ok_or("missing matching RESD channel 0 samples")
    }

    pub fn sample_at(&self, nanoseconds: u64, hold_last: bool) -> Option<T> {
        if !hold_last && self.end.is_some_and(|end| nanoseconds >= end) {
            return None;
        }
        let index = self
            .points
            .partition_point(|&(timestamp, _)| timestamp <= nanoseconds);
        index.checked_sub(1).map(|index| self.points[index].1)
    }
}

fn take<const N: usize>(input: &mut &[u8]) -> Result<[u8; N], &'static str> {
    Ok(take_bytes(input, N as u64)?.try_into().unwrap())
}

fn take_bytes<'a>(input: &mut &'a [u8], size: u64) -> Result<&'a [u8], &'static str> {
    let size = usize::try_from(size).map_err(|_| "invalid RESD field size")?;
    let (head, tail) = input.split_at_checked(size).ok_or("truncated RESD data")?;
    *input = tail;
    Ok(head)
}
