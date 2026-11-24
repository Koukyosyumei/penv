use crate::migrant::{
    object_identifier::ObjectIdentifier,
    primitive::{Reader, Writer},
};
use std::{
    collections::HashSet,
    io::{Cursor, ErrorKind, Seek, SeekFrom},
    rc::Rc,
};

#[test]
fn should_generate_unique_id() {
    const OBJECTS: usize = 100_000;
    let mut identifier = ObjectIdentifier::default();
    let objects: Vec<_> = (0..OBJECTS).map(|_| Rc::new(())).collect();
    let mut generated = HashSet::new();
    for object in &objects {
        let (id, _) = identifier.id(object);
        assert!(generated.insert(id), "Not unique ID was generated.");
    }
}

struct Random(u64);

impl Random {
    fn bits(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn next(&mut self, max: i32) -> i32 {
        (self.bits() % max as u64) as i32
    }

    fn long(&mut self) -> i64 {
        (i64::from(self.next(i32::MAX - 1)) << 32) + i64::from(self.next(i32::MAX))
    }
}

fn roundtrip<T: std::fmt::Debug + PartialEq + Copy>(
    buffered: bool,
    values: &[T],
    mut write: impl FnMut(&mut Writer<&mut Cursor<Vec<u8>>>, T) -> std::io::Result<()>,
    mut read: impl FnMut(&mut Reader<&mut Cursor<Vec<u8>>>) -> std::io::Result<T>,
) {
    let mut stream = Cursor::new(Vec::new());
    let mut writer = Writer::new(&mut stream, buffered);
    for &value in values {
        write(&mut writer, value).unwrap();
    }
    writer.finish().unwrap();
    let position = stream.position();
    stream.seek(SeekFrom::Start(0)).unwrap();
    let mut reader = Reader::new(&mut stream, buffered);
    for &value in values {
        assert_eq!(read(&mut reader).unwrap(), value);
    }
    reader.finish().unwrap();
    assert_eq!(
        stream.position(),
        position,
        "Stream position differs after reading."
    );
}

fn ints(buffered: bool, count: usize) {
    let mut random = Random(0xcf17_efc2_5274_ae65);
    let values: Vec<_> = (0..count)
        .map(|_| i64::from(random.next(i32::MAX) - random.next(i32::MAX)))
        .collect();
    roundtrip(buffered, &values, |w, v| w.signed(v), |r| r.signed());
}

fn longs(buffered: bool, count: usize) {
    let mut random = Random(0xd218_1939_4715_efa5);
    let values: Vec<_> = (0..count).map(|_| random.long()).collect();
    roundtrip(buffered, &values, |w, v| w.signed(v), |r| r.signed());
}

fn ulongs(buffered: bool, count: usize) {
    let mut random = Random(0xe6ed_9d38_6a44_79a5);
    let values: Vec<_> = (0..count).map(|_| random.long() as u64).collect();
    roundtrip(buffered, &values, |w, v| w.unsigned(v), |r| r.unsigned());
}

fn doubles(buffered: bool, count: usize) {
    let mut random = Random(0xfc1f_9f98_9785_33e1);
    let values: Vec<_> = (0..count)
        .map(|_| {
            let fraction = (random.bits() >> 11) as f64 / (1_u64 << 53) as f64;
            (fraction - 0.5) * f64::MAX
        })
        .collect();
    roundtrip(
        buffered,
        &values,
        |writer, value| writer.signed(value.to_bits() as i64),
        |reader| reader.signed().map(|bits| f64::from_bits(bits as u64)),
    );
}

fn negative_int(buffered: bool) {
    let mut random = Random(0xcf17_efc2_5274_ae65);
    roundtrip(
        buffered,
        &[i64::from(-random.next(i32::MAX))],
        |w, v| w.signed(v),
        |r| r.signed(),
    );
}

fn limits(buffered: bool) {
    let mut stream = Cursor::new(Vec::new());
    let mut writer = Writer::new(&mut stream, buffered);
    let byte_values = [u8::MIN, u8::MAX, i8::MIN as u8, i8::MAX as u8];
    let integer_values = [
        (i16::MIN as u64, true),
        (i16::MAX as u64, true),
        (u16::MIN.into(), false),
        (u16::MAX.into(), false),
        (i32::MIN as u64, true),
        (i32::MAX as u64, true),
        (u32::MIN.into(), false),
        (u32::MAX.into(), false),
        (i64::MIN as u64, true),
        (i64::MAX as u64, true),
        (u64::MIN, false),
        (u64::MAX, false),
    ];
    writer.bytes(&byte_values).unwrap();
    for (value, signed) in integer_values {
        if signed {
            writer.signed(value as i64)
        } else {
            writer.unsigned(value)
        }
        .unwrap();
    }
    writer.finish().unwrap();
    let position = stream.position();
    stream.seek(SeekFrom::Start(0)).unwrap();
    let mut reader = Reader::new(&mut stream, buffered);
    for expected in byte_values {
        assert_eq!(reader.byte().unwrap(), expected);
    }
    for (expected, signed) in integer_values {
        let actual = if signed {
            reader.signed().unwrap() as u64
        } else {
            reader.unsigned().unwrap()
        };
        assert_eq!(actual, expected);
    }
    reader.finish().unwrap();
    assert_eq!(stream.position(), position);
}

fn unaligned(buffered: bool) {
    const COUNT: usize = 80_000;
    let mut stream = Cursor::new(Vec::new());
    let mut writer = Writer::new(&mut stream, buffered);
    writer.bytes(&[1]).unwrap();
    for _ in 0..COUNT {
        writer.signed(i64::from(i32::MAX)).unwrap();
    }
    writer.finish().unwrap();
    let position = stream.position();
    stream.seek(SeekFrom::Start(0)).unwrap();
    let mut reader = Reader::new(&mut stream, buffered);
    assert_eq!(reader.byte().unwrap(), 1);
    for _ in 0..COUNT {
        assert_eq!(reader.signed().unwrap(), i64::from(i32::MAX));
    }
    reader.finish().unwrap();
    assert_eq!(stream.position(), position);
}

fn copy_stream(buffered: bool) {
    let expected: Vec<_> = (0..1000).map(|value| value as u8).collect();
    let mut stream = Cursor::new(Vec::new());
    let mut writer = Writer::new(&mut stream, buffered);
    writer
        .copy_from(&mut Cursor::new(&expected), expected.len())
        .unwrap();
    writer.finish().unwrap();
    stream.seek(SeekFrom::Start(0)).unwrap();
    let mut reader = Reader::new(&mut stream, buffered);
    let mut actual = Vec::with_capacity(expected.len());
    reader.copy_to(&mut actual, expected.len()).unwrap();
    reader.finish().unwrap();
    assert_eq!(actual, expected);
}

fn premature_eof(buffered: bool) {
    let mut reader = Reader::new(Cursor::new(Vec::new()), buffered);
    let error = reader.copy_to(&mut Vec::new(), 100).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::UnexpectedEof);
    reader.finish().unwrap();
}

