use crate::{
    dma_engine::{Access, Destination, Request, Source, Width, issue_copy},
    machine::Machine,
};

const SIZE: usize = 5 * 1024;
const MAPPED_SOURCE: u32 = 0;
const MAPPED_DESTINATION: u32 = 0x10000;
const ARRAY_SOURCE: u32 = 0x20000;
const ARRAY_DESTINATION: u32 = 0x30000;
const PATTERN: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

#[derive(Clone, Copy)]
enum Place {
    Raw,
    Array,
    Mapped,
}

use Place::*;
use Width::*;

fn transfer(source: Place, destination: Place, read: Width, write: Width) {
    let mut machine = Machine::without_default_ram();
    let bus = &mut machine.bus;
    let mut raw_source = vec![0; SIZE];
    let mut raw_destination = vec![0; SIZE];
    for address in [MAPPED_SOURCE, MAPPED_DESTINATION] {
        bus.map_ram(address, SIZE).unwrap();
    }
    for address in [ARRAY_SOURCE, ARRAY_DESTINATION] {
        bus.map_array_memory(address, SIZE).unwrap();
    }

    let source_bytes = match source {
        Raw => &mut raw_source,
        Array => bus.memory64(ARRAY_SOURCE.into(), SIZE).unwrap(),
        Mapped => bus.memory64(MAPPED_SOURCE.into(), SIZE).unwrap(),
    };
    for chunk in source_bytes.chunks_exact_mut(PATTERN.len()) {
        chunk.copy_from_slice(&PATTERN);
    }
    assert_eq!(source_bytes[..16], PATTERN);
    const COUNT: usize = 32;
    const READ: usize = 8;
    const WRITE: usize = 32;
    let expected = source_bytes[READ..READ + COUNT].to_vec();

    let from = match source {
        Raw => Source::Array(&raw_source, READ),
        Array => Source::Bus(u64::from(ARRAY_SOURCE) + READ as u64),
        Mapped => Source::Bus(u64::from(MAPPED_SOURCE) + READ as u64),
    };
    let to = match destination {
        Raw => Destination::Array(&mut raw_destination, WRITE),
        Array => Destination::Bus(u64::from(ARRAY_DESTINATION) + WRITE as u64),
        Mapped => Destination::Bus(u64::from(MAPPED_DESTINATION) + WRITE as u64),
    };
    issue_copy(
        bus,
        Request {
            source: from,
            destination: to,
            size: COUNT,
            read: Access::new(read),
            write: Access::new(write),
        },
        &mut std::io::sink(),
    )
    .unwrap();
    let destination_bytes = match destination {
        Raw => raw_destination.as_slice(),
        Array => bus.ram64(ARRAY_DESTINATION.into(), SIZE).unwrap(),
        Mapped => bus.ram64(MAPPED_DESTINATION.into(), SIZE).unwrap(),
    };
    assert_eq!(&destination_bytes[WRITE..WRITE + COUNT], expected);
}

macro_rules! cases {
    ($source:ident => $destination:ident; $( $test:ident: $read:ident, $write:ident; )+) => {
        $(
            #[test]
            fn $test() {
                transfer($source, $destination, $read, $write);
            }
        )+
    };
}

cases! {
    Raw => Raw;
    raw_to_raw_1_1: Byte, Byte;
    raw_to_raw_1_2: Byte, Word;
    raw_to_raw_1_4: Byte, DoubleWord;
    raw_to_raw_1_8: Byte, QuadWord;
    raw_to_raw_2_1: Word, Byte;
    raw_to_raw_2_2: Word, Word;
    raw_to_raw_2_4: Word, DoubleWord;
    raw_to_raw_2_8: Word, QuadWord;
    raw_to_raw_4_1: DoubleWord, Byte;
    raw_to_raw_4_2: DoubleWord, Word;
    raw_to_raw_4_4: DoubleWord, DoubleWord;
    raw_to_raw_4_8: DoubleWord, QuadWord;
    raw_to_raw_8_1: QuadWord, Byte;
    raw_to_raw_8_2: QuadWord, Word;
    raw_to_raw_8_4: QuadWord, DoubleWord;
    raw_to_raw_8_8: QuadWord, QuadWord;
}

