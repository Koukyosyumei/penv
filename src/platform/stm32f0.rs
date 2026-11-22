use std::{collections::HashMap, ops::RangeInclusive};

use super::{Entry, number};

pub(super) fn prepare(entries: &mut Vec<Entry>) -> Result<Vec<RangeInclusive<u32>>, String> {
    if !entries
        .iter()
        .any(|entry| entry.model == "Miscellaneous.STM32F0_RCC")
    {
        return Err("--stm32f0-console requires a platform with an STM32F0 RCC".into());
    }
    let omitted: HashMap<_, _> = entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.model.as_str(),
                "I2C.STM32F7_I2C"
                    | "CAN.STMCAN"
                    | "Timers.STM32F4_RTC"
                    | "Miscellaneous.LED"
                    | "Miscellaneous.Button"
            )
        })
        .map(|entry| (entry.name.clone(), entry.model.clone()))
        .collect();
    let mut skipped = Vec::new();
    let mut tags = Vec::new();
    let mut gpio_wiring = false;
    entries.retain_mut(|entry| {
        if omitted.contains_key(&entry.name) {
            if !entry.model.is_empty() {
                skipped.push(format!("{} ({})", entry.name, entry.model));
            }
            return false;
        }
        match entry.model.as_str() {
            "GPIOPort.STM32_GPIOPort" => {
                entry.body.retain(|(_, text)| {
                    if text.contains("->") {
                        gpio_wiring = true;
                        return false;
                    }
                    !text.starts_with("invertedAFPins:")
                });
            }
            "Timers.STM32_Timer" => {
                entry.body.retain_mut(|(_, text)| {
                    let Some((source, targets)) = text.split_once("->") else {
                        return true;
                    };
                    let remaining: Vec<_> = targets
                        .split('|')
                        .map(str::trim)
                        .filter(|target| {
                            let Some((name, _)) = target.split_once('@') else {
                                return true;
                            };
                            if name.contains('#') {
                                gpio_wiring = true;
                                return false;
                            }
                            true
                        })
                        .collect();
                    if remaining.is_empty() {
                        return false;
                    }
                    *text = format!("{} -> {}", source.trim(), remaining.join(" | "));
                    true
                });
            }
            _ => {}
        }
        true
    });
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
                "GPIOPort.STM32_GPIOPort"
                    if entry.body.iter().all(|(_, text)| text.contains("->")) =>
                {
                    gpio_wiring |= !entry.body.is_empty();
                }
                "Timers.STM32_Timer"
                    if entry.body.iter().all(|(_, text)| {
                        text.split_once("->").is_some_and(|(_, locations)| {
                            locations.split('|').all(|location| location.contains('#'))
                        })
                    }) =>
                {
                    gpio_wiring |= !entry.body.is_empty();
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
    if !skipped.is_empty() {
        eprintln!("penv: STM32F0 console mode omits {}", skipped.join(", "));
    }
    if gpio_wiring {
        eprintln!("penv: STM32F0 console mode omits GPIO external and alternate-function wiring");
    }
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