macro_rules! cases {
    ($($name:ident: $method:ident($($argument:expr),+);)+) => {$(
        #[test]
        fn $name() {
            $method($($argument),+);
        }
    )+};
}

cases! {
    doubles_direct_1: doubles(false, 1);
    doubles_direct_10: doubles(false, 10);
    doubles_direct_100: doubles(false, 100);
    doubles_direct_10000: doubles(false, 10000);
    doubles_direct_1000000: doubles(false, 1000000);
    ints_direct_1: ints(false, 1);
    ints_direct_10: ints(false, 10);
    ints_direct_100: ints(false, 100);
    ints_direct_10000: ints(false, 10000);
    ints_direct_1000000: ints(false, 1000000);
    longs_direct_1: longs(false, 1);
    longs_direct_10: longs(false, 10);
    longs_direct_100: longs(false, 100);
    longs_direct_10000: longs(false, 10000);
    longs_direct_1000000: longs(false, 1000000);
    ulongs_direct_1: ulongs(false, 1);
    ulongs_direct_10: ulongs(false, 10);
    ulongs_direct_100: ulongs(false, 100);
    ulongs_direct_10000: ulongs(false, 10000);
    ulongs_direct_1000000: ulongs(false, 1000000);
    copy_stream_direct: copy_stream(false);
    unaligned_direct: unaligned(false);
    limits_direct: limits(false);
    premature_eof_direct: premature_eof(false);
    negative_int_direct: negative_int(false);
    doubles_buffered_1: doubles(true, 1);
    doubles_buffered_10: doubles(true, 10);
    doubles_buffered_100: doubles(true, 100);
    doubles_buffered_10000: doubles(true, 10000);
    doubles_buffered_1000000: doubles(true, 1000000);
    ints_buffered_1: ints(true, 1);
    ints_buffered_10: ints(true, 10);
    ints_buffered_100: ints(true, 100);
    ints_buffered_10000: ints(true, 10000);
    ints_buffered_1000000: ints(true, 1000000);
    longs_buffered_1: longs(true, 1);
    longs_buffered_10: longs(true, 10);
    longs_buffered_100: longs(true, 100);
    longs_buffered_10000: longs(true, 10000);
    longs_buffered_1000000: longs(true, 1000000);
    ulongs_buffered_1: ulongs(true, 1);
    ulongs_buffered_10: ulongs(true, 10);
    ulongs_buffered_100: ulongs(true, 100);
    ulongs_buffered_10000: ulongs(true, 10000);
    ulongs_buffered_1000000: ulongs(true, 1000000);
    copy_stream_buffered: copy_stream(true);
    unaligned_buffered: unaligned(true);
    limits_buffered: limits(true);
    premature_eof_buffered: premature_eof(true);
    negative_int_buffered: negative_int(true);
}

