use std::{cell::RefCell, rc::Rc};

use crate::{
    bus_translation::{NativePeripheral, PeripheralHandle, Port},
    cast_canctrl::{CastCan, Frame},
    dma_engine::{DmaBus, Width},
    machine::Machine,
};

const TBUF: usize = 0x50;
const STATUS: usize = 0xa0;
const CMD: usize = 0xa1;
const CONTROL: usize = 0xa2;
const ENABLE: usize = 0xa4;
const FLAGS: usize = 0xa5;
const FILTER: usize = 0xb8;

struct Attachment(Rc<RefCell<CastCan>>);

impl NativePeripheral for Attachment {
    fn read(&mut self, region: Option<&str>, offset: u64) -> u64 {
        self.0.borrow_mut().read(region, offset)
    }

    fn write(&mut self, region: Option<&str>, offset: u64, value: u64) {
        self.0.borrow_mut().write(region, offset, value);
    }

    fn read_width(&mut self, region: Option<&str>, offset: u64, width: Width) -> u64 {
        self.0.borrow_mut().read_width(region, offset, width)
    }

    fn write_width(&mut self, region: Option<&str>, offset: u64, width: Width, value: u64) {
        self.0
            .borrow_mut()
            .write_width(region, offset, width, value);
    }
}

struct Fixture {
    can: Rc<RefCell<CastCan>>,
    machine: Machine,
}

impl Fixture {
    fn new() -> Self {
        let fixture = Self {
            can: Rc::new(RefCell::new(CastCan::default())),
            machine: Machine::without_default_ram(),
        };
        fixture.write(STATUS, 0);
        fixture
    }

    fn read(&self, offset: usize) -> u8 {
        self.can.borrow_mut().read_byte(offset)
    }

    fn write(&self, offset: usize, value: u8) {
        self.can.borrow_mut().write_byte(offset, value);
    }

    fn write_many(&self, registers: &[(usize, u8)]) {
        for &(offset, value) in registers {
            self.write(offset, value);
        }
    }

    fn sent(&self) -> Rc<RefCell<Vec<Frame>>> {
        let frames = Rc::new(RefCell::new(Vec::new()));
        let captured = Rc::clone(&frames);
        self.can
            .borrow_mut()
            .on_frame_sent(move |frame| captured.borrow_mut().push(frame));
        frames
    }

    fn irq(&self) -> bool {
        self.can.borrow().irq()
    }

    fn register(&mut self, base: u32, length: u32, region: Option<&str>) {
        self.machine
            .bus
            .register_translated_port_widths(
                &PeripheralHandle::default(),
                vec![(
                    Port {
                        base,
                        length,
                        region: region.map(str::to_owned),
                    },
                    vec![Width::Byte, Width::DoubleWord],
                )],
                Box::new(Attachment(Rc::clone(&self.can))),
            )
            .unwrap();
    }

    fn bus_write(&mut self, address: u64, width: Width, value: u64) {
        self.machine
            .bus
            .write_unit(address, width, value, &mut std::io::sink())
            .unwrap();
    }

    fn bus_read(&mut self, address: u64, width: Width) -> u64 {
        self.machine.bus.read_unit(address, width).unwrap()
    }
}

#[test]
fn documented_registers_and_reset_values() {
    let f = Fixture::new();
    f.can.borrow_mut().reset();
    for (offset, expected) in [
        (STATUS, 0x80),
        (CONTROL, 0x84),
        (ENABLE, 0xfe),
        (0xa7, 0x1b),
        (0xa8, 3),
        (0xa9, 2),
        (0xaa, 2),
        (0xab, 1),
        (0xb5, 2),
        (0xb6, 1),
        (0xb7, 0),
        (0xbc, 9),
        (0xbd, 7),
        (0xd0, 1),
    ] {
        assert_eq!(f.read(offset), expected, "register {offset:#x}");
    }
    f.write(0xb4, 0x20);
    assert_eq!(
        [0, 1, 2, 3].map(|i| f.read(FILTER + i)),
        [0xff, 0xff, 0xff, 0x1f]
    );
    f.write_many(&[(FILTER, 0x55), (0xb4, 0)]);
    assert_eq!(f.read(FILTER), 0);
    f.write_many(&[(FILTER, 0xaa), (0xb4, 0x20)]);
    assert_eq!(f.read(FILTER), 0x55);
}

#[test]
fn transmit_full_length_fd_frame() {
    let f = Fixture::new();
    let sent = f.sent();
    for (i, value) in [0x67, 0x45, 0x23, 0x01, 0xbf].into_iter().enumerate() {
        f.write(TBUF + i, value);
    }
    for i in 0..64 {
        f.write(TBUF + 8 + i, (i + 1) as u8);
    }
    f.write(CMD, 0x10);
    let sent = sent.borrow();
    let frame = sent.last().expect("sent frame");
    assert_eq!(frame.id, 0x1234567);
    assert!(frame.extended);
    assert!(frame.fd);
    assert!(frame.bit_rate_switch);
    assert_eq!(frame.data, (1..=64).collect::<Vec<_>>());
    assert_eq!(f.read(FLAGS), 8);
    assert!(f.irq());
    f.write(FLAGS, 8);
    assert_eq!(f.read(FLAGS), 0);
    assert!(!f.irq());
}

