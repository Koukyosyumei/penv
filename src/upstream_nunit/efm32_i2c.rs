use crate::{
    bus_translation::Port,
    dma_engine::{DmaBus, Width},
    efm32_i2c::Efm32I2c,
    machine::Machine,
};

const BASE: u64 = 0x4000_a000;

fn setup() -> Machine {
    let mut controller = Efm32I2c::default();
    controller.reset();
    map_controller(controller)
}

fn map_controller(controller: Efm32I2c) -> Machine {
    let mut machine = Machine::without_default_ram();
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

#[test]
fn dual_i2c_address_bmc050_test() {
    use std::{cell::RefCell, rc::Rc, thread, time::Duration};

    let mut sensor = crate::bmc050::Bmc050::default();
    sensor.reset();
    let sensor = Rc::new(RefCell::new(sensor));
    let mut controller = Efm32I2c::default();
    controller.reset();
    controller.register_slave(0x18, sensor.clone());
    controller.register_slave(0x10, sensor);
    let mut machine = map_controller(controller);

    write(&mut machine, 0x00, 1);
    write(&mut machine, 0x34, 0x1ffff);
    for (write_address, register, read_address, chip_id) in
        [(0x18, 0x00, 0x19, 0x03), (0x10, 0x40, 0x11, 0x32)]
    {
        write(&mut machine, 0x04, 1);
        assert_eq!(read(&mut machine, 0x28) & 1, 1);
        write(&mut machine, 0x24, write_address);
        assert_ne!(read(&mut machine, 0x28) & 0x1000, 0x1000);
        assert_eq!(read(&mut machine, 0x28) & 0x40, 0x40);
        write(&mut machine, 0x24, register);
        assert_ne!(read(&mut machine, 0x28) & 0x1000, 0x1000);
        write(&mut machine, 0x04, 1);
        write(&mut machine, 0x24, read_address);
        assert_ne!(read(&mut machine, 0x28) & 0xc, 0xc);

        let mut iterations = 0;
        let available = loop {
            let available = read(&mut machine, 0x28) & 0x20;
            iterations += 1;
            thread::sleep(Duration::from_millis(10));
            if available == 0x20 || iterations == 1_000 {
                break available;
            }
        };
        assert_eq!(available, 0x20);
        assert_ne!(iterations, 1_000);
        assert_eq!(read(&mut machine, 0x1c), chip_id);
        write(&mut machine, 0x04, 2);
        assert_eq!(read(&mut machine, 0x28) & 0x100, 0x100);
    }
}
