use crate::{
    bus_translation::{NativePeripheral, PeripheralHandle, Port},
    dma_engine::Width,
    machine::Machine,
};

struct MockDoubleWordPeripheral;

impl NativePeripheral for MockDoubleWordPeripheral {
    fn read(&mut self, _: Option<&str>, _: u64) -> u64 {
        0
    }

    fn write(&mut self, _: Option<&str>, _: u64, _: u64) {}
}

fn register(machine: &mut Machine, base: u32) -> PeripheralHandle {
    let handle = PeripheralHandle::default();
    machine
        .bus
        .register_translated_peripheral(
            &handle,
            vec![Port {
                base,
                length: 10,
                region: None,
            }],
            Width::DoubleWord,
            Box::new(MockDoubleWordPeripheral),
        )
        .unwrap();
    handle
}

#[test]
fn should_throw_on_registering_another_peripheral_with_the_same_name() {
    let mut machine = Machine::without_default_ram();
    let first = register(&mut machine, 0);
    let second = register(&mut machine, 10);
    machine
        .set_peripheral_local_name(&first, Some("name"))
        .unwrap();
    assert!(machine
        .set_peripheral_local_name(&second, Some("name"))
        .is_err());
}

#[test]
fn should_find_peripheral_by_path() {
    let mut machine = Machine::without_default_ram();
    let peripheral = register(&mut machine, 0);
    machine
        .set_peripheral_local_name(&peripheral, Some("name"))
        .unwrap();
    assert!(machine.peripheral_by_path("sysbus.name") == Some(&peripheral));
}

#[test]
fn should_find_peripheral_by_path_when_there_are_two() {
    let mut machine = Machine::without_default_ram();
    let first = register(&mut machine, 0);
    let second = register(&mut machine, 10);
    machine
        .set_peripheral_local_name(&first, Some("first"))
        .unwrap();
    machine
        .set_peripheral_local_name(&second, Some("second"))
        .unwrap();
    assert!(machine.peripheral_by_path("sysbus.first") == Some(&first));
    assert!(machine.peripheral_by_path("sysbus.second") == Some(&second));
}

#[test]
fn should_throw_on_null_or_empty_peripheral_name() {
    let mut machine = Machine::without_default_ram();
    let peripheral = register(&mut machine, 0);
    assert!(machine
        .set_peripheral_local_name(&peripheral, Some(""))
        .is_err());
    assert!(machine
        .set_peripheral_local_name(&peripheral, None)
        .is_err());
}
