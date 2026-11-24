use crate::packet_layout::{
    ByteOrder::{Big, Little},
    DecodeError,
    Decoded::{self, Nested, Scalar},
    Field, FieldKind, Layout,
};

const fn scalar(offset: Option<usize>, width: usize, capacity: usize) -> Field {
    Field {
        offset,
        padding: 0,
        alignment: 0,
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
        padding: 0,
        alignment: 0,
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
    padding: 0,
    alignment: 0,
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

const MISALIGNED_ARRAY: Layout = little(&[Field {
    offset: Some(1),
    padding: 0,
    alignment: 0,
    kind: FieldKind::Bytes(4),
}]);

#[test]
fn test_decode_field_offsets() {
    let data = 0xdeadbeefdeadbeefu64.to_le_bytes();
    let data1 = [
        0xff, 0x00, 0x11, 0x22, 0x33, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xdd, 0xcc, 0xbb, 0xaa,
    ];
    let data2: Vec<u8> = (0..=17).collect();
    assert_scalars(
        DEFAULT_OFFSETS.decode(&data2).unwrap(),
        &[0, 0x0201, 0x06050403, 0x0e0d0c0b0a090807],
    );
    assert_eq!(MISALIGNED_ARRAY.decode(&data2), Err(DecodeError::Unaligned));
    assert_eq!(A.decode(&[]), Err(DecodeError::InsufficientData));
    let length = C.byte_len().unwrap();
    assert_eq!(
        C.decode_at(&data, (data.len() + 1 - length) as isize),
        Err(DecodeError::InsufficientData)
    );
    assert_eq!(C.decode_at(&data, -1), Err(DecodeError::InvalidOffset));
    assert_eq!(ARRAY.decode_at(&data, -1), Err(DecodeError::InvalidOffset));

    let first = C.decode_at(&data1, 1).unwrap();
    assert_eq!(first[0], Scalar(0));
    assert_eq!(first[3], Scalar(0x33));
    let last = C
        .decode_at(&data1, (data1.len() - length) as isize)
        .unwrap();
    assert_eq!(last[0], Scalar(0xdd));
    assert_eq!(last[3], Scalar(0xaa));
}

#[test]
fn test_encode() {
    use Decoded::{Bytes, NullBytes};
    assert_eq!(A.encode(&[Scalar(0x012345)]).unwrap(), [0x50, 0x34, 0x02]);
    let bits = [1, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 1, 1, 0, 1].map(Scalar);
    assert_eq!(B.encode(&bits).unwrap(), [0xef, 0xbe]);

    for input in [vec![100, 201, 102, 203, 104], vec![100, 201, 102]] {
        assert_eq!(
            ARRAY.encode(&[Bytes(input)]),
            Err(DecodeError::InvalidValue)
        );
    }
    let input = [100, 201, 102, 203];
    assert_eq!(ARRAY.encode(&[Bytes(input.to_vec())]).unwrap(), input);
    assert_eq!(
        MISALIGNED_ARRAY.encode(&[Bytes(vec![0, 1, 2, 3])]),
        Err(DecodeError::Unaligned)
    );
    let input = [5, 11, 44, 255];
    assert_eq!(ARRAY.encode(&[Bytes(input.to_vec())]).unwrap(), input);

    const EMPTY_NESTED: Layout = little(&[nested(&NO_PACKET)]);
    const ENUMS: Layout = little(&[next(8), next(8), next(32), scalar(None, 8, 32)]);
    const NULLABLE_BYTES: Layout = Layout {
        order: Big,
        fields: &[Field {
            kind: FieldKind::Bytes(2),
            ..ARRAY.fields[0]
        }],
    };
    assert!(NO_PACKET.encode(&[]).unwrap().is_empty());
    assert!(EMPTY_NESTED.encode(&[Nested(vec![])]).unwrap().is_empty());
    assert_eq!(
        ENUMS
            .encode(&[Scalar(1), Scalar(2), Scalar(3), Scalar(1)])
            .unwrap(),
        [1, 2, 3, 0, 0, 0, 1]
    );
    assert_eq!(NULLABLE_BYTES.encode(&[NullBytes]).unwrap(), [0, 0]);
    let non_packet_fields = [0xee, 0xff];
    assert!(NO_PACKET
        .encode(
            &non_packet_fields[..NO_PACKET.fields.len()]
                .iter()
                .copied()
                .map(Scalar)
                .collect::<Vec<_>>()
        )
        .unwrap()
        .is_empty());
    assert!(ZERO.encode(&[Scalar(0xee)]).unwrap().is_empty());
    assert_eq!(
        OFFSET_ZERO.encode(&[Scalar(0xee)]),
        Err(DecodeError::InvalidOffset)
    );
    assert_eq!(
        SHORT
            .encode(&[Scalar(1), Scalar(0), Scalar(1), Scalar(0)])
            .unwrap(),
        [5]
    );
}

#[test]
fn test_nested_encode() {
    let data = [
        Scalar(0x1122334455667788),
        Nested(vec![Nested(vec![Scalar(0xc0fe)]), Scalar(0xdeadc0de)]),
        Nested(vec![Nested(vec![Scalar(0xabcd)]), Scalar(0xba5eba11)]),
    ];
    assert_eq!(
        NESTED.encode(&data).unwrap(),
        [
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0xc0, 0xfe, 0xde, 0xad, 0xc0, 0xde,
            0xab, 0xcd, 0xba, 0x5e, 0xba, 0x11,
        ]
    );
}

const fn elements(
    count: usize,
    element_bits: Option<usize>,
    explicit_bits: Option<usize>,
) -> Field {
    Field {
        kind: FieldKind::Elements {
            count,
            element_bits,
            explicit_bits,
        },
        ..ARRAY.fields[0]
    }
}

#[test]
fn test_width_in_elements() {
    const WIDTH_ELEMENTS: Layout = little(&[elements(2, Some(16), None)]);
    const WIDTH_BOTH: Layout = little(&[elements(2, Some(16), Some(32))]);
    let data = [1, 2, 3, 4];
    for layout in [&WIDTH_ELEMENTS, &WIDTH_BOTH] {
        let decoded = layout.decode(&data).unwrap();
        assert_eq!(decoded, [Decoded::Elements(vec![0x0201, 0x0403])]);
        assert_eq!(layout.encode(&decoded).unwrap(), data);
        assert_eq!(layout.byte_len().unwrap(), 4);
    }
}

#[test]
fn test_width_in_elements_errors() {
    const NON_ARRAY: Layout = little(&[elements(2, None, None)]);
    const CONFLICTING: Layout = little(&[elements(4, Some(8), Some(40))]);
    assert_eq!(NON_ARRAY.byte_len(), Err(DecodeError::InvalidWidth));
    assert_eq!(CONFLICTING.byte_len(), Err(DecodeError::InvalidWidth));
}

#[test]
fn test_padding_before() {
    const PADDED: Layout = little(&[
        next(8),
        Field {
            padding: 16,
            ..next(8)
        },
    ]);
    let data = [0xaa, 0x12, 0x34, 0xbb];
    let decoded = PADDED.decode(&data).unwrap();
    assert_eq!(decoded, [Scalar(0xaa), Scalar(0xbb)]);
    assert_eq!(PADDED.byte_len().unwrap(), 4);
    assert_eq!(PADDED.encode(&decoded).unwrap(), [0xaa, 0, 0, 0xbb]);
}

#[test]
fn test_alignment() {
    const ALIGNED: Layout = little(&[
        next(8),
        Field {
            alignment: 32,
            ..next(32)
        },
    ]);
    let data = [0x0a, 0xff, 0xff, 0xff, 0x44, 0x33, 0x22, 0x11];
    let decoded = ALIGNED.decode(&data).unwrap();
    assert_eq!(decoded, [Scalar(0x0a), Scalar(0x11223344)]);
    assert_eq!(ALIGNED.byte_len().unwrap(), 8);
    let encoded = ALIGNED.encode(&decoded).unwrap();
    assert_eq!(encoded[0], 0x0a);
    assert_eq!(encoded[1..4], [0, 0, 0]);
    assert_eq!(
        u32::from_le_bytes(encoded[4..8].try_into().unwrap()),
        0x11223344
    );
}
