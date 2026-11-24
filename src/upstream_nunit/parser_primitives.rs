use crate::{repl_parser::range_value, repl_syntax};

#[test]
fn decimal_range() {
    assert_eq!(range_value("<100, 200>"), Ok((100, 100)));
}

#[test]
fn hexadecimal_range() {
    assert_eq!(range_value("<0x0, 0x1000>"), Ok((0, 0x1000)));
}

#[test]
fn mixed_range_with_plus() {
    assert_eq!(range_value("<100, +0x100>"), Ok((100, 0x100)));
}

#[test]
fn hexadecimal_range_with_plus() {
    assert_eq!(range_value("<0x1000, +0x1000>"), Ok((0x1000, 0x1000)));
}

#[test]
fn rejects_negative_range() {
    assert!(range_value("<0x1000, 0x100>").is_err());
}

fn complete_number(input: &str) -> Option<std::borrow::Cow<'_, str>> {
    let (value, tail) = repl_syntax::number_prefix(input)?;
    tail.trim().is_empty().then_some(value)
}

macro_rules! number_cases {
    ($($name:ident: $input:literal),+ $(,)?) => {
        $(
            #[test]
            fn $name() {
                assert_eq!(complete_number($input).unwrap(), $input.replace(' ', ""));
            }
        )+
    };
}

number_cases! {
    number_hex: "0x1234",
    number_negative_hex: "-0x123",
    number_spaced_negative_hex: "- 0x36",
    number_decimal: "22",
    number_negative_decimal: "-13",
    number_spaced_negative_decimal: "-  45",
    number_fractional: "1.0",
    number_negative_fractional: "-3.45",
}

macro_rules! invalid_number_cases {
    ($($name:ident: $input:literal),+ $(,)?) => {
        $(
            #[test]
            fn $name() {
                assert!(complete_number($input).is_none());
            }
        )+
    };
}

invalid_number_cases! {
    rejects_invalid_hex: "0xghi",
    rejects_spaced_fraction: "12 .34",
    rejects_missing_integer: ".45",
    rejects_trailing_minus: "345-",
}

fn assert_irq_range(input: &str, expected: &[u32]) {
    use crate::repl_parser::{IrqEnd, irq_end, irq_range};

    let IrqEnd::Range(start, end) = irq_end(input, false).unwrap() else {
        panic!("expected an IRQ range");
    };
    let mut actual: Vec<_> = irq_range(start, end).collect();
    let mut expected = expected.to_vec();
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(actual, expected);
}

#[test]
fn positive_irq_range() {
    assert_irq_range("3 - 5", &[3, 4, 5]);
}

#[test]
fn negative_irq_range() {
    assert_irq_range("7 - 4", &[7, 6, 5, 4]);
}

#[test]
fn irq_sources() {
    use crate::repl_parser::{IrqEnd, irq_ends, irq_range};

    let mut numbers = Vec::new();
    let mut properties = Vec::new();
    for end in irq_ends("[1,2, 4-6, IRQ]", true).unwrap() {
        match end {
            IrqEnd::Number(number) => numbers.push(number),
            IrqEnd::Range(start, end) => numbers.extend(irq_range(start, end)),
            IrqEnd::Property(name) => properties.push((numbers.len(), name)),
        }
    }
    assert_eq!(numbers, [1, 2, 4, 5, 6]);
    assert_eq!(properties, [(5, "IRQ")]);
}
