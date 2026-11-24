//! Log capture for one Robot monitor session on the emulation thread.
use crate::machine::{Machine, Stop};
use std::{cell::RefCell, collections::VecDeque, io};

struct Capture {
    limit: usize,
    entries: VecDeque<(i8, String)>,
    minimum_level: i8,
    dropped: bool,
    failing: Vec<LogPattern>,
    revision: u64,
}

struct LogPattern {
    text: String,
    regex: Option<regex::Regex>,
}

impl LogPattern {
    fn new(kind: &str, encoded: &str) -> Result<Self, String> {
        let text = decode(encoded)?;
        let regex = match kind {
            "literal" => None,
            "regex" => Some(
                regex::Regex::new(&text)
                    .map_err(|error| format!("unsupported monitor log expression: {error}"))?,
            ),
            _ => return Err("invalid log pattern kind".into()),
        };
        Ok(Self { text, regex })
    }

    fn matches(&self, message: &str) -> bool {
        self.regex
            .as_ref()
            .map_or_else(|| message.contains(&self.text), |re| re.is_match(message))
    }

    fn same_as(&self, other: &Self) -> bool {
        self.text == other.text && self.regex.is_some() == other.regex.is_some()
    }
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
                capture.dropped = true;
                capture.entries.drain(..(capture.limit / 10).max(1));
            }
            capture.entries.push_back((level, message.to_owned()));
            capture.revision = capture.revision.wrapping_add(1);
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
            dropped: false,
            failing: Vec::new(),
            revision: 0,
        })
    });
    Ok(())
}

pub(crate) fn is_command(word: &str) -> bool {
    matches!(
        word,
        "AttachMonitorLog" | "ClearMonitorLog" | "RegisterFailingMonitorLog"
            | "UnregisterFailingMonitorLog" | "WaitForMonitorLog" | "AssertAbsentMonitorLog"
    )
}

