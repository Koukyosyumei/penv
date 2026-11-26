use crate::{
    bus::NULL_REGISTRATION_POINT,
    bus_translation::{NativePeripheral, PeripheralHandle, Port, RegistrationError},
    dma_engine::Width,
    machine::Machine,
};

struct Container;

impl NativePeripheral for Container {
    fn read(&mut self, _: Option<&str>, _: u64) -> u64 {
        1337
    }
    fn write(&mut self, _: Option<&str>, _: u64, _: u64) {}
}

struct Fixture {
    machine: Machine,
    owner: PeripheralHandle,
    first: PeripheralHandle,
    second: PeripheralHandle,
}

impl Fixture {
    fn new() -> Self {
        let mut machine = Machine::without_default_ram();
        let owner = PeripheralHandle::default();
        let first = PeripheralHandle::default();
        let second = PeripheralHandle::default();
        machine
            .bus
            .register_translated_peripheral(
                &owner,
                vec![Port {
                    base: 1337,
                    length: 666,
                    region: None,
                }],
                Width::DoubleWord,
                Box::new(Container),
            )
            .unwrap();
        Self {
            machine,
            owner,
            first,
            second,
        }
    }

    fn attach(&mut self, second: bool) -> Result<(), RegistrationError> {
        let handle = if second { &self.second } else { &self.first };
        self.machine
            .bus
            .register_null_child(&self.owner, handle, &NULL_REGISTRATION_POINT)
    }

    fn detach_first(&mut self) -> Result<(), RegistrationError> {
        self.machine
            .bus
            .unregister_null_child(&self.owner, &self.first)
    }

    fn assert_point(&self) {
        let points: Vec<_> = self
            .machine
            .bus
            .null_registration_points(&self.owner, &self.first)
            .collect();
        assert_eq!(points.len(), 1);
        assert!(std::ptr::eq(points[0], &NULL_REGISTRATION_POINT));
    }
}

#[test]
fn should_register_peripheral() {
    let mut f = Fixture::new();
    f.attach(false).unwrap();
    assert!(f.machine.bus.is_peripheral_registered(&f.first));
}

#[test]
fn should_throw_when_second_peripheral() {
    let mut f = Fixture::new();
    f.attach(true).unwrap();
    assert_eq!(f.attach(false), Err(RegistrationError::ContainerOccupied));
}

#[test]
fn should_unregister_peripheral() {
    let mut f = Fixture::new();
    f.attach(false).unwrap();
    f.detach_first().unwrap();
    assert!(!f.machine.bus.is_peripheral_registered(&f.first));
}

#[test]
fn should_throw_when_unregistering_not_registered_peripheral() {
    let mut f = Fixture::new();
    f.attach(true).unwrap();
    assert_eq!(f.detach_first(), Err(RegistrationError::NotRegistered));
}

#[test]
fn should_throw_when_unregistering_from_empty_containers() {
    let mut f = Fixture::new();
    assert_eq!(f.detach_first(), Err(RegistrationError::NotRegistered));
}

#[test]
fn should_get_registration_points() {
    let mut f = Fixture::new();
    f.attach(false).unwrap();
    f.assert_point();
}

#[test]
fn should_get_empty_registration_points() {
    let mut f = Fixture::new();
    assert_eq!(
        f.machine
            .bus
            .null_registration_points(&f.owner, &f.first)
            .count(),
        0
    );
    f.attach(false).unwrap();
    f.detach_first().unwrap();
    assert_eq!(
        f.machine
            .bus
            .null_registration_points(&f.owner, &f.first)
            .count(),
        0
    );
}

#[test]
fn should_get_registered_peripheral_as_children() {
    let mut f = Fixture::new();
    f.attach(false).unwrap();
    f.assert_point();
    let child = f.machine.bus.null_child(&f.owner).unwrap();
    assert!(child.peripheral == &f.first);
    assert!(std::ptr::eq(
        child.registration_point,
        &NULL_REGISTRATION_POINT
    ));
}

#[test]
fn should_get_empty_children() {
    let mut f = Fixture::new();
    assert!(f.machine.bus.null_child(&f.owner).is_none());
    f.attach(false).unwrap();
    f.detach_first().unwrap();
    assert!(f.machine.bus.null_child(&f.owner).is_none());
}

#[test]
fn should_register_2nd_after_unregistering() {
    let mut f = Fixture::new();
    f.attach(false).unwrap();
    f.detach_first().unwrap();
    f.attach(true).unwrap();
    assert!(!f.machine.bus.is_peripheral_registered(&f.first));
    assert!(f.machine.bus.is_peripheral_registered(&f.second));
}
