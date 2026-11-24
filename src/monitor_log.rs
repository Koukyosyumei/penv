//! Log capture for one Robot monitor session on the emulation thread.
use crate::machine::{Machine, Stop};
use std::{cell::RefCell, collections::VecDeque, io};

struct Capture {
    limit: usize,
    entries: VecDeque<(i8, String)>,
    minimum_level: i8,
}

thread_local! {
    static CAPTURE: RefCell<Option<Capture>> = const { RefCell::new(None) };
}

pub(crate) fn record(level: i8, message: &str) {
    CAPTURE.with_borrow_mut(|capture| {
        if let Some(capture) = capture
            && level >= capture.minimum_level
        {
            if capture.entries.len() == capture.limit {
                capture.entries.pop_front();
            }
            capture.entries.push_back((level, message.to_owned()));
        }
    });
}

pub(crate) fn set_level(level: i8) {
    CAPTURE.with_borrow_mut(|capture| {
        if let Some(capture) = capture {
            capture.minimum_level = level;
        }
    });
}

pub(crate) fn attach(limit: usize) -> Result<(), String> {
    if limit == 0 || limit > 1_000_000 {
        return Err("log capacity must be between 1 and 1000000".into());
    }
    CAPTURE.with_borrow_mut(|capture| {
        *capture = Some(Capture {
            limit,
            entries: VecDeque::new(),
            minimum_level: 1,
        })
    });
    Ok(())
}

pub(crate) fn handle(
    machine: &mut Machine,
    command: &str,
) -> Option<Result<(&'static str, String), String>> {
    let words: Vec<_> = command.split_ascii_whitespace().collect();
    match words.as_slice() {
        ["AttachMonitorLog", limit] => Some((|| {
            attach(limit.parse().map_err(|_| "invalid log capacity")?)?;
            Ok(("OK", String::new()))
        })()),
        ["WaitForMonitorLog", steps, kind, keep, level, pattern] => Some((|| {
            let ms = steps
                .parse::<u64>()
                .map_err(|_| "invalid timeout milliseconds")?;
            let steps = ms
                .checked_mul(u64::from(machine.bus.step_hz()))
                .ok_or("monitor log timeout exceeds virtual-time range")?
                / 1000;
            let keep = match *keep {
                "keep" => true,
                "consume" => false,
                _ => return Err("invalid log retention option".into()),
            };
            let level = if *level == "any" {
                None
            } else {
                Some(level.parse::<i8>().map_err(|_| "invalid log severity")?)
            };
            let pattern = decode(pattern)?;
            let expression = match *kind {
                "literal" => None,
                "regex" => Some(
                    regex::Regex::new(&pattern)
                        .map_err(|e| format!("unsupported monitor log expression: {e}"))?,
                ),
                _ => return Err("invalid log pattern kind".into()),
            };
            if !CAPTURE.with_borrow(|capture| capture.is_some()) {
                return Err("attach a log tester before waiting".into());
            }
            let find = |keep: bool| {
                CAPTURE.with_borrow_mut(|capture| {
                    let capture = capture.as_mut().expect("attached log tester");
                    let index = capture.entries.iter().position(|(severity, text)| {
                        level.is_none_or(|level| level == *severity)
                            && expression
                                .as_ref()
                                .map_or_else(|| text.contains(&pattern), |re| re.is_match(text))
                    });
                    index.map(|index| {
                        let message = capture.entries[index].1.clone();
                        if !keep {
                            capture.entries.drain(..=index);
                        }
                        message
                    })
                })
            };
            let mut found = find(keep);
            let mut stop = Stop::Limit;
            if found.is_none() && steps != 0 {
                (stop, _) =
                    machine.run_until(steps, &mut io::sink(), |_, _| find(true).is_some())?;
                found = find(keep);
            }
            Ok(match found {
                Some(message) => ("MATCH", message),
                None if stop != Stop::Limit => ("STOP", format!("{stop:?}")),
                None => (
                    "FAIL",
                    format!("Expected pattern {pattern:?} did not appear in the log"),
                ),
            })
        })()),
        _ => None,
    }
}

fn decode(value: &str) -> Result<String, String> {
    if value == "-" {
        return Ok(String::new());
    }
    if !value.len().is_multiple_of(2) || !value.is_ascii() {
        return Err("invalid hexadecimal log pattern".into());
    }
    let data = (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    String::from_utf8(data).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_filters_levels_and_keeps_or_consumes_matches() {
        let mut machine = Machine::new(4096).unwrap();
        attach(4).unwrap();
        record(0, "debug");
        record(1, "info");
        let wait = |machine: &mut Machine, command: &str| handle(machine, command).unwrap().unwrap();
        assert_eq!(
            wait(
                &mut machine,
                "WaitForMonitorLog 0 literal keep 0 6465627567"
            )
            .0,
            "FAIL"
        );
        for mode in ["keep", "consume"] {
            let command = format!("WaitForMonitorLog 0 literal {mode} 1 696e666f");
            assert_eq!(wait(&mut machine, &command), ("MATCH", "info".into()));
        }
        assert_eq!(
            wait(
                &mut machine,
                "WaitForMonitorLog 0 literal consume any 696e666f"
            )
            .0,
            "FAIL"
        );
        set_level(0);
        record(0, "debug");
        assert_eq!(
            wait(
                &mut machine,
                "WaitForMonitorLog 0 literal consume 0 6465627567"
            ),
            ("MATCH", "debug".into())
        );
    }
}
