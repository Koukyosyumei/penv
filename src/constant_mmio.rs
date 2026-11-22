use crate::platform::{Entry, number};

pub struct ConstantMmio {
    pub base: u32,
    pub size: u32,
    values: Vec<(u32, u32)>,
    warn_unhandled: bool,
}

impl ConstantMmio {
    pub fn from_platform(entry: &Entry) -> Result<Self, String> {
        let mut size = None;
        let mut script = None;
        let mut initable = None;
        for (_, property) in &entry.body {
            let (key, value) = property
                .split_once(':')
                .ok_or("expected Python peripheral property")?;
            match key.trim() {
                "size" if size.is_none() => size = Some(number(value.trim())?),
                "initable" if initable.is_none() && value.trim() == "false" => {
                    initable = Some(false)
                }
                "script" if script.is_none() => script = Some(value),
                _ => return Err(format!("unsupported Python peripheral property {key:?}")),
            }
        }
        let size = size
            .filter(|&size| size != 0)
            .ok_or("missing Python peripheral size")?;
        let base = entry.sysbus()?.ok_or("missing Python peripheral address")?;
        let script = script.ok_or("missing Python peripheral script")?;
        let (mut values, warn_unhandled) = parse(script, size)?;
        values.sort_unstable_by_key(|&(offset, _)| offset);
        if values.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err("duplicate Python peripheral offset".into());
        }
        Ok(Self {
            base,
            size,
            values,
            warn_unhandled,
        })
    }

    pub fn offset(&self, address: u32, width: usize) -> Option<u32> {
        let offset = address.checked_sub(self.base)?;
        (u64::from(offset) + width as u64 <= u64::from(self.size)).then_some(offset)
    }

    pub fn read(&self, offset: u32, width: usize) -> u32 {
        let value = self
            .values
            .binary_search_by_key(&offset, |&(offset, _)| offset)
            .map_or_else(
                |_| {
                    self.warn(offset);
                    0
                },
                |index| self.values[index].1,
            );
        value & (u32::MAX >> (32 - width * 8))
    }

    pub fn write(&self, offset: u32) {
        if self
            .values
            .binary_search_by_key(&offset, |&(offset, _)| offset)
            .is_err()
        {
            self.warn(offset);
        }
    }

    fn warn(&self, offset: u32) {
        if self.warn_unhandled {
            eprintln!(
                "penv: peripheral at {:#010x}: unhandled access to offset {offset:#x}",
                self.base
            );
        }
    }
}

fn parse(script: &str, size: u32) -> Result<(Vec<(u32, u32)>, bool), String> {
    let mut lines = script
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|line| !line.is_empty());
    let mut registers = Vec::new();
    let mut warn = false;
    while let Some(condition) = lines.next() {
        if condition == "else:" && !registers.is_empty() {
            let line = lines.next().ok_or("missing Python peripheral fallback")?;
            if line != "self.WarningLog(\"Unhandled access to offset 0x{0:X}\", request.Offset)"
                || lines.next().is_some()
            {
                return Err("unsupported Python peripheral fallback".into());
            }
            warn = true;
            break;
        }
        let prefix = if registers.is_empty() { "if " } else { "elif " };
        let condition = condition
            .strip_prefix(prefix)
            .ok_or("expected an offset condition in Python peripheral")?;
        let mut offsets = Vec::new();
        if let Some(address) = condition
            .strip_prefix("request.Offset == ")
            .and_then(|line| line.strip_suffix(':'))
        {
            offsets.push(number(address.trim())?);
        } else if condition == "request.Offset in (" {
            loop {
                let line = lines
                    .next()
                    .ok_or("unterminated Python peripheral offset list")?;
                if line == "):" {
                    break;
                }
                offsets.push(number(line.strip_suffix(',').unwrap_or(line).trim())?);
            }
        }
        if offsets.is_empty() || offsets.iter().any(|&offset| offset >= size) {
            return Err("unsupported or out-of-range Python peripheral offset".into());
        }
        let assignment = lines
            .next()
            .and_then(|line| line.strip_prefix("request.Value = "))
            .ok_or("expected constant Python peripheral response")?;
        let value = number(assignment.trim())?;
        registers.extend(offsets.into_iter().map(|offset| (offset, value)));
    }
    if registers.is_empty() {
        return Err("expected constant Python peripheral responses".into());
    }
    Ok((registers, warn))
}
