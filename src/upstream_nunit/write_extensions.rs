use crate::{
    access_translation::{Endian, NativeWrite, write_using},
    dma_engine::Width::{self, Byte, DoubleWord, QuadWord, Word},
};
use std::collections::HashMap;

#[derive(Default)]
struct Peripheral {
    reads: HashMap<u64, u64>,
    writes: Vec<(u64, u64)>,
}

impl NativeWrite for Peripheral {
    fn read_for_write(&mut self, address: u64) -> u64 {
        self.reads.get(&address).copied().unwrap_or_default()
    }

    fn write_native(&mut self, address: u64, value: u64) {
        self.writes.push((address, value));
    }
}

#[derive(Clone, Copy)]
enum Op {
    Call(u64, u64),
    Setup(u64, u64),
    Verify(u64, u64, bool),
}
use Op::*;

fn check(native: Width, requested: Width, endian: Endian, ops: &[Op]) {
    let mut mock = Peripheral::default();
    for &op in ops {
        match op {
            Call(address, value) => {
                write_using(address, value, requested, native, endian, &mut mock)
            }
            Setup(address, result) => {
                mock.reads.insert(address, result);
            }
            Verify(address, value, once) => {
                let calls = mock
                    .writes
                    .iter()
                    .filter(|&&pair| pair == (address, value))
                    .count();
                assert!(
                    if once { calls == 1 } else { calls > 0 },
                    "native write ({address:#x}, {value:#x}) observed {calls} times in {:?}",
                    mock.writes
                );
            }
        }
    }
}

fn quad_bytes(endian: Endian) {
    let bytes = [0x12, 0x34, 0x56, 0x78, 0x87, 0x65, 0x43, 0x21];
    let mut mock = Peripheral::default();
    let mut expected = 0;
    for (offset, &byte) in bytes[..7].iter().enumerate() {
        write_using(offset as u64, byte, Byte, QuadWord, endian, &mut mock);
        let lane = match endian {
            Endian::Little => offset,
            Endian::Big => 7 - offset,
        };
        expected |= byte << (8 * lane);
        assert!(mock.writes.contains(&(0, expected)));
        mock.reads.insert(0, expected);
    }
}

#[test]
fn should_write_byte_using_quad_word() {
    quad_bytes(Endian::Little);
}

#[test]
fn should_write_byte_using_quad_word_big_endian() {
    quad_bytes(Endian::Big);
}

macro_rules! cases {
    ($($name:ident: $native:ident => $requested:ident, $endian:ident, $ops:expr;)+) => {$(
        #[test]
        fn $name() {
            check($native, $requested, Endian::$endian, $ops);
        }
    )+};
}