cases! {
    Raw => Array;
    raw_to_array_1_1: Byte, Byte;
    raw_to_array_1_2: Byte, Word;
    raw_to_array_1_4: Byte, DoubleWord;
    raw_to_array_1_8: Byte, QuadWord;
    raw_to_array_2_1: Word, Byte;
    raw_to_array_2_2: Word, Word;
    raw_to_array_2_4: Word, DoubleWord;
    raw_to_array_2_8: Word, QuadWord;
    raw_to_array_4_1: DoubleWord, Byte;
    raw_to_array_4_2: DoubleWord, Word;
    raw_to_array_4_4: DoubleWord, DoubleWord;
    raw_to_array_4_8: DoubleWord, QuadWord;
    raw_to_array_8_1: QuadWord, Byte;
    raw_to_array_8_2: QuadWord, Word;
    raw_to_array_8_4: QuadWord, DoubleWord;
    raw_to_array_8_8: QuadWord, QuadWord;
}

cases! {
    Raw => Mapped;
    raw_to_mapped_1_1: Byte, Byte;
    raw_to_mapped_1_2: Byte, Word;
    raw_to_mapped_1_4: Byte, DoubleWord;
    raw_to_mapped_1_8: Byte, QuadWord;
    raw_to_mapped_2_1: Word, Byte;
    raw_to_mapped_2_2: Word, Word;
    raw_to_mapped_2_4: Word, DoubleWord;
    raw_to_mapped_2_8: Word, QuadWord;
    raw_to_mapped_4_1: DoubleWord, Byte;
    raw_to_mapped_4_2: DoubleWord, Word;
    raw_to_mapped_4_4: DoubleWord, DoubleWord;
    raw_to_mapped_4_8: DoubleWord, QuadWord;
    raw_to_mapped_8_1: QuadWord, Byte;
    raw_to_mapped_8_2: QuadWord, Word;
    raw_to_mapped_8_4: QuadWord, DoubleWord;
    raw_to_mapped_8_8: QuadWord, QuadWord;
}

cases! {
    Array => Raw;
    array_to_raw_1_1: Byte, Byte;
    array_to_raw_1_2: Byte, Word;
    array_to_raw_1_4: Byte, DoubleWord;
    array_to_raw_1_8: Byte, QuadWord;
    array_to_raw_2_1: Word, Byte;
    array_to_raw_2_2: Word, Word;
    array_to_raw_2_4: Word, DoubleWord;
    array_to_raw_2_8: Word, QuadWord;
    array_to_raw_4_1: DoubleWord, Byte;
    array_to_raw_4_2: DoubleWord, Word;
    array_to_raw_4_4: DoubleWord, DoubleWord;
    array_to_raw_4_8: DoubleWord, QuadWord;
    array_to_raw_8_1: QuadWord, Byte;
    array_to_raw_8_2: QuadWord, Word;
    array_to_raw_8_4: QuadWord, DoubleWord;
    array_to_raw_8_8: QuadWord, QuadWord;
}

cases! {
    Array => Array;
    array_to_array_1_1: Byte, Byte;
    array_to_array_1_2: Byte, Word;
    array_to_array_1_4: Byte, DoubleWord;
    array_to_array_1_8: Byte, QuadWord;
    array_to_array_2_1: Word, Byte;
    array_to_array_2_2: Word, Word;
    array_to_array_2_4: Word, DoubleWord;
    array_to_array_2_8: Word, QuadWord;
    array_to_array_4_1: DoubleWord, Byte;
    array_to_array_4_2: DoubleWord, Word;
    array_to_array_4_4: DoubleWord, DoubleWord;
    array_to_array_4_8: DoubleWord, QuadWord;
    array_to_array_8_1: QuadWord, Byte;
    array_to_array_8_2: QuadWord, Word;
    array_to_array_8_4: QuadWord, DoubleWord;
    array_to_array_8_8: QuadWord, QuadWord;
}

