use crate::{constant_mmio::ConstantMmio, machine::Machine};

#[test]
fn find_after_registration() {
    let mut machine = Machine::without_default_ram();
    machine
        .bus
        .map_constant_mmio(ConstantMmio::doubleword(1000, 1000, 0x666).unwrap())
        .unwrap();
    assert_eq!(machine.bus.read_scalar::<4>(1000, true).unwrap(), 0x666);
}

#[test]
fn find_after_many_registrations() {
    let mut machine = Machine::without_default_ram();
    let peripherals = [(1000, 0x666), (2000, 0x667), (3000, 0x668)];
    for (base, value) in peripherals {
        machine
            .bus
            .map_constant_mmio(ConstantMmio::doubleword(base, 100, value).unwrap())
            .unwrap();
    }
    for (base, value) in peripherals {
        assert_eq!(machine.bus.read_scalar::<4>(base, true).unwrap(), value);
    }
}