cases! {
    should_write_word_using_byte: Byte => Word, Little, &[
        Call(0, 0x3412),
        Call(2, 0x7856),
        Verify(0, 0x12, true),
        Verify(1, 0x34, true),
        Verify(2, 0x56, true),
        Verify(3, 0x78, true),
    ];
    should_write_word_using_byte_big_endian: Byte => Word, Big, &[
        Call(0, 0x3412),
        Call(2, 0x7856),
        Verify(0, 0x34, true),
        Verify(1, 0x12, true),
        Verify(2, 0x78, true),
        Verify(3, 0x56, true),
    ];
    should_write_double_word_using_byte: Byte => DoubleWord, Little, &[
        Call(0, 0x78563412),
        Verify(0, 0x12, true),
        Verify(1, 0x34, true),
        Verify(2, 0x56, true),
        Verify(3, 0x78, true),
    ];
    should_write_double_word_using_byte_big_endian: Byte => DoubleWord, Big, &[
        Call(0, 0x78563412),
        Verify(0, 0x78, true),
        Verify(1, 0x56, true),
        Verify(2, 0x34, true),
        Verify(3, 0x12, true),
    ];
    should_write_quad_word_using_byte: Byte => QuadWord, Little, &[
        Call(0, 0x7856341221436587),
        Verify(0, 0x87, true),
        Verify(1, 0x65, true),
        Verify(2, 0x43, true),
        Verify(3, 0x21, true),
        Verify(4, 0x12, true),
        Verify(5, 0x34, true),
        Verify(6, 0x56, true),
        Verify(7, 0x78, true),
    ];
    should_write_quad_word_using_byte_big_endian: Byte => QuadWord, Big, &[
        Call(0, 0x7856341221436587),
        Verify(0, 0x78, true),
        Verify(1, 0x56, true),
        Verify(2, 0x34, true),
        Verify(3, 0x12, true),
        Verify(4, 0x21, true),
        Verify(5, 0x43, true),
        Verify(6, 0x65, true),
        Verify(7, 0x87, true),
    ];
    should_write_byte_using_word: Word => Byte, Little, &[
        Call(0, 0x12),
        Verify(0, 0x0012, false),
        Setup(0, 0x0012),
        Call(1, 0x34),
        Verify(0, 0x3412, false),
        Call(2, 0x56),
        Verify(2, 0x0056, false),
        Setup(2, 0x0056),
        Call(3, 0x78),
        Verify(2, 0x7856, false),
    ];
    should_write_byte_using_word_big_endian: Word => Byte, Big, &[
        Call(0, 0x12),
        Verify(0, 0x1200, false),
        Setup(0, 0x1200),
        Call(1, 0x34),
        Verify(0, 0x1234, false),
        Call(2, 0x56),
        Verify(2, 0x5600, false),
        Setup(2, 0x5600),
        Call(3, 0x78),
        Verify(2, 0x5678, false),
    ];
    should_write_double_word_using_word: Word => DoubleWord, Little, &[
        Call(0, 0x78563412),
        Verify(0, 0x3412, false),
        Verify(2, 0x7856, false),
    ];
    should_write_double_word_using_word_big_endian: Word => DoubleWord, Big, &[
        Call(0, 0x78563412),
        Verify(0, 0x5678, false),
        Verify(2, 0x1234, false),
    ];
    should_write_quad_word_using_word: Word => QuadWord, Little, &[
        Call(0, 0x7856341221436587),
        Verify(0, 0x6587, false),
        Verify(2, 0x2143, false),
        Verify(4, 0x3412, false),
        Verify(6, 0x7856, false),
    ];
    should_write_quad_word_using_word_big_endian: Word => QuadWord, Big, &[
        Call(0, 0x7856341221436587),
        Verify(0, 0x5678, false),
        Verify(2, 0x1234, false),
        Verify(4, 0x4321, false),
        Verify(6, 0x8765, false),
    ];
    should_write_byte_using_double_word: DoubleWord => Byte, Little, &[
        Call(0, 0x12),
        Verify(0, 0x12, false),
        Setup(0, 0x12),
        Call(1, 0x34),
        Verify(0, 0x3412, false),
        Setup(0, 0x3412),
        Call(2, 0x56),
        Verify(0, 0x563412, false),
        Setup(0, 0x563412),
        Call(3, 0x78),
        Verify(0, 0x78563412, false),
    ];
    should_write_byte_using_double_word_big_endian: DoubleWord => Byte, Big, &[
        Call(0, 0x12),
        Verify(0, 0x12000000, false),
        Setup(0, 0x12000000),
        Call(1, 0x34),
        Verify(0, 0x12340000, false),
        Setup(0, 0x12340000),
        Call(2, 0x56),
        Verify(0, 0x12345600, false),
        Setup(0, 0x12345600),
        Call(3, 0x78),
        Verify(0, 0x12345678, false),
    ];
    should_write_word_using_double_word: DoubleWord => Word, Little, &[
        Call(0, 0x3412),
        Verify(0, 0x3412, false),
        Setup(0, 0x3412),
        Call(2, 0x7856),
        Verify(0, 0x78563412, false),
    ];
    should_write_word_using_double_word_big_endian: DoubleWord => Word, Big, &[
        Call(0, 0x3412),
        Verify(0, 0x12340000, false),
        Setup(0, 0x12340000),
        Call(2, 0x7856),
        Verify(0, 0x12345678, false),
    ];
    should_write_quad_word_using_double_word: DoubleWord => QuadWord, Little, &[
        Call(0, 0x7856341221436587),
        Verify(0, 0x21436587, false),
        Verify(4, 0x78563412, false),
    ];
    should_write_quad_word_using_double_word_big_endian: DoubleWord => QuadWord, Big, &[
        Call(0, 0x7856341221436587),
        Verify(0, 0x12345678, false),
        Verify(4, 0x87654321, false),
    ];
    should_write_word_using_double_word_not_aligned1: DoubleWord => Word, Little, &[
        Setup(0, 0x78563412),
        Setup(4, 0xccbbaa90),
        Call(1, 0xdfef),
        Verify(0, 0x78dfef12, true),
    ];
    should_write_word_using_double_word_not_aligned1_big_endian: DoubleWord => Word, Big, &[
        Setup(0, 0x78563412),
        Setup(4, 0xccbbaa90),
        Call(1, 0xdfef),
        Verify(0, 0x78efdf12, true),
    ];
    should_write_word_using_quad_word: QuadWord => Word, Little, &[
        Call(0, 0x3412),
        Verify(0, 0x3412, false),
        Setup(0, 0x3412),
        Call(2, 0x7856),
        Verify(0, 0x78563412, false),
        Setup(0, 0x78563412),
        Call(4, 0x5678),
        Verify(0, 0x567878563412, false),
        Setup(0, 0x567878563412),
        Call(6, 0x1234),
        Verify(0, 0x1234567878563412, false),
    ];
    should_write_word_using_quad_word_big_endian: QuadWord => Word, Big, &[
        Call(0, 0x3412),
        Verify(0, 0x1234000000000000, false),
        Setup(0, 0x1234000000000000),
        Call(2, 0x7856),
        Verify(0, 0x1234567800000000, false),
        Setup(0, 0x1234567800000000),
        Call(4, 0x6587),
        Verify(0, 0x1234567887650000, false),
        Setup(0, 0x1234567887650000),
        Call(6, 0x2143),
        Verify(0, 0x1234567887654321, false),
    ];
    should_write_double_word_using_quad_word: QuadWord => DoubleWord, Little, &[
        Call(0, 0x78563412),
        Verify(0, 0x78563412, false),
        Setup(0, 0x78563412),
        Call(4, 0x12345678),
        Verify(0, 0x1234567878563412, false),
    ];
    should_write_double_word_using_quad_word_big_endian: QuadWord => DoubleWord, Big, &[
        Call(0, 0x78563412),
        Verify(0, 0x1234567800000000, false),
        Setup(0, 0x1234567800000000),
        Call(4, 0x21436587),
        Verify(0, 0x1234567887654321, false),
    ];
}
