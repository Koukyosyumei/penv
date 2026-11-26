use crate::{
    machine::Machine,
    pydev::Peripheral,
    rtc_clock::{RtcClock, RtcMode},
};

const SIMPLE: &str = "
if request.IsInit:
\tnum = 4
\tstr = 'napis'
if request.IsRead:
\trequest.Value = num + len(str)
";

#[test]
fn serialize_simple_pydev() {
    let _machine = Machine::without_default_ram();
    let mut device = Peripheral::new(SIMPLE, 0, 100, true).unwrap();
    let mut copy = Peripheral::from_snapshot(device.snapshot().unwrap()).unwrap();
    assert_eq!(copy.access(0, 1, None).unwrap(), 9);
}

#[test]
fn serialize_pydev_with_module_import() {
    let _machine = Machine::without_default_ram();
    let source = "
import time

if request.IsRead:
\trequest.Value = time.localtime().tm_hour
";
    let mut device = Peripheral::new(source, 0, 100, true).unwrap();
    let mut copy = Peripheral::from_snapshot(device.snapshot().unwrap()).unwrap();

    let now = copy.access(0, 4, None).unwrap();
    let mut clock = RtcClock::default();
    clock.set_mode(RtcMode::HostTimeLocal).unwrap();
    let date_time = clock.date_time(0);
    let hour = date_time[11..13].parse::<u64>().unwrap();
    assert!(
        hour.abs_diff(now) <= 1,
        "local hour {hour}, device hour {now}"
    );
}

#[test]
fn snapshot_preserves_initialized_python_scope() {
    let source = "
if request.IsInit:
    some_list = ['a', 666]
    some_dict = {'lion': 'yellow', 'kitty': 'red'}
    import time
if request.IsWrite:
    some_list[1] = request.Value
if request.IsRead:
    if request.Offset == 0:
        request.Value = some_list[1]
    if request.Offset == 4:
        request.Value = len(some_dict['kitty'])
    if request.Offset == 8:
        request.Value = int(time.__name__ == 'time')
";
    let mut device = Peripheral::new(source, 0, 100, true).unwrap();
    device.access(0, 4, Some(123)).unwrap();
    let snapshot = device.snapshot().unwrap();
    device.access(0, 4, Some(456)).unwrap();
    let mut copy = Peripheral::from_snapshot(snapshot).unwrap();
    assert_eq!(copy.access(0, 4, None).unwrap(), 123);
    assert_eq!(copy.access(4, 4, None).unwrap(), 3);
    assert_eq!(copy.access(8, 4, None).unwrap(), 1);
    assert_eq!(device.access(0, 4, None).unwrap(), 456);
}

#[test]
fn serialize_pydev_with_list_and_dictionary() {
    use std::io::{Cursor, Seek, SeekFrom};

    use crate::pydev::PeripheralSnapshot;

    let _machine = Machine::without_default_ram();
    let source = "
if request.IsInit:
\tsome_list = [ 'a', 666 ]
\tsome_dict = { 'lion': 'yellow', 'kitty': 'red' }
if request.IsRead:
\tif request.Offset == 0:
\t\trequest.Value = some_list[1]
\tif request.Offset == 4:
\t\trequest.Value = len(some_dict['kitty'])
";
    let mut device = Peripheral::new(source, 0, 100, true).unwrap();
    let mut stream = Cursor::new(Vec::new());
    // The CPython worker marshals dictionary entries and restores a dictionary directly.
    device.snapshot().unwrap().write_to(&mut stream).unwrap();
    stream.seek(SeekFrom::Start(0)).unwrap();
    let mut copy =
        Peripheral::from_snapshot(PeripheralSnapshot::read_from(stream).unwrap()).unwrap();
    assert_eq!(copy.access(0, 4, None).unwrap(), 666);
    assert_eq!(copy.access(4, 4, None).unwrap(), 3);
}

#[test]
fn serialize_machine_with_simple_pydev() {
    let source = "
if request.IsInit:
    num = 4
    str = 'napis'
if request.IsRead:
    request.Value = num + len(str)
";
    let mut machine = Machine::without_default_ram();
    machine
        .bus
        .map_pydev_range(source, 0x100, 100, 0x10, true)
        .unwrap();
    assert_eq!(machine.bus.read::<1>(0x100).unwrap(), 9);
    let snapshot = machine.bus.snapshot_pydevs().unwrap();
    let mut copy = Machine::without_default_ram();
    copy.bus.restore_pydevs(snapshot).unwrap();
    assert_eq!(copy.bus.read::<1>(0x100).unwrap(), 9);
}
