use crate::{
    bus_translation::Port,
    dma_engine::{DmaBus, Width},
    efm32_i2c::Efm32I2c,
    machine::Machine,
};

const BASE: u64 = 0x4000_a000;

fn setup() -> Machine {
    let mut machine = Machine::without_default_ram();
    let mut controller = Efm32I2c::default();
    controller.reset();
    machine
        .bus
        .map_translated_peripheral(
            vec![Port {
                base: BASE as u32,
                length: 0x400,
                region: None,
            }],
            Width::DoubleWord,
            Box::new(controller),
        )
        .unwrap();
    machine
}

fn read(machine: &mut Machine, offset: u64) -> u64 {
    machine
        .bus
        .read_unit(BASE + offset, Width::DoubleWord)
        .unwrap()
}

fn write(machine: &mut Machine, offset: u64, value: u64) {
    machine
        .bus
        .write_unit(
            BASE + offset,
            Width::DoubleWord,
            value,
            &mut std::io::sink(),
        )
        .unwrap();
}

#[test]
fn init_test() {
    let mut machine = setup();
    assert_eq!(read(&mut machine, 0x00), 0);
    write(&mut machine, 0x00, 1);
    assert_eq!(read(&mut machine, 0x00), 1);
    assert_eq!(read(&mut machine, 0x28), 0);
    assert_eq!(read(&mut machine, 0x34), 0);
    write(&mut machine, 0x34, 0x1ffff);
    for offset in [0x08, 0x0c, 0x10, 0x14, 0x18, 0x1c, 0x20] {
        assert_eq!(read(&mut machine, offset), 0);
    }
    assert_eq!(read(&mut machine, 0x28), 0x2000);
    assert_eq!(read(&mut machine, 0x34), 0x1ffff);
    assert_eq!(read(&mut machine, 0x38), 0);
}

#[test]
fn ctrl_test() {
    let mut machine = setup();
    write(&mut machine, 0, 1);
    assert_eq!(read(&mut machine, 0), 1);
}

#[test]
fn interrupt_test() {
    let mut machine = setup();
    write(&mut machine, 0, 1);
    write(&mut machine, 0x34, 0x1ffff);
    write(&mut machine, 0x30, 0x1ffff);
    assert_eq!(read(&mut machine, 0x28), 0);
    let mask = 0x1 | 0x40 | 0x80;
    write(&mut machine, 0x2c, mask);
    assert_eq!(read(&mut machine, 0x28), mask);
    write(&mut machine, 0x30, 0x1ffff);
    assert_eq!(read(&mut machine, 0x28), 0);
    write(&mut machine, 4, 1);
    assert_eq!(read(&mut machine, 0x28) & 1, 1);
}
