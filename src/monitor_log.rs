//! Monitor log history and tester capture on the emulation thread.
use crate::machine::{Machine, Stop};
use std::{cell::RefCell, collections::{HashMap, VecDeque}, io};

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

pub(crate) const HISTORY_LIMIT: usize = 1000;
pub(crate) const DEFAULT_TAIL: i32 = 20;

#[derive(Default)]
struct History {
    entries: VecDeque<String>,
    minimum_level: Option<i8>,
}

pub(crate) fn enable_history() {
    HISTORY.with_borrow_mut(|history| *history = Some(History::default()));
}

pub(crate) fn tail(count: i32) -> Vec<String> {
    HISTORY.with_borrow(|history| {
        history.as_ref().map_or_else(Vec::new, |history| {
            let skip = (history.entries.len() as i32).wrapping_sub(count).max(0) as usize;
            history.entries.iter().skip(skip)
                .cloned().collect()
        })
    })
}

pub(crate) fn parse_level(value: &str) -> Result<i8, String> {
    match value {
        "-1" | "Noisy" => Ok(-1),
        "0" | "Debug" => Ok(0),
        "1" | "Info" => Ok(1),
        "2" | "Warning" => Ok(2),
        "3" | "Error" => Ok(3),
        _ => Err(format!("unsupported monitor log level {value:?}")),
    }
}

