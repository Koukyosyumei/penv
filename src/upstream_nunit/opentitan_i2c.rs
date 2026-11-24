use std::{cell::RefCell, rc::Rc};

use crate::{
    bus_translation::{NativePeripheral, Port},
    dma_engine::{DmaBus, Width},
    efr32_i2c::Slave,
    machine::Machine,
    nrf52840::bmp180::Bmp180,
    opentitan_i2c::OpenTitanI2c,
    pac1934::Pac1934,
};

const BASE: u64 = 0x1000;
const CONTROL: u64 = 0x10;
const STATUS: u64 = 0x14;
const RX: u64 = 0x18;
const FORMAT: u64 = 0x1c;
const FIFO_CONTROL: u64 = 0x20;
const FIFO_STATUS: u64 = 0x24;
const ACQUIRE: u64 = 0x4c;
const TX: u64 = 0x50;
const SENSOR: u32 = 0x77;
const START: u32 = 0x100;
const STOP: u32 = 0x200;
const READ: u32 = 0x400;

struct Registration(Rc<RefCell<OpenTitanI2c>>);

impl NativePeripheral for Registration {
    fn read(&mut self, region: Option<&str>, offset: u64) -> u64 {
        NativePeripheral::read(&mut *self.0.borrow_mut(), region, offset)
    }

    fn write(&mut self, region: Option<&str>, offset: u64, value: u64) {
        NativePeripheral::write(&mut *self.0.borrow_mut(), region, offset, value)
    }
}

struct Fixture {
    controller: Rc<RefCell<OpenTitanI2c>>,
    machine: Machine,
}

impl Fixture {
    fn new() -> Self {
        let mut machine = Machine::without_default_ram();
        let controller = Rc::new(RefCell::new(OpenTitanI2c::default()));
        machine
            .bus
            .map_translated_peripheral(
                vec![Port {
                    base: BASE as u32,
                    length: 0x1000,
                    region: None,
                }],
                Width::DoubleWord,
                Box::new(Registration(controller.clone())),
            )
            .unwrap();
        controller.borrow_mut().register(0x77, Bmp180::new(0.0, 0));
        controller.borrow_mut().register(0x78, Pac1934::default());
        Self {
            controller,
            machine,
        }
    }

    fn read(&mut self, offset: u64) -> u32 {
        self.machine
            .bus
            .read_unit(BASE + offset, Width::DoubleWord)
            .unwrap() as u32
    }

    fn write(&mut self, offset: u64, value: u32) {
        self.machine
            .bus
            .write_unit(
                BASE + offset,
                Width::DoubleWord,
                value.into(),
                &mut std::io::sink(),
            )
            .unwrap();
    }

    fn enqueue_read(&mut self, sensor: u32, register: u32) {
        self.write(FORMAT, START | sensor);
        self.write(FORMAT, register);
        self.write(FORMAT, READ | STOP | 1);
    }

    fn finish(&self) {
        self.controller.borrow_mut().finish_transmission().unwrap();
    }
}

#[test]
fn should_perform_simple_transaction() {
    let mut f = Fixture::new();
    f.write(CONTROL, 1);
    f.enqueue_read(SENSOR, 0xd0);
    assert_eq!(f.read(RX), 0x55);
}

#[test]
fn should_not_start_transactions_until_host_is_enabled() {
    let mut f = Fixture::new();
    f.enqueue_read(SENSOR, 0xd0);
    assert_eq!(f.read(RX), 0);
    assert_eq!(f.read(STATUS) & 4, 0);
    assert_eq!(f.read(STATUS) & 0x20, 0x20);
    f.write(CONTROL, 1);
    assert_eq!(f.read(STATUS) & 0x20, 0);
    assert_eq!(f.read(RX), 0x55);
}

#[test]
fn should_set_exception_on_fmt_watermark() {
    let mut f = Fixture::new();
    f.write(4, 1);
    f.write(FIFO_CONTROL, 1 << 5);
    for _ in 0..3 {
        f.write(FORMAT, SENSOR | START);
    }
    assert_eq!(f.read(0), 0);
    f.write(FORMAT, SENSOR | START);
    assert_eq!(f.read(0) & 1, 1);
    assert!(f.controller.borrow().irq(1));
}

#[test]
fn should_set_exception_on_rx_watermark() {
    let mut f = Fixture::new();
    f.write(4, 2);
    f.write(CONTROL, 1);
    f.write(FORMAT, SENSOR | START);
    f.write(FORMAT, 0xd0);
    assert_eq!(f.read(0) & 2, 0);
    f.write(FORMAT, 1 | READ | STOP);
    assert_eq!(f.read(0) & 2, 2);
    assert!(f.controller.borrow().irq(2));
}

#[test]
fn should_set_exception_on_overflow() {
    let mut f = Fixture::new();
    f.write(4, 4);
    for _ in 0..64 {
        f.write(FORMAT, SENSOR | START);
    }
    assert_eq!(f.read(0) & 4, 0);
    f.write(FORMAT, SENSOR | START);
    assert_eq!(f.read(0) & 4, 4);
    assert!(f.controller.borrow().irq(4));
}

