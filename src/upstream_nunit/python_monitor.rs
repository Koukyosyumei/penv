use crate::monitor::MonitorBuiltins;

fn parse(monitor: &mut MonitorBuiltins, command: &str) -> String {
    monitor.execute(command).unwrap().unwrap()
}

#[test]
fn next_value_test() {
    let mut monitor = MonitorBuiltins::default();
    let value: i64 = parse(&mut monitor, "next_value")
        .trim_end_matches(['\r', '\n'])
        .parse()
        .unwrap();
    let next_value: i64 = parse(&mut monitor, "next_value")
        .trim_end_matches(['\r', '\n'])
        .parse()
        .unwrap();
    assert_eq!(next_value - value, 1);
}

#[test]
fn sleep_test() {
    let mut monitor = MonitorBuiltins::default();
    parse(&mut monitor, "sleep 1");
    let stopwatch = std::time::Instant::now();
    parse(&mut monitor, "sleep 5");
    let seconds = stopwatch.elapsed().as_secs() % 60;
    assert!(seconds >= 4);
    assert!(seconds <= 6);
}

#[test]
fn echo_test() {
    let mut monitor = MonitorBuiltins::default();
    let text = "test text with stuff";
    let actual = parse(&mut monitor, &format!("echo \"{text}\""));
    assert_eq!(actual.trim_end_matches(['\r', '\n']), text);
}