#[test]
fn discard_secondary_requests_on_empty_buffer() {
    let f = Fixture::new();
    let sent = f.sent();
    f.write_many(&[
        (CMD, 0x80),
        (TBUF, 0x23),
        (TBUF + 1, 1),
        (TBUF + 4, 2),
        (TBUF + 8, 0xa5),
        (TBUF + 9, 0x5a),
        (CMD, 0x84),
    ]);
    assert_eq!(f.read(CMD), 0x80);
    f.write(CMD, 0x82);
    assert_eq!(f.read(CMD), 0x80);
    assert!(sent.borrow().is_empty());
    f.write(CONTROL, 0x40);
    assert!(sent.borrow().is_empty());
    assert_eq!(f.read(FLAGS), 0);
    f.write(CMD, 0x84);
    let sent = sent.borrow();
    let frame = sent.last().expect("sent frame");
    assert_eq!(frame.id, 0x123);
    assert_eq!(frame.data, [0xa5, 0x5a]);
    assert_eq!(f.read(FLAGS), 4);
}

#[test]
fn accept_tsnext_and_tsone_in_same_double_word_write() {
    let mut f = Fixture::new();
    let base = 0x1000;
    f.register(base as u32, 0x1000, None);
    let sent = f.sent();
    f.bus_write(base + CMD as u64, Width::Byte, 0x80);
    f.bus_write(base + TBUF as u64, Width::DoubleWord, 0x123);
    f.bus_write(base + TBUF as u64 + 4, Width::DoubleWord, 1);
    f.bus_write(base + TBUF as u64 + 8, Width::DoubleWord, 0xab);
    f.bus_write(base + STATUS as u64, Width::DoubleWord, 0x00408400);
    let sent = sent.borrow();
    let frame = sent.last().expect("sent frame");
    assert_eq!(frame.id, 0x123);
    assert_eq!(frame.data, [0xab]);
    assert_eq!(f.bus_read(base + FLAGS as u64, Width::Byte), 4);
}

#[test]
fn abort_queued_secondary_frames_without_transmitting() {
    let f = Fixture::new();
    let sent = f.sent();
    f.write_many(&[
        (CMD, 0x80),
        (TBUF, 0x23),
        (TBUF + 4, 1),
        (TBUF + 8, 0xab),
        (CONTROL, 0x40),
        (CONTROL, 0x40),
        (ENABLE, 0),
        (CMD, 0x83),
    ]);
    assert!(sent.borrow().is_empty());
    assert_eq!(f.read(CONTROL) & 0x43, 0);
    assert_eq!(f.read(CMD), 0x80);
    assert_eq!(f.read(FLAGS), 1);
    assert!(f.irq());
    f.write(FLAGS, 1);
    assert_eq!(f.read(FLAGS), 0);
    assert!(!f.irq());
}

#[test]
fn keep_primary_buffer_when_writing_secondary_buffer() {
    let f = Fixture::new();
    let sent = f.sent();
    f.write_many(&[
        (TBUF, 0x11),
        (TBUF + 4, 1),
        (TBUF + 8, 0xaa),
        (CMD, 0x80),
        (TBUF, 0x22),
        (TBUF + 4, 1),
        (TBUF + 8, 0xbb),
        (CONTROL, 0x40),
        (ENABLE, 0),
        (CMD, 0x84),
        (CMD, 0),
        (CMD, 0x10),
    ]);
    let sent = sent.borrow();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].id, 0x22);
    assert_eq!(sent[0].data, [0xbb]);
    assert_eq!(sent[1].id, 0x11);
    assert_eq!(sent[1].data, [0xaa]);
    assert_eq!(f.read(FLAGS), 0);
    f.write(ENABLE, 0x0c);
    assert!(!f.irq());
}

#[test]
fn access_counter_with_bytes() {
    let mut f = Fixture::new();
    f.register(0x2000, 0x100, Some("counter"));
    f.bus_write(0x2004, Width::DoubleWord, 0x12345678);
    assert_eq!(f.bus_read(0x2005, Width::Byte), 0x56);
    f.bus_write(0x2005, Width::Byte, 0xab);
    assert_eq!(f.bus_read(0x2004, Width::DoubleWord), 0x1234ab78);
    f.bus_write(0x2000, Width::Byte, 4);
    assert_eq!(f.bus_read(0x2004, Width::DoubleWord), 0);
}
