use crate::{machine::Machine, monitor::MonitorBuiltins};

#[derive(Default)]
struct MonitorFixture {
    monitor: MonitorBuiltins,
    machine: Option<Machine>,
}

impl MonitorFixture {
    fn parse(&mut self, command: &str) -> String {
        let machine = self
            .machine
            .get_or_insert_with(Machine::without_default_ram);
        self.monitor.execute(machine, command).unwrap().unwrap()
    }

    fn build_emulation(&mut self) -> &mut Machine {
        let machine = self.machine.insert(Machine::without_default_ram());
        machine.bus.map_named_ram("memory", 0x1000, 0x1000).unwrap();
        machine
    }
}

#[test]
fn next_value_test() {
    let mut monitor = MonitorFixture::default();
    let value: i64 = monitor
        .parse("next_value")
        .trim_end_matches(['\r', '\n'])
        .parse()
        .unwrap();
    let next_value: i64 = monitor
        .parse("next_value")
        .trim_end_matches(['\r', '\n'])
        .parse()
        .unwrap();
    assert_eq!(next_value - value, 1);
}

#[test]
fn sleep_test() {
    let mut monitor = MonitorFixture::default();
    monitor.parse("sleep 1");
    let stopwatch = std::time::Instant::now();
    monitor.parse("sleep 5");
    let seconds = stopwatch.elapsed().as_secs() % 60;
    assert!(seconds >= 4);
    assert!(seconds <= 6);
}

#[test]
fn echo_test() {
    let mut monitor = MonitorFixture::default();
    let text = "test text with stuff";
    let actual = monitor.parse(&format!("echo \"{text}\""));
    assert_eq!(actual.trim_end_matches(['\r', '\n']), text);
}

#[test]
fn dump_test() {
    const ADDRESS: u32 = 0x1000;
    const MESSAGE: &[u8] = b"MAGIC MESSAGE";
    let mut monitor = MonitorFixture::default();
    let machine = monitor.build_emulation();
    machine
        .bus
        .write_bytes(ADDRESS, &[0], &mut std::io::sink())
        .unwrap();
    machine
        .bus
        .write_bytes(ADDRESS, MESSAGE, &mut std::io::sink())
        .unwrap();
    let result = monitor.parse(&format!("dump {ADDRESS} {}", MESSAGE.len()));

    let mut index = 0;
    for (section, element) in result
        .split('|')
        .filter(|part| !part.is_empty())
        .enumerate()
    {
        if section % 3 == 1 {
            for byte in element.split(' ').filter(|byte| !byte.is_empty()) {
                let actual = u8::from_str_radix(byte, 16).unwrap();
                assert_eq!(actual, MESSAGE.get(index).copied().unwrap_or(0));
                index += 1;
            }
        }
    }
    assert!(index >= MESSAGE.len());
}

#[test]
fn dump_file_test() {
    const ADDRESS: u32 = 0x1000;
    const MESSAGE: &str = "MAGIC MESSAGE";
    let mut monitor = MonitorFixture::default();
    let machine = monitor.build_emulation();
    machine
        .bus
        .write_bytes(ADDRESS, MESSAGE.as_bytes(), &mut std::io::sink())
        .unwrap();
    let file = std::env::temp_dir().join(format!("penv-monitor-dump-{}", std::process::id()));
    let path = file.display().to_string().replace(' ', "\\ ");
    monitor.parse(&format!("dump_file {ADDRESS} {} @{path}", MESSAGE.len()));
    let value = std::fs::read_to_string(&file);
    std::fs::remove_file(file).unwrap();
    assert_eq!(value.unwrap(), MESSAGE);
}
