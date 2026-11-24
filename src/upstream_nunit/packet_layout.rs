use crate::packet_layout::{
    ByteOrder::{Big, Little},
    DecodeError,
    Decoded::{self, Nested, Scalar},
    Field, FieldKind, Layout,
};

const fn scalar(offset: Option<usize>, width: usize, capacity: usize) -> Field {
    Field {
        offset,
        kind: FieldKind::Scalar { width, capacity },
    }
}

const fn next(width: usize) -> Field {
    scalar(None, width, width)
}

const fn at(offset: usize, width: usize, capacity: usize) -> Field {
    scalar(Some(offset), width, capacity)
}

const fn nested(layout: &'static Layout) -> Field {
    Field {
        offset: None,
        kind: FieldKind::Nested(layout),
    }
}

const fn little(fields: &'static [Field]) -> Layout {
    Layout {
        order: Little,
        fields,
    }
}

const A: Layout = little(&[at(4, 16, 64)]);
const B: Layout = little(&[
    at(0, 1, 1),
    at(1, 1, 1),
    at(2, 1, 1),
    at(3, 1, 1),
    at(4, 1, 1),
    at(5, 1, 1),
    at(6, 1, 1),
    at(7, 1, 1),
    at(8, 1, 1),
    at(9, 1, 1),
    at(10, 1, 1),
    at(11, 1, 1),
    at(12, 1, 8),
    at(13, 1, 16),
    at(14, 1, 32),
    at(15, 1, 64),
]);
const C: Layout = little(&[at(0, 8, 8), at(8, 8, 8), at(16, 8, 8), at(24, 8, 8)]);
const ARRAY: Layout = little(&[Field {
    offset: None,
    kind: FieldKind::Bytes(4),
}]);
const DEFAULT_OFFSETS: Layout = little(&[next(8), next(16), next(32), next(64)]);
const ENDIAN_FIELDS: &[Field] = &[
    at(0, 64, 64),
    at(0, 32, 32),
    at(0, 16, 16),
    at(0, 8, 8),
    at(0, 64, 64),
    at(0, 32, 32),
    at(0, 16, 16),
];
const LSB: Layout = little(ENDIAN_FIELDS);
const MSB: Layout = Layout {
    order: Big,
    fields: ENDIAN_FIELDS,
};
const ONE_BIT: Layout = little(&[at(63, 1, 1)]);
const INNER_B: Layout = Layout {
    order: Big,
    fields: &[next(16)],
};
const INNER_A: Layout = Layout {
    order: Big,
    fields: &[nested(&INNER_B), next(32)],
};
const NESTED: Layout = Layout {
    order: Big,
    fields: &[next(64), nested(&INNER_A), nested(&INNER_A)],
};
const NO_PACKET: Layout = little(&[]);
const ZERO: Layout = little(&[scalar(None, 0, 8)]);
const OFFSET_ZERO: Layout = little(&[at(24, 0, 8)]);
const SHORT: Layout = little(&[at(0, 1, 1), at(1, 1, 1), at(2, 1, 1), at(3, 1, 1)]);

#[test]
fn test_length_calculation() {
    for (layout, expected) in [
        (&A, 3),
        (&B, 2),
        (&C, 4),
        (&ARRAY, 4),
        (&DEFAULT_OFFSETS, 15),
        (&LSB, 8),
        (&ONE_BIT, 8),
        (&NESTED, 20),
        (&NO_PACKET, 0),
        (&ZERO, 0),
        (&OFFSET_ZERO, 3),
        (&SHORT, 1),
    ] {
        assert_eq!(layout.byte_len().unwrap(), expected);
    }
}

fn assert_scalars(actual: Vec<Decoded>, expected: &[u64]) {
    assert_eq!(
        actual,
        expected.iter().copied().map(Scalar).collect::<Vec<_>>()
    );
}

#[test]
fn test_decode() {
    let data = 0xdead_beefu32.to_le_bytes();
    assert_scalars(A.decode(&data).unwrap(), &[0xdbee]);
    assert_scalars(
        B.decode(&data).unwrap(),
        &[1, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 1, 1, 0, 1],
    );
    assert_scalars(C.decode(&data).unwrap(), &[0xef, 0xbe, 0xad, 0xde]);

    const INVALID: Layout = little(&[scalar(None, 64, 8)]);
    assert_eq!(INVALID.decode(&data), Err(DecodeError::InvalidWidth));

    let mut no_packet = [0u8; 2];
    for (field, value) in no_packet.iter_mut().zip(NO_PACKET.decode(&data).unwrap()) {
        let Scalar(value) = value else {
            panic!("expected scalar")
        };
        *field = value as u8;
    }
    assert_eq!(no_packet[0], 0);
    assert_eq!(no_packet[1], 0);
    assert_scalars(ZERO.decode(&data).unwrap(), &[0]);
    assert_scalars(OFFSET_ZERO.decode(&data).unwrap(), &[0]);
    assert_scalars(SHORT.decode(&data).unwrap(), &[1, 1, 1, 1]);
}

#[test]
fn test_nested_decode() {
    let data = [
        0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0xc0, 0xfe, 0xde, 0xad, 0xc0, 0xde, 0xab,
        0xcd, 0xba, 0x5e, 0xba, 0x11,
    ];
    let decoded = NESTED.decode(&data).unwrap();
    assert_eq!(decoded[0], Scalar(0x1122334455667788));
    let Nested(first) = &decoded[1] else {
        panic!("expected nested packet")
    };
    let Nested(inner) = &first[0] else {
        panic!("expected nested member")
    };
    assert_eq!(inner[0], Scalar(0xc0fe));
    assert_eq!(first[1], Scalar(0xdeadc0de));
}

#[test]
fn test_decode_edianness() {
    let data = 0xdeadbeefdeadbeefu64.to_le_bytes();
    assert_scalars(
        LSB.decode(&data).unwrap(),
        &[
            0xdeadbeefdeadbeef,
            0xdeadbeef,
            0xbeef,
            0xef,
            0xdeadbeefdeadbeef,
            0xdeadbeef,
            0xbeef,
        ],
    );
    assert_scalars(
        MSB.decode(&data).unwrap(),
        &[
            0xefbeaddeefbeadde,
            0xefbeadde,
            0xefbe,
            0xef,
            0xefbeaddeefbeadde,
            0xefbeadde,
            0xefbe,
        ],
    );
}