#[test]
fn should_be_able_to_reset_fmt_fifo() {
    let mut f = Fixture::new();
    f.enqueue_read(SENSOR, 0xd0);
    assert_eq!(f.read(STATUS) & 4, 0);
    f.write(FIFO_CONTROL, 2);
    assert_eq!(f.read(STATUS) & 4, 4);
}

#[test]
fn should_be_able_to_reset_rx_fifo() {
    let mut f = Fixture::new();
    f.write(CONTROL, 1);
    f.enqueue_read(SENSOR, 0xd0);
    assert_eq!(f.read(STATUS) & 0x20, 0);
    f.write(FIFO_CONTROL, 1);
    assert_eq!(f.read(STATUS) & 0x20, 0x20);
}

#[test]
fn should_not_allow_enabling_both_modes_at_the_same_time() {
    let mut f = Fixture::new();
    f.write(CONTROL, 2);
    f.write(CONTROL, 1);
    assert_eq!(f.read(CONTROL), 1);
}

#[test]
fn should_not_accept_commands_when_in_target_mode() {
    let mut f = Fixture::new();
    f.write(CONTROL, 2);
    f.write(FORMAT, SENSOR | START);
    assert_eq!(f.read(STATUS) & 4, 4);
}

fn transmit_queue(mode: u32, expected: u32) {
    let mut f = Fixture::new();
    f.write(CONTROL, mode);
    for byte in [1, 2, 3, 4, 5, 6, 7, 8, 9] {
        f.write(TX, byte);
    }
    assert_eq!((f.read(FIFO_STATUS) >> 8) & 0x7f, expected);
}

#[test]
fn should_queue_transmit_bytes_when_in_target_mode() {
    transmit_queue(2, 9);
}
#[test]
fn should_not_queue_transmission_when_in_host_mode() {
    transmit_queue(1, 0);
}

#[test]
fn target_should_interpret_transmission_complete_as_stop() {
    let mut f = Fixture::new();
    f.write(CONTROL, 2);
    f.finish();
    assert_eq!(f.read(ACQUIRE), STOP);
}

#[test]
fn target_should_correctly_assign_rwflag() {
    let mut f = Fixture::new();
    f.write(CONTROL, 2);
    let address = 0x7f;
    for direction in [1, 0] {
        Slave::write(
            &mut *f.controller.borrow_mut(),
            &[(address << 1) | direction],
        );
        f.finish();
    }
    for direction in [1, 0] {
        let packet = f.read(ACQUIRE);
        assert_eq!(packet & 0xff, (u32::from(address) << 1) | direction);
        assert_eq!(packet & (START | 1) == START | 1, direction == 1);
        assert_eq!(packet & START, START);
        assert_eq!(packet & STOP, 0);
        assert_eq!(f.read(ACQUIRE) & STOP, STOP);
    }
}

#[test]
fn target_should_properly_handle_read() {
    let mut f = Fixture::new();
    f.write(CONTROL, 2);
    let data = [1, 2, 0xff, 3, 4, 0xfe];
    for byte in data {
        f.write(TX, byte.into());
    }
    let read = Slave::read(&mut *f.controller.borrow_mut(), 6);
    assert_eq!(read.len(), 6);
    assert_eq!(read, data);
}

#[test]
fn target_should_not_return_more_than_queued() {
    let mut f = Fixture::new();
    f.write(CONTROL, 2);
    f.write(TX, 1);
    assert_eq!(Slave::read(&mut *f.controller.borrow_mut(), 10).len(), 1);
}

fn target_fifo(acquire: bool) {
    let mut f = Fixture::new();
    f.write(CONTROL, 2);
    let (empty, full, level, reset) = if acquire {
        (0x200, 0x80, 24, 0x80)
    } else {
        (0x100, 0x40, 8, 0x100)
    };
    assert_eq!(f.read(STATUS) & empty, empty);
    for size in [4, 60] {
        for _ in 0..size {
            if acquire {
                Slave::write(&mut *f.controller.borrow_mut(), &[1]);
            } else {
                f.write(TX, 1);
            }
        }
        if size == 4 {
            assert_eq!(f.read(STATUS) & empty, 0);
            let value = f.read(FIFO_STATUS) >> level;
            assert_eq!(if acquire { value & 0x7f } else { value }, 4);
        }
    }
    assert_eq!(f.read(STATUS) & full, full);
    assert_eq!(f.read(FIFO_STATUS) >> level, 64);
    f.write(FIFO_CONTROL, reset);
    assert_eq!(f.read(STATUS) & empty, empty);
}
#[test]
fn should_correctly_show_status_of_the_acq_fifo() {
    target_fifo(true);
}
#[test]
fn should_correctly_show_status_of_the_tx_fifo() {
    target_fifo(false);
}

#[test]
fn should_be_able_to_do_two_consecutive_transmissions() {
    let mut f = Fixture::new();
    f.write(CONTROL, 1);
    f.enqueue_read(SENSOR, 0xd0);
    assert_eq!(f.read(RX), 0x55);
    f.enqueue_read(0x78, 0xfd);
    assert_eq!(f.read(RX), 0x5b);
}