fn history_entry(level: i8, message: &str) -> String {
    let level = match level {
        -1 => "NOISY",
        0 => "DEBUG",
        1 => "INFO",
        2 => "WARNING",
        3 => "ERROR",
        _ => "LOG",
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let seconds = now.as_secs() % 86_400;
    format!("{:02}:{:02}:{:02}.{:04} [{level}] {message}",
        seconds / 3600, seconds / 60 % 60, seconds % 60, now.subsec_micros() / 100)
}

thread_local! {
    static HISTORY: RefCell<Option<History>> = const { RefCell::new(None) };
    static CAPTURE: RefCell<Option<Capture>> = const { RefCell::new(None) };
}

thread_local! {
    static TARGET_LEVELS: RefCell<HashMap<String, i8>> = RefCell::new(HashMap::new());
}

pub(crate) fn set_target_level(target: &str, level: i8) {
    TARGET_LEVELS.with_borrow_mut(|levels| { levels.insert(target.to_owned(), level); });
}

pub(crate) fn record(level: i8, message: &str) {
    let target_level = message.split_once(':').and_then(|(target, _)| {
        TARGET_LEVELS.with_borrow(|levels| levels.get(target).copied())
    });
    HISTORY.with_borrow_mut(|history| {
        if let Some(history) = history
            && level >= target_level.or(history.minimum_level).unwrap_or(1)
        {
            if history.entries.len() == HISTORY_LIMIT {
                history.entries.pop_front();
            }
            history.entries.push_back(history_entry(level, message));
        }
    });
    CAPTURE.with_borrow_mut(|capture| {
        if let Some(capture) = capture
            && level >= target_level.unwrap_or(capture.minimum_level)
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
    HISTORY.with_borrow_mut(|history| {
        if let Some(history) = history {
            history.minimum_level = Some(level);
        }
    });
    CAPTURE.with_borrow_mut(|capture| {
        if let Some(capture) = capture {
            capture.minimum_level = level;
        }
    });
}

pub(crate) fn parse_capacity(value: &str) -> Result<usize, String> {
    let limit = value.parse().map_err(|_| "invalid log capacity")?;
    if !(1..=1_000_000).contains(&limit) {
        return Err("log capacity must be between 1 and 1000000".into());
    }
    Ok(limit)
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
    output: &mut impl io::Write,
) -> Option<Result<(&'static str, String), String>> {
    let words: Vec<_> = command.split_ascii_whitespace().collect();
    match words.as_slice() {
        ["AttachMonitorLog", limit] => Some((|| {
            attach(parse_capacity(limit)?)?;
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
            options @ ..,
        ] => Some((|| {
            let (mode, pause) = match options {
                [] => ("legacy", false),
                [mode @ ("start" | "no-start" | "buffered"), pause @ ("pause" | "continue")] =>
                    (*mode, *pause == "pause"),
                _ => return Err("expected log wait options: start|no-start|buffered pause|continue".into()),
            };
            let absent = *operation == "AssertAbsentMonitorLog";
            let ms = if let Some(seconds) = steps.strip_suffix('s') {
                let seconds: f32 = seconds.parse().map_err(|_| "invalid log timeout seconds")?;
                let milliseconds = seconds * 1000.0;
                if !seconds.is_finite() || seconds < 0.0 || f64::from(milliseconds) >= u64::MAX as f64 {
                    return Err("invalid log timeout seconds".into());
                }
                // Renode receives a float in seconds and schedules an integer millisecond deadline.
                milliseconds as u64
            } else {
                steps.parse::<u64>().map_err(|_| "invalid timeout milliseconds")?
            };
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
                return Ok(("FAIL", "Log tester is not available".into()));
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
            if found.is_none() && limit != 0 && (mode == "buffered" || (mode == "no-start" && !machine.monitor_is_active())) {
                stop = Stop::Paused;
            } else if found.is_none() && limit != 0 {
                if mode == "start" {
                    machine.set_monitor_active(true);
                }
                let revision = || {
                    CAPTURE.with_borrow(|capture| {
                        capture.as_ref().expect("attached log tester").revision
                    })
                };
                let checked = std::cell::Cell::new(revision());
                (stop, _) = crate::monitor::run_until_deadline(machine, limit, output, |_, _| {
                    let current = revision();
                    checked.replace(current) != current && find(true).is_some()
                })?;
                found = find(keep);
                if pause && (found.is_some() || (stop == Stop::Limit && limit == steps)) {
                    machine.set_monitor_active(false);
                }
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
    fn monitor_history_evicts_one_entry_without_consuming_the_tail() {
        enable_history();
        for index in 0..=HISTORY_LIMIT {
            record(1, &format!("Script: unique {index:04}"));
        }
        record(0, "filtered");
        let entries = tail(HISTORY_LIMIT as i32);
        assert_eq!(entries.len(), HISTORY_LIMIT);
        assert!(entries[0].ends_with("Script: unique 0001"));
        assert!(entries.last().unwrap().ends_with("Script: unique 1000"));
        assert_eq!(tail(1), entries[HISTORY_LIMIT - 1..]);
        assert!(tail(-1).is_empty());
        assert_eq!(tail(i32::MIN), entries);
    }

    #[test]
    fn stopped_log_wait_matches_buffered_entries_without_advancing_time() {
        let mut machine = Machine::new(4096).unwrap();
        attach(4).unwrap();
        let command = "WaitForMonitorLog 1000 literal consume any 78 buffered pause";
        record(1, "x");
        let mut output = io::sink();
        let observed = handle(&mut machine, command, u64::MAX, &mut output).unwrap().unwrap();
        assert_eq!(observed, ("MATCH", "x".into()));
        let missing = handle(&mut machine, command, u64::MAX, &mut output).unwrap().unwrap();
        assert_eq!(missing, ("STOP", "Paused".into()));
        assert_eq!(machine.elapsed_steps(), 0);
    }

    #[test]
    fn explicit_log_wait_can_resume_and_pause_at_the_deadline() {
        let mut machine = Machine::new(4096).unwrap();
        machine.bus.write(0x8000_0000, 4, 0x6f, &mut io::sink()).unwrap();
        machine.primary.monitor_paused = true;
        attach(4).unwrap();
        let command = "AssertAbsentMonitorLog 0.0019s literal keep any 78";
        for (options, expected) in [("no-start pause", "STOP"), ("start pause", "MATCH")] {
            let result = handle(&mut machine, &format!("{command} {options}"), u64::MAX, &mut io::sink())
                .unwrap().unwrap();
            assert_eq!(result.0, expected);
            assert!(machine.primary.monitor_paused);
        }
        assert_eq!(machine.elapsed_steps(), 1000);
    }

    #[test]
    fn script_budget_cannot_satisfy_a_larger_absence_window() {
        let mut machine = Machine::new(4096).unwrap();
        attach(4).unwrap();
        let result = handle(&mut machine, "AssertAbsentMonitorLog 1 literal keep any 78", 0, &mut io::sink())
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
            &mut machine, "AssertAbsentMonitorLog 1 literal keep any 78", u64::MAX, &mut io::sink(),
        ).unwrap().unwrap();
        assert_eq!((status, reason.as_str()), ("STOP", "Paused"));
    }

    #[test]
    fn log_absence_rejects_matches_and_dropped_entries() {
        let mut machine = Machine::new(4096).unwrap();
        let absent = "AssertAbsentMonitorLog 0 literal keep any 6e6565646c65";
        let run = |machine: &mut Machine, command| handle(machine, command, u64::MAX, &mut io::sink()).unwrap().unwrap().0;
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
        let run = |machine: &mut Machine, command| handle(machine, command, u64::MAX, &mut io::sink()).unwrap().unwrap();
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
            |machine: &mut Machine, command: &str| handle(machine, command, u64::MAX, &mut io::sink()).unwrap().unwrap();
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
