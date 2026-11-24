use crate::{
    Machine,
    monitor::{AutoLoadCommand, MonitorBuiltins},
};

#[test]
fn auto_load_command() {
    let mut monitor = MonitorBuiltins::default();
    let mut machine = Machine::without_default_ram();
    let command = AutoLoadCommand {
        name: "featuresTests.TestCommand",
        description: "is just a test command.",
        method_signatures: &[],
        execute: Box::new(|_, _| Ok(String::new())),
    };
    let description = command.description;
    command.register(&mut monitor).unwrap();
    let contents = monitor.execute(&mut machine, "help").unwrap().unwrap();
    assert!(contents.contains(description));
}
