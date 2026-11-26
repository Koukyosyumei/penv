use std::{collections::HashMap, ops::RangeInclusive};

use super::{Entry, number};

pub(super) fn is_stm32f0(entries: &[Entry]) -> bool {
    entries
        .iter()
        .any(|entry| entry.model == "Miscellaneous.STM32F0_RCC")
}

pub(super) fn prepare(entries: &mut Vec<Entry>) -> Result<Vec<RangeInclusive<u32>>, String> {
    let mut tags = Vec::new();
    let mut definitions = HashMap::new();
    let mut loaded: Vec<Entry> = Vec::with_capacity(entries.len());
    for entry in entries.drain(..) {
        if entry.name == "sysbus" && entry.model.is_empty() && entry.registration.is_none() {
            read_tags(&entry, &mut tags)?;
        } else if entry.model.is_empty()
            && entry.registration.is_none()
            && let Some(&index) = definitions.get(&entry.name)
        {
            let previous: &mut Entry = &mut loaded[index];
            match previous.model.as_str() {
                "GPIOPort.STM32_GPIOPort" | "Timers.STM32_Timer"
                    if entry.body.iter().all(|(_, text)| text.contains("->")) =>
                {
                    previous.body.extend(entry.body);
                }
                "CRC.STM32_CRC" => {
                    for (_, value) in super::keys(&entry, &["configurablePoly"])? {
                        let value = value
                            .parse::<bool>()
                            .map_err(|_| entry.error("invalid configurablePoly"))?;
                        previous.body.retain(|(_, text)| {
                            text.split_once(':')
                                .is_none_or(|(key, _)| key.trim() != "configurablePoly")
                        });
                        previous
                            .body
                            .push((entry.line, format!("configurablePoly: {value}")));
                    }
                }
                _ => return Err(entry.error("unsupported declaration override")),
            }
        } else {
            definitions.insert(entry.name.clone(), loaded.len());
            loaded.push(entry);
        }
    }
    *entries = loaded;
    Ok(tags)
}

fn read_tags(entry: &Entry, tags: &mut Vec<RangeInclusive<u32>>) -> Result<(), String> {
    let [(_, init), lines @ ..] = entry.body.as_slice() else {
        return Err(entry.error("expected sysbus init block"));
    };
    if init != "init:" {
        return Err(entry.error("expected sysbus init block"));
    }
    for (line, text) in lines {
        let tag = text
            .strip_prefix("Tag <")
            .and_then(|text| text.split_once('>'))
            .and_then(|(range, name)| {
                let label = name.trim().strip_prefix('"')?.strip_suffix('"')?;
                let (start, end) = range.split_once(',')?;
                (!label.contains('"')).then_some((start, end))
            });
        let Some((start, end)) = tag else {
            return Err(entry.error(&format!("unsupported sysbus init command on line {line}")));
        };
        let start = number(start.trim()).map_err(|e| entry.error(&e))?;
        let end = number(end.trim()).map_err(|e| entry.error(&e))?;
        if start > end {
            return Err(entry.error(&format!("reversed tag range on line {line}")));
        }
        tags.push(start..=end);
    }
    Ok(())
}