fn strings(buffered: bool, count: usize, long_strings: bool) {
    let mut random = Random(0x6fde_a2d4_bfe6_7621);
    let values: Vec<String> = (0..count)
        .map(|_| {
            let length = if long_strings && random.next(i32::MAX) % 10 == 0 {
                8000
            } else {
                random.next(100)
            };
            (0..length)
                .map(|_| {
                    loop {
                        let character = char::from(random.next(256) as u8);
                        if !character.is_control() {
                            break character;
                        }
                    }
                })
                .collect()
        })
        .collect();
    let mut stream = Cursor::new(Vec::new());
    let mut writer = Writer::new(&mut stream, buffered);
    for value in &values {
        writer.string(value).unwrap();
    }
    writer.finish().unwrap();
    let end = stream.position();
    stream.seek(SeekFrom::Start(0)).unwrap();
    let mut reader = Reader::new(&mut stream, buffered);
    for value in values {
        assert_eq!(reader.string().unwrap(), value);
    }
    reader.finish().unwrap();
    assert_eq!(stream.position(), end);
}

fn byte_arrays(buffered: bool, count: usize) {
    let mut random = Random(0x1eed_436b_c709_82fd);
    let array: Vec<_> = (0..count).map(|_| random.bits() as u8).collect();
    check_chunks(buffered, &[&array]);
}

fn partial_arrays(buffered: bool) {
    let arrays: Vec<Vec<u8>> = (0..100).map(|_| (0..=255).collect()).collect();
    let chunks: Vec<_> = arrays
        .iter()
        .enumerate()
        .map(|(index, array)| &array[index..])
        .collect();
    check_chunks(buffered, &chunks);
}

fn check_chunks(buffered: bool, chunks: &[&[u8]]) {
    let mut stream = Cursor::new(Vec::new());
    let mut writer = Writer::new(&mut stream, buffered);
    for chunk in chunks {
        writer.bytes(chunk).unwrap();
    }
    writer.finish().unwrap();
    let end = stream.position();
    stream.seek(SeekFrom::Start(0)).unwrap();
    let mut reader = Reader::new(&mut stream, buffered);
    for expected in chunks {
        let mut actual = vec![0; expected.len()];
        reader.bytes(&mut actual).unwrap();
        assert_eq!(actual, *expected);
    }
    reader.finish().unwrap();
    assert_eq!(stream.position(), end);
}

cases! {
    byte_arrays_direct_10: byte_arrays(false, 10);
    byte_arrays_direct_1000: byte_arrays(false, 1000);
    byte_arrays_direct_100000: byte_arrays(false, 100000);
    strings_direct_1_true: strings(false, 1, true);
    strings_direct_1_false: strings(false, 1, false);
    strings_direct_100_true: strings(false, 100, true);
    strings_direct_100_false: strings(false, 100, false);
    strings_direct_10000_true: strings(false, 10000, true);
    strings_direct_10000_false: strings(false, 10000, false);
    partial_arrays_direct: partial_arrays(false);
    byte_arrays_buffered_10: byte_arrays(true, 10);
    byte_arrays_buffered_1000: byte_arrays(true, 1000);
    byte_arrays_buffered_100000: byte_arrays(true, 100000);
    strings_buffered_1_true: strings(true, 1, true);
    strings_buffered_1_false: strings(true, 1, false);
    strings_buffered_100_true: strings(true, 100, true);
    strings_buffered_100_false: strings(true, 100, false);
    strings_buffered_10000_true: strings(true, 10000, true);
    strings_buffered_10000_false: strings(true, 10000, false);
    partial_arrays_buffered: partial_arrays(true);
}

