use crate::hex_string::decode;

#[test]
fn bytes_from_hex_string() {
    for (input, reverse, ignore_whitespace, expected) in [
        ("0a", false, false, &[0x0a][..]),
        (
            "abcdefABCDEF0123456789",
            false,
            false,
            &[
                0xab, 0xcd, 0xef, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89,
            ],
        ),
        ("0aB1", false, false, &[0x0a, 0xb1]),
        ("0aB1", true, false, &[0xb1, 0x0a]),
        ("00aB15", false, false, &[0x00, 0xab, 0x15]),
        ("00aB15", true, false, &[0x15, 0xab, 0x00]),
        ("a\tb", false, true, &[0xab]),
        ("a\t \nb", false, true, &[0xab]),
        ("ab cd", false, true, &[0xab, 0xcd]),
        ("ab cd", true, true, &[0xcd, 0xab]),
        ("abc def", false, true, &[0xab, 0xcd, 0xef]),
        ("abc def", true, true, &[0xef, 0xcd, 0xab]),
    ] {
        assert_eq!(decode(input, reverse, ignore_whitespace).unwrap(), expected);
    }
    for invalid in ["ab3", "ab cd", "abc def", "x", "xx"] {
        assert!(decode(invalid, false, false).is_err(), "{invalid:?}");
    }
}