pub(crate) fn handle(
    machine: &mut Machine,
    command: &str,
    budget: u64,
) -> Option<Result<(&'static str, String), String>> {
    let words: Vec<_> = command.split_ascii_whitespace().collect();
    match words.as_slice() {
        ["AttachMonitorLog", limit] => Some((|| {
            attach(limit.parse().map_err(|_| "invalid log capacity")?)?;
            Ok(("OK", String::new()))
        })()),
        ["ClearMonitorLog"] => Some(CAPTURE.with_borrow_mut(|capture| {
            let capture = capture.as_mut().ok_or("Log tester is not available")?;
            capture.entries.clear();
            capture.dropped = false;
            Ok(("OK", String::new()))
        })),
        [
            operation @ ("RegisterFailingMonitorLog" | "UnregisterFailingMonitorLog"),
            kind,
            encoded,
        ] => Some((|| {
            let pattern = LogPattern::new(kind, encoded)?;
            CAPTURE.with_borrow_mut(|capture| {
                let capture = capture.as_mut().ok_or("Log tester is not available")?;
                if *operation == "RegisterFailingMonitorLog" {
                    capture.failing.push(pattern);
                } else {
                    let index = capture
                        .failing
                        .iter()
                        .position(|value| value.same_as(&pattern))
                        .ok_or("Unable to unregister failing string, entry not found")?;
                    capture.failing.remove(index);
                }
                Ok(("OK", String::new()))
            })
        })()),
        [
            operation @ ("WaitForMonitorLog" | "AssertAbsentMonitorLog"),
            steps,
            kind,
            keep,
            level,
            pattern,
        ] => Some((|| {
            let absent = *operation == "AssertAbsentMonitorLog";
            let ms = steps
                .parse::<u64>()
                .map_err(|_| "invalid timeout milliseconds")?;
            let steps = crate::monitor::duration_cycles(ms, 1000, machine.bus.monitor_step_hz())?;
            let keep = absent
                || match *keep {
                    "keep" => true,
                    "consume" => false,
                    _ => return Err("invalid log retention option".into()),
                };
            let level = if *level == "any" {
                None
            } else {
                Some(level.parse::<i8>().map_err(|_| "invalid log severity")?)
            };
            let pattern = LogPattern::new(kind, pattern)?;
            if !CAPTURE.with_borrow(|capture| capture.is_some()) {
                return Err("Log tester is not available".into());
            }
            let find = |keep: bool| {
                CAPTURE.with_borrow_mut(|capture| {
                    let capture = capture.as_mut().expect("attached log tester");
                    let failing = capture.entries.iter().position(|(_, text)| {
                        capture.failing.iter().any(|pattern| pattern.matches(text))
                    });
                    let index = failing.or_else(|| {
                        capture.entries.iter().position(|(severity, text)| {
                            level.is_none_or(|level| level == *severity) && pattern.matches(text)
                        })
                    });
                    index.map(|index| {
                        let message = capture.entries[index].1.clone();
                        if !keep {
                            capture.entries.drain(..=index);
                        }
                        (message, failing.is_some())
                    })
                })
            };
            let limit = steps.min(budget);
            let mut found = find(keep);
            let mut stop = Stop::Limit;
            if found.is_none() && steps != 0 {
                let revision = || {
                    CAPTURE.with_borrow(|capture| {
                        capture.as_ref().expect("attached log tester").revision
                    })
                };
                let checked = std::cell::Cell::new(revision());
                (stop, _) = crate::monitor::run_until_deadline(machine, limit, &mut io::sink(), |_, _| {
                    let current = revision();
                    checked.replace(current) != current && find(true).is_some()
                })?;
                found = find(keep);
            }
            let dropped =
                CAPTURE.with_borrow(|capture| capture.as_ref().is_some_and(|c| c.dropped));
            Ok(match found {
                _ if absent && dropped => ("FAIL", "Log tester dropped buffered messages".into()),
                Some((message, true)) => (
                    "FAIL",
                    format!("Test failing entry has been found in log: {message}"),
                ),
                Some((message, false)) if absent => (
                    "FAIL",
                    format!("Unexpected line detected in the log: {message}"),
                ),
                Some((message, false)) => ("MATCH", message),
                None if stop == Stop::Limit && limit < steps => ("STOP", "Limit".into()),
                None if stop == Stop::Limit && absent => {
                    ("MATCH", format!("No log entry matched {:?}", pattern.text))
                }
                None if stop != Stop::Limit => ("STOP", format!("{stop:?}")),
                None => (
                    "FAIL",
                    format!(
                        "Expected pattern {:?} did not appear in the log",
                        pattern.text
                    ),
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
    fn script_budget_cannot_satisfy_a_larger_absence_window() {
        let mut machine = Machine::new(4096).unwrap();
        attach(4).unwrap();
        let result = handle(&mut machine, "AssertAbsentMonitorLog 1 literal keep any 78", 0)
            .unwrap().unwrap();
        assert_eq!(result, ("STOP", "Limit".into()));
        assert_eq!(machine.elapsed_steps(), 0);
    }

    #[test]
    fn paused_monitor_does_not_satisfy_a_positive_absence_window() {
        let mut machine = Machine::new(4096).unwrap();
        machine.primary.monitor_paused = true;
        attach(4).unwrap();
        let (status, reason) = handle(
            &mut machine, "AssertAbsentMonitorLog 1 literal keep any 78", u64::MAX,
        ).unwrap().unwrap();
        assert_eq!((status, reason.as_str()), ("STOP", "Paused"));
    }

    #[test]
    fn log_absence_rejects_matches_and_dropped_entries() {
        let mut machine = Machine::new(4096).unwrap();
        let absent = "AssertAbsentMonitorLog 0 literal keep any 6e6565646c65";
        let run = |machine: &mut Machine, command| handle(machine, command, u64::MAX).unwrap().unwrap().0;
        attach(1).unwrap();
        assert_eq!(run(&mut machine, absent), "MATCH");
        record(2, "needle");
        assert_eq!(run(&mut machine, absent), "FAIL");
        assert_eq!(run(&mut machine, absent), "FAIL");
        record(2, "different");
        assert_eq!(run(&mut machine, absent), "FAIL");
        assert_eq!(run(&mut machine, "ClearMonitorLog"), "OK");
        assert_eq!(run(&mut machine, absent), "MATCH");
    }

    #[test]
    fn failing_patterns_precede_expected_entries_and_can_be_removed() {
        let mut machine = Machine::new(4096).unwrap();
        attach(4).unwrap();
        record(2, "expected");
        record(2, "failure 42");
        let run = |machine: &mut Machine, command| handle(machine, command, u64::MAX).unwrap().unwrap();
        run(
            &mut machine,
            "RegisterFailingMonitorLog regex 6661696c757265205b302d395d",
        );
        let wait = "WaitForMonitorLog 0 literal keep any 6578706563746564";
        let (status, message) = run(&mut machine, wait);
        assert_eq!(status, "FAIL");
        assert!(message.contains("failure 42"));
        run(
            &mut machine,
            "UnregisterFailingMonitorLog regex 6661696c757265205b302d395d",
        );
        assert_eq!(run(&mut machine, wait), ("MATCH", "expected".into()));
    }

    #[test]
    fn capture_filters_levels_and_keeps_or_consumes_matches() {
        let mut machine = Machine::new(4096).unwrap();
        attach(4).unwrap();
        record(0, "debug");
        record(1, "info");
        let wait =
            |machine: &mut Machine, command: &str| handle(machine, command, u64::MAX).unwrap().unwrap();
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