fn decimals(buffered: bool, count: usize) {
    use crate::migrant::primitive::Decimal;

    let mut random = Random(0xe019_f2d8_76eb_4835);
    let values: Vec<_> = (0..count)
        .map(|_| Decimal {
            scale: random.next(29) as u8,
            negative: random.next(2) == 1,
            magnitude: std::array::from_fn(|_| {
                ((random.bits() as u32 & 0xffff) << 16) | (random.bits() as u32 & 0xffff)
            }),
        })
        .collect();
    roundtrip(buffered, &values, |w, v| w.decimal(v), |r| r.decimal());
}

fn guids(buffered: bool, count: usize) {
    use crate::migrant::primitive::Guid;

    let mut random = Random(0x8b6d_9409_a310_e747);
    let values: Vec<_> = (0..count)
        .map(|_| {
            let mut bytes = std::array::from_fn(|_| random.bits() as u8);
            // Guid.NewGuid produces an RFC 4122 version 4 value. The two
            // leading Guid fields that are integers use .NET byte-array order.
            bytes[7] = (bytes[7] & 0x0f) | 0x40;
            bytes[8] = (bytes[8] & 0x3f) | 0x80;
            Guid(bytes)
        })
        .collect();
    roundtrip(buffered, &values, |w, v| w.guid(v), |r| r.guid());
}

fn calendar_ticks(year: i32, month: i32, day: i32, hour: i32, minute: i32, second: i32) -> i64 {
    let previous = year - 1;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let days = previous * 365 + previous / 4 - previous / 100
        + previous / 400
        + month_days[..(month - 1) as usize].iter().sum::<i32>()
        + day
        - 1;
    (i64::from(days) * 86_400 + i64::from(hour * 3600 + minute * 60 + second)) * 10_000_000
}

fn date_times(buffered: bool, count: usize) {
    let mut random = Random(0x035d_5ea7_b85b_ef41);
    const MAX_DATE: i64 = 3_155_378_975_999_999_999;
    let half = count / 2;
    let mut values: Vec<_> = (0..half)
        .map(|_| {
            (i64::from(random.next(((MAX_DATE >> 32) - 1) as i32)) << 32)
                + i64::from(random.next(i32::MAX))
        })
        .collect();
    let mut component = |every, maximum| i32::from(random.next(every) != 0) * random.next(maximum);
    for _ in half..count {
        values.push(calendar_ticks(
            2000 + component(16, 100),
            1 + component(16, 12),
            1 + component(16, 28),
            component(8, 24),
            component(4, 60),
            component(2, 60),
        ));
    }
    roundtrip(buffered, &values, |w, v| w.date_time(v), |r| r.date_time());
}

// The upstream TimeSpan-named method also writes and reads DateTime.
cases! {
    datetime_direct_1: date_times(false, 1);
    datetime_direct_10: date_times(false, 10);
    datetime_direct_100: date_times(false, 100);
    datetime_direct_10000: date_times(false, 10000);
    timespan_direct_1: date_times(false, 1);
    timespan_direct_10: date_times(false, 10);
    timespan_direct_100: date_times(false, 100);
    timespan_direct_10000: date_times(false, 10000);
    decimal_direct_1: decimals(false, 1);
    decimal_direct_10: decimals(false, 10);
    decimal_direct_100: decimals(false, 100);
    decimal_direct_10000: decimals(false, 10000);
    decimal_direct_1000000: decimals(false, 1000000);
    guid_direct_10: guids(false, 10);
    guid_direct_1000: guids(false, 1000);
    guid_direct_100000: guids(false, 100000);
    datetime_buffered_1: date_times(true, 1);
    datetime_buffered_10: date_times(true, 10);
    datetime_buffered_100: date_times(true, 100);
    datetime_buffered_10000: date_times(true, 10000);
    timespan_buffered_1: date_times(true, 1);
    timespan_buffered_10: date_times(true, 10);
    timespan_buffered_100: date_times(true, 100);
    timespan_buffered_10000: date_times(true, 10000);
    decimal_buffered_1: decimals(true, 1);
    decimal_buffered_10: decimals(true, 10);
    decimal_buffered_100: decimals(true, 100);
    decimal_buffered_10000: decimals(true, 10000);
    decimal_buffered_1000000: decimals(true, 1000000);
    guid_buffered_10: guids(true, 10);
    guid_buffered_1000: guids(true, 1000);
    guid_buffered_100000: guids(true, 100000);
}
