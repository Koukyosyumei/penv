// RESD v1 illuminance samples on channel 0.
pub struct IlluminanceSamples(Vec<(u64, u32)>);

impl IlluminanceSamples {
    pub fn decode(input: &[u8]) -> Result<Self, &'static str> {
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
            if sample_type != 0x0a || channel != 0 {
                continue;
            }
            if samples.is_some() {
                return Err("multiple illuminance blocks on RESD channel 0 are unsupported");
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
                _ => return Err("unsupported RESD illuminance block type"),
            };
            let metadata_size = u64::from_le_bytes(take(&mut block)?);
            take_bytes(&mut block, metadata_size)?;
            let mut points = Vec::<(u64, u32)>::new();
            while !block.is_empty() {
                let offset = match period {
                    Some(period) => period.checked_mul(points.len() as u64),
                    None => Some(u64::from_le_bytes(take(&mut block)?)),
                };
                let timestamp = offset
                    .and_then(|offset| start.checked_add(offset))
                    .ok_or("RESD illuminance timestamp overflow")?;
                if points
                    .last()
                    .is_some_and(|&(previous, _)| timestamp < previous)
                {
                    return Err("RESD illuminance timestamps must be ordered");
                }
                points.push((timestamp, u32::from_le_bytes(take(&mut block)?)));
            }
            samples = Some(points);
        }
        samples
            .filter(|samples| !samples.is_empty())
            .map(Self)
            .ok_or("missing RESD channel 0 illuminance samples")
    }

    pub fn lux_at(&self, nanoseconds: u64) -> f64 {
        let index = self
            .0
            .partition_point(|&(timestamp, _)| timestamp <= nanoseconds);
        let value = index.checked_sub(1).map_or(0, |i| self.0[i].1);
        f64::from(value) / 1000.0
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
