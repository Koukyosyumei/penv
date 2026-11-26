use std::{cell::RefCell, rc::Rc};

use crate::{
    bus_translation::{NativePeripheral, Port},
    dma_engine::Width,
    lpc43xx_gpio::Lpc43xxGpio,
    machine::Machine,
};

const DIR: u64 = 0x2000;
const MASK: u64 = 0x2080;
const PIN: u64 = 0x2100;
const MPIN: u64 = 0x2180;
const SET: u64 = 0x2200;
const CLR: u64 = 0x2280;
const NOT: u64 = 0x2300;
const ALL: u32 = u32::MAX;

struct RegisteredGpio(Rc<RefCell<Lpc43xxGpio>>);

impl NativePeripheral for RegisteredGpio {
    fn read(&mut self, region: Option<&str>, offset: u64) -> u64 {
        self.0.borrow_mut().read(region, offset)
    }

    fn write(&mut self, region: Option<&str>, offset: u64, value: u64) {
        self.0.borrow_mut().write(region, offset, value);
    }
}

fn setup() -> (Machine, Rc<RefCell<Lpc43xxGpio>>) {
    let mut machine = Machine::without_default_ram();
    let gpio = Rc::new(RefCell::new(Lpc43xxGpio::default()));
    machine
        .bus
        .map_translated_peripheral(
            vec![Port {
                base: 0x4000_a000,
                length: 0x400,
                region: None,
            }],
            Width::DoubleWord,
            Box::new(RegisteredGpio(gpio.clone())),
        )
        .unwrap();
    gpio.borrow_mut().reset();
    (machine, gpio)
}

#[test]
fn init_test() {
    let (_machine, gpio) = setup();
    for offset in (0..8).map(|port| 4 * port) {
        for register in [DIR, MASK, SET] {
            assert_eq!(gpio.borrow().read_register(register + offset), 0);
        }
    }
}

#[test]
fn change_direction_to_output() {
    let (_machine, gpio) = setup();
    let mut gpio = gpio.borrow_mut();
    for offset in (0..8).map(|port| 4 * port) {
        gpio.write_register(DIR + offset, ALL);
        assert_eq!(gpio.read_register(DIR + offset), ALL);
        for register in [MASK, PIN, MPIN, SET] {
            assert_eq!(gpio.read_register(register + offset), 0);
        }
    }
}

#[test]
fn change_outputs_state() {
    let (_machine, gpio) = setup();
    let mut gpio = gpio.borrow_mut();
    for offset in (0..8).map(|port| 4 * port) {
        gpio.write_register(DIR + offset, ALL);
        for value in [ALL, 0] {
            gpio.write_register(PIN + offset, value);
            assert_eq!(gpio.read_register(PIN + offset), value);
            assert_eq!(gpio.read_register(SET + offset), value);
        }
        for (register, input, expected) in [
            (SET, ALL, ALL),
            (SET, 0, ALL),
            (CLR, ALL, 0),
            (CLR, 0, 0),
            (NOT, ALL, ALL),
            (NOT, ALL, 0),
            (NOT, 0, 0),
        ] {
            gpio.write_register(register + offset, input);
            assert_eq!(gpio.read_register(PIN + offset), expected);
        }
    }
}

#[test]
fn change_mask() {
    let (_machine, gpio) = setup();
    let mut gpio = gpio.borrow_mut();
    for offset in (0..8).map(|port| 4 * port) {
        gpio.write_register(MASK + offset, ALL);
        assert_eq!(gpio.read_register(MASK + offset), ALL);
    }
}

#[test]
fn change_masked_ouput_state() {
    let (_machine, gpio) = setup();
    let mut gpio = gpio.borrow_mut();
    for offset in (0..8).map(|port| 4 * port) {
        gpio.write_register(DIR + offset, ALL);
        gpio.write_register(PIN + offset, ALL);
        gpio.write_register(MASK + offset, 0x0000_ffff);
        assert_eq!(gpio.read_register(MPIN + offset), 0xffff_0000);
        gpio.write_register(MPIN + offset, 0);
        assert_eq!(gpio.read_register(PIN + offset), 0x0000_ffff);
        gpio.write_register(MASK + offset, 0xffff_0000);
        assert_eq!(gpio.read_register(MPIN + offset), 0x0000_ffff);
    }
}
