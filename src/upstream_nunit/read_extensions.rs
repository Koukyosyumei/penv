use crate::{
    access_translation::{Endian, read_using},
    dma_engine::Width::{self, Byte, DoubleWord, QuadWord, Word},
};

fn check(native: Width, requested: Width, endian: Endian, expected: &[(u64, u64)]) {
    // The four mocks expose distinct data for their native bus widths.
    let read = |address| match native {
        Byte => [0x12, 0x34, 0x56, 0x78, 0x90, 0xaa, 0xbb, 0xcc]
            .get(address as usize)
            .copied()
            .unwrap_or(0),
        Word => match address {
            0 => 0x3412,
            2 => 0x7856,
            4 => 0xaa90,
            6 => 0xccbb,
            _ => 0,
        },
        DoubleWord => match address {
            0 => 0x7856_3412,
            4 => 0xccbb_aa90,
            _ => 0,
        },
        QuadWord if address == 0 => 0x7856_3412_2143_6587,
        QuadWord => 0,
    };
    for &(address, value) in expected {
        assert_eq!(
            read_using(address, requested, native, endian, read),
            value,
            "address {address}"
        );
    }
}

macro_rules! cases {
    ($($name:ident: $native:ident => $requested:ident, $endian:ident, $expected:expr;)+) => {$(
        #[test]
        fn $name() {
            check($native, $requested, Endian::$endian, $expected);
        }
    )+};
}

cases! {
    should_read_byte_using_word: Word => Byte, Little, &[(0, 0x12), (1, 0x34), (2, 0x56), (3, 0x78)];
    should_read_byte_using_word_big_endian: Word => Byte, Big, &[(0, 0x34), (1, 0x12), (2, 0x78), (3, 0x56)];
    should_read_byte_using_double_word: DoubleWord => Byte, Little, &[(0, 0x12), (1, 0x34), (2, 0x56), (3, 0x78)];
    should_read_byte_using_double_word_big_endian: DoubleWord => Byte, Big, &[(0, 0x78), (1, 0x56), (2, 0x34), (3, 0x12)];
    should_read_byte_using_quad_word: QuadWord => Byte, Little, &[(0, 0x87), (1, 0x65), (2, 0x43), (3, 0x21), (4, 0x12), (5, 0x34), (6, 0x56), (7, 0x78)];
    should_read_byte_using_quad_word_big_endian: QuadWord => Byte, Big, &[(0, 0x78), (1, 0x56), (2, 0x34), (3, 0x12), (4, 0x21), (5, 0x43), (6, 0x65), (7, 0x87)];
    should_read_word_using_byte: Byte => Word, Little, &[(0, 0x3412), (2, 0x7856)];
    should_read_word_using_byte_big_endian: Byte => Word, Big, &[(0, 0x1234), (2, 0x5678)];
    should_read_word_using_byte_not_aligned: Byte => Word, Little, &[(1, 0x5634), (3, 0x9078)];
    should_read_word_using_byte_not_aligned_big_endian: Byte => Word, Big, &[(1, 0x3456), (3, 0x7890)];
    should_read_word_using_double_word: DoubleWord => Word, Little, &[(0, 0x3412), (2, 0x7856)];
    should_read_word_using_double_word_big_endian: DoubleWord => Word, Big, &[(0, 0x5678), (2, 0x1234)];
    should_read_word_using_quad_word: QuadWord => Word, Little, &[(0, 0x6587), (2, 0x2143), (4, 0x3412), (6, 0x7856)];
    should_read_word_using_quad_word_big_endian: QuadWord => Word, Big, &[(0, 0x5678), (2, 0x1234), (4, 0x4321), (6, 0x8765)];
    should_read_double_word_using_byte: Byte => DoubleWord, Little, &[(0, 0x78563412)];
    should_read_double_word_using_byte_big_endian: Byte => DoubleWord, Big, &[(0, 0x12345678)];
    should_read_double_word_using_byte_not_aligned: Byte => DoubleWord, Little, &[(1, 0x90785634)];
    should_read_double_word_using_byte_not_aligned_big_endian: Byte => DoubleWord, Big, &[(1, 0x34567890)];
    should_read_double_word_using_word: Word => DoubleWord, Little, &[(0, 0x78563412)];
    should_read_double_word_using_word_big_endian: Word => DoubleWord, Big, &[(0, 0x12345678)];
    should_read_double_word_using_quad_word: QuadWord => DoubleWord, Little, &[(0, 0x21436587), (4, 0x78563412)];
    should_read_double_word_using_quad_word_big_endian: QuadWord => DoubleWord, Big, &[(0, 0x12345678), (4, 0x87654321)];
    should_read_quad_word_using_byte: Byte => QuadWord, Little, &[(0, 0xccbbaa9078563412)];
    should_read_quad_word_using_byte_big_endian: Byte => QuadWord, Big, &[(0, 0x1234567890aabbcc)];
    should_read_quad_word_using_byte_not_aligned: Byte => QuadWord, Little, &[(1, 0xccbbaa90785634)];
    should_read_quad_word_using_byte_not_aligned_big_endian: Byte => QuadWord, Big, &[(1, 0x34567890aabbcc00)];
    should_read_quad_word_using_word: Word => QuadWord, Little, &[(0, 0xccbbaa9078563412)];
    should_read_quad_word_using_word_big_endian: Word => QuadWord, Big, &[(0, 0x1234567890aabbcc)];
    should_read_quad_word_using_double_word: DoubleWord => QuadWord, Little, &[(0, 0xccbbaa9078563412)];
    should_read_quad_word_using_double_word_big_endian: DoubleWord => QuadWord, Big, &[(0, 0x1234567890aabbcc)];
}
