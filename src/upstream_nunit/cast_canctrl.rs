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

const RCTRL: usize = 0xa3;

impl Fixture {
    fn receive(&self, id: u32, data: &[u8], control: u8) {
        self.can.borrow_mut().receive(Frame {
            id,
            data: data.to_vec(),
            extended: control & 0x80 != 0,
            remote: control & 0x40 != 0,
            fd: control & 0x20 != 0,
            bit_rate_switch: control & 0x10 != 0,
        });
    }
}

#[test]
fn receive_remote_frame_length_without_payload() {
    let f = Fixture::new();
    f.receive(0x123, &[0xa5; 4], 0x40);
    assert_eq!(f.read(4), 0x44);
    assert!((8..16).all(|offset| f.read(offset) == 0));
}

#[test]
fn latch_receive_flag_only_if_enabled_at_reception() {
    let f = Fixture::new();
    f.write(ENABLE, 0);
    f.receive(0x123, &[0xaa], 0);
    assert_eq!(f.read(FLAGS), 0);
    assert!(!f.irq());
    f.write(ENABLE, 0x80);
    assert!(!f.irq());
    f.receive(0x124, &[0xbb], 0);
    assert_eq!(f.read(FLAGS), 0x80);
    assert!(f.irq());
    f.write(FLAGS, 0x80);
    assert!(!f.irq());
}

#[test]
fn expose_oldest_frame_and_advance_on_release() {
    let f = Fixture::new();
    let fd_data: Vec<_> = (1..=12).collect();
    f.receive(0x321, &[0xaa, 0xbb], 0);
    f.receive(0x1234567, &fd_data, 0xb0);

    assert_eq!(f.read(RCTRL) & 3, 2);
    assert_eq!(
        [0, 1, 4, 8, 9].map(|offset| f.read(offset)),
        [0x21, 3, 2, 0xaa, 0xbb]
    );
    f.write(RCTRL, 0x10);
    assert_eq!(f.read(RCTRL) & 3, 1);
    assert_eq!(
        [0, 1, 2, 3, 4].map(|offset| f.read(offset)),
        [0x67, 0x45, 0x23, 1, 0xb9]
    );
    assert_eq!(
        (8..20).map(|offset| f.read(offset)).collect::<Vec<_>>(),
        fd_data
    );
    f.write(RCTRL, 0x10);
    assert_eq!(f.read(RCTRL) & 3, 0);
}

#[test]
fn apply_acceptance_mask_and_frame_type() {
    let f = Fixture::new();
    f.write(0xb6, 0);
    f.receive(0x123, &[0xaa], 0);
    assert_eq!(f.read(RCTRL) & 3, 0);

    f.write_many(&[
        (STATUS, 0x80),
        (FILTER, 0x23),
        (FILTER + 1, 1),
        (0xb4, 0x20),
        (FILTER, 0),
        (FILTER + 1, 0xf8),
        (FILTER + 2, 0xff),
        (FILTER + 3, 0x5f),
        (0xb6, 1),
        (STATUS, 0),
    ]);
    f.write(FILTER, 0xff);
    assert_eq!(f.read(FILTER), 0);

    f.receive(0x124, &[0xbb], 0);
    f.receive(0x123, &[0xcc], 0x80);
    assert_eq!(f.read(RCTRL) & 3, 0);
    assert_eq!(f.read(FLAGS), 0);
    f.receive(0x123, &[0xdd], 0);
    assert_eq!(f.read(RCTRL) & 3, 1);
    assert_eq!(f.read(8), 0xdd);
}

#[test]
fn signal_almost_full_at_scaled_warning_limit() {
    let f = Fixture::new();
    f.write_many(&[(0xa7, 0x2b), (ENABLE, 0x10)]);
    for id in 0..3 {
        f.receive(id, &[], 0);
    }
    assert_eq!(f.read(RCTRL) & 3, 1);
    assert_eq!(f.read(FLAGS), 0);
    assert!(!f.irq());

    f.receive(3, &[], 0);
    assert_eq!(f.read(RCTRL) & 3, 2);
    assert_eq!(f.read(FLAGS), 0x10);
    assert!(f.irq());
    f.write(FLAGS, 0x10);
    assert!(!f.irq());
    f.receive(4, &[], 0);
    assert_eq!(f.read(FLAGS), 0);
    f.write_many(&[(RCTRL, 0x10), (RCTRL, 0x10)]);
    assert_eq!(f.read(RCTRL) & 3, 1);
}

#[test]
fn handle_both_receive_overflow_modes_and_software_reset() {
    let f = Fixture::new();
    for id in 0..16 {
        f.receive(id, &[], 0);
    }
    assert_eq!(f.read(RCTRL) & 3, 3);
    f.write(FLAGS, 0xf0);
    f.receive(16, &[], 0);
    assert_eq!(f.read(RCTRL) & 0x23, 0x23);
    assert_eq!(f.read(FLAGS), 0xe0);
    assert_eq!(f.read(0), 1);
    assert!(f.irq());

    f.write(STATUS, 0x80);
    assert_eq!(f.read(RCTRL) & 0x23, 0);
    assert_eq!(f.read(FLAGS), 0);
    assert!(!f.irq());
    f.receive(0x123, &[], 0);
    assert_eq!(f.read(RCTRL) & 3, 0);
    f.write(STATUS, 0);

    f.write(RCTRL, 0x40);
    for id in 0..16 {
        f.receive(id, &[], 0);
    }
    f.write(FLAGS, 0xf0);
    f.receive(16, &[], 0);
    assert_eq!(f.read(RCTRL) & 0x63, 0x63);
    assert_eq!(f.read(FLAGS), 0x60);
    assert_eq!(f.read(0), 0);
    f.write(RCTRL, 0x50);
    assert_eq!(f.read(RCTRL) & 0x20, 0);
    assert_eq!(f.read(0), 1);
}