cases! {
    Array => Mapped;
    array_to_mapped_1_1: Byte, Byte;
    array_to_mapped_1_2: Byte, Word;
    array_to_mapped_1_4: Byte, DoubleWord;
    array_to_mapped_1_8: Byte, QuadWord;
    array_to_mapped_2_1: Word, Byte;
    array_to_mapped_2_2: Word, Word;
    array_to_mapped_2_4: Word, DoubleWord;
    array_to_mapped_2_8: Word, QuadWord;
    array_to_mapped_4_1: DoubleWord, Byte;
    array_to_mapped_4_2: DoubleWord, Word;
    array_to_mapped_4_4: DoubleWord, DoubleWord;
    array_to_mapped_4_8: DoubleWord, QuadWord;
    array_to_mapped_8_1: QuadWord, Byte;
    array_to_mapped_8_2: QuadWord, Word;
    array_to_mapped_8_4: QuadWord, DoubleWord;
    array_to_mapped_8_8: QuadWord, QuadWord;
}

cases! {
    Mapped => Raw;
    mapped_to_raw_1_1: Byte, Byte;
    mapped_to_raw_1_2: Byte, Word;
    mapped_to_raw_1_4: Byte, DoubleWord;
    mapped_to_raw_1_8: Byte, QuadWord;
    mapped_to_raw_2_1: Word, Byte;
    mapped_to_raw_2_2: Word, Word;
    mapped_to_raw_2_4: Word, DoubleWord;
    mapped_to_raw_2_8: Word, QuadWord;
    mapped_to_raw_4_1: DoubleWord, Byte;
    mapped_to_raw_4_2: DoubleWord, Word;
    mapped_to_raw_4_4: DoubleWord, DoubleWord;
    mapped_to_raw_4_8: DoubleWord, QuadWord;
    mapped_to_raw_8_1: QuadWord, Byte;
    mapped_to_raw_8_2: QuadWord, Word;
    mapped_to_raw_8_4: QuadWord, DoubleWord;
    mapped_to_raw_8_8: QuadWord, QuadWord;
}

cases! {
    Mapped => Array;
    mapped_to_array_1_1: Byte, Byte;
    mapped_to_array_1_2: Byte, Word;
    mapped_to_array_1_4: Byte, DoubleWord;
    mapped_to_array_1_8: Byte, QuadWord;
    mapped_to_array_2_1: Word, Byte;
    mapped_to_array_2_2: Word, Word;
    mapped_to_array_2_4: Word, DoubleWord;
    mapped_to_array_2_8: Word, QuadWord;
    mapped_to_array_4_1: DoubleWord, Byte;
    mapped_to_array_4_2: DoubleWord, Word;
    mapped_to_array_4_4: DoubleWord, DoubleWord;
    mapped_to_array_4_8: DoubleWord, QuadWord;
    mapped_to_array_8_1: QuadWord, Byte;
    mapped_to_array_8_2: QuadWord, Word;
    mapped_to_array_8_4: QuadWord, DoubleWord;
    mapped_to_array_8_8: QuadWord, QuadWord;
}

cases! {
    Mapped => Mapped;
    mapped_to_mapped_1_1: Byte, Byte;
    mapped_to_mapped_1_2: Byte, Word;
    mapped_to_mapped_1_4: Byte, DoubleWord;
    mapped_to_mapped_1_8: Byte, QuadWord;
    mapped_to_mapped_2_1: Word, Byte;
    mapped_to_mapped_2_2: Word, Word;
    mapped_to_mapped_2_4: Word, DoubleWord;
    mapped_to_mapped_2_8: Word, QuadWord;
    mapped_to_mapped_4_1: DoubleWord, Byte;
    mapped_to_mapped_4_2: DoubleWord, Word;
    mapped_to_mapped_4_4: DoubleWord, DoubleWord;
    mapped_to_mapped_4_8: DoubleWord, QuadWord;
    mapped_to_mapped_8_1: QuadWord, Byte;
    mapped_to_mapped_8_2: QuadWord, Word;
    mapped_to_mapped_8_4: QuadWord, DoubleWord;
    mapped_to_mapped_8_8: QuadWord, QuadWord;
}
