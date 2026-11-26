use crate::{
    bus_translation::{NativePeripheral, PeripheralHandle, Port, RegistrationError},
    dma_engine::Width,
    machine::Machine,
};
use std::cell::RefCell;
use std::rc::Rc;

const GROUP: &str = "test-group";

#[derive(Clone, Default)]
struct Memory(Rc<RefCell<[u8; 10]>>);

impl NativePeripheral for Memory {
    fn read(&mut self, _: Option<&str>, offset: u64) -> u64 {
        u64::from(self.0.borrow()[offset as usize])
    }

    fn write(&mut self, _: Option<&str>, offset: u64, value: u64) {
        self.0.borrow_mut()[offset as usize] = value as u8;
    }
}

fn register(machine: &mut Machine, handle: &PeripheralHandle, memory: Memory) {
    machine
        .bus
        .register_translated_peripheral(
            handle,
            vec![Port {
                base: 0,
                length: 10,
                region: None,
            }],
            Width::Byte,
            Box::new(memory),
        )
        .unwrap();
}

#[test]
fn should_not_unregister_single_peripheral_from_group() {
    let mut machine = Machine::without_default_ram();
    let handle = PeripheralHandle::default();
    let memory = Memory::default();
    register(&mut machine, &handle, memory.clone());
    machine
        .bus
        .unregister_translated_peripheral(&handle)
        .unwrap();

    register(&mut machine, &handle, memory);
    machine
        .bus
        .get_or_create_translated_group(GROUP, std::slice::from_ref(&handle))
        .unwrap();
    assert_eq!(
        machine.bus.unregister_translated_peripheral(&handle),
        Err(RegistrationError::AlreadyGrouped)
    );
}

#[test]
fn should_unregister_peripheral_groups() {
    let mut machine = Machine::without_default_ram();
    let handle = PeripheralHandle::default();
    register(&mut machine, &handle, Memory::default());
    machine
        .bus
        .get_or_create_translated_group(GROUP, std::slice::from_ref(&handle))
        .unwrap();
    assert!(machine.bus.is_translated_peripheral_registered(&handle));
    machine.bus.unregister_translated_group(GROUP).unwrap();
    assert!(!machine.bus.is_translated_peripheral_registered(&handle));
}

#[test]
fn should_not_add_unregistered_peripheral_to_group() {
    let mut machine = Machine::without_default_ram();
    let _memory = Memory::default();
    let handle = PeripheralHandle::default();
    assert_eq!(
        machine.bus.get_or_create_translated_group(GROUP, &[handle]),
        Err(RegistrationError::NotRegistered)
    );
}
