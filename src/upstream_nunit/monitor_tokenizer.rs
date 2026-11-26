use crate::monitor_tokenizer::{tokenize, Kind, Value};

fn kinds(input: &str, unmatched: usize, expected: &[Kind]) -> Vec<Value> {
    let result = tokenize(input);
    assert_eq!(result.error, None);
    assert_eq!(result.unmatched_characters, unmatched);
    assert_eq!(
        result
            .tokens
            .iter()
            .map(|token| token.kind)
            .collect::<Vec<_>>(),
        expected
    );
    result.tokens.into_iter().map(|token| token.value).collect()
}

fn text(value: &str) -> Value {
    Value::Text(value.into())
}

#[test]
fn comment_test() {
    use Kind::*;
    assert_eq!(kinds("#something", 0, &[Comment]), [text("something")]);
    assert_eq!(
        kinds(
            "emu[\"SomeIndexWith#Hash\"]",
            0,
            &[Literal, LeftBrace, String, RightBrace]
        ),
        [
            text("emu"),
            text("["),
            text("SomeIndexWith#Hash"),
            text("]")
        ]
    );
    assert_eq!(
        kinds(
            "emu[\"SomeIndexWithoutHash\"#Comment]",
            0,
            &[Literal, LeftBrace, String, Comment]
        ),
        [
            text("emu"),
            text("["),
            text("SomeIndexWithoutHash"),
            text("Comment]")
        ]
    );
}

#[test]
fn execution_test() {
    use Kind::*;
    assert_eq!(kinds("`something`", 0, &[Execution]), [text("something")]);
}

#[test]
fn variable_test() {
    use Kind::*;
    assert_eq!(
        kinds(
            "$name $_name $NaMe $.name $123name123",
            0,
            &[Variable, Variable, Variable, Variable, Variable]
        ),
        [
            text("name"),
            text("_name"),
            text("NaMe"),
            text(".name"),
            text("123name123")
        ]
    );
    assert_eq!(
        kinds("$variable=\"value\"", 0, &[Variable, Equality, String]),
        [text("variable"), text("="), text("value")]
    );
    assert_eq!(
        kinds(
            "$variable?=\"value\"",
            0,
            &[Variable, ConditionalEquality, String]
        ),
        [text("variable"), text("?="), text("value")]
    );
}

#[test]
fn index_test() {
    use Kind::*;
    assert_eq!(
        kinds(
            "emu[\"SomeIndex\"]",
            0,
            &[Literal, LeftBrace, String, RightBrace]
        ),
        [text("emu"), text("["), text("SomeIndex"), text("]")]
    );
    assert_eq!(
        kinds(
            "emu[15]",
            0,
            &[Literal, LeftBrace, DecimalInteger, RightBrace]
        ),
        [text("emu"), text("["), Value::Integer(15), text("]")]
    );
}

#[test]
fn string_test() {
    use Kind::*;
    assert_eq!(
        kinds("'string1' \"string2\"", 0, &[String, String]),
        [text("string1"), text("string2")]
    );
    assert_eq!(
        kinds("'string1\" 'string2\"", 1, &[String, Literal]),
        [text("string1\" "), text("string2")]
    );
}

#[test]
fn unbalanced_string_test() {
    use Kind::*;
    assert_eq!(
        kinds(
            "\"test\\\"       \\\"         \\\"       test \" 'test\\'        \\'  \\'   test '",
            0,
            &[String, String]
        ),
        [
            text("test\"       \"         \"       test "),
            text("test'        '  '   test ")
        ]
    );
}

#[test]
fn range_test() {
    use Kind::*;
    assert_eq!(
        kinds(
            "<    \t0x123abDE  \t, \t\t  0xabcdef0 \t\t   >",
            0,
            &[AbsoluteRange]
        ),
        [Value::Range(0x123abde, 0xabcdef0 - 0x123abde + 1)]
    );
    kinds("<0xdefg, 0xefgh>", 16, &[]);
}

#[test]
fn relative_range_test() {
    use Kind::*;
    assert_eq!(
        kinds("<0x6 0x2>", 0, &[RelativeRange]),
        [Value::Range(0x6, 0x2)]
    );
}

#[test]
fn simple_path_test() {
    use Kind::*;
    assert_eq!(
        kinds("@Some\\path\\to\\File", 0, &[Path]),
        [text("Some\\path\\to\\File")]
    );
    assert_eq!(
        kinds("@Some\\path\\to\\Directory\\", 0, &[Path]),
        [text("Some\\path\\to\\Directory\\")]
    );
}

#[test]
fn escaped_path_test() {
    use Kind::*;
    assert_eq!(
        kinds("@Some\\path\\to\\file\\ with\\ Spaces", 0, &[Path]),
        [text("Some\\path\\to\\file with Spaces")]
    );
    assert_eq!(
        kinds("@Some\\path\\to\\directory\\ with\\ Spaces\\", 0, &[Path]),
        [text("Some\\path\\to\\directory with Spaces\\")]
    );
}

#[test]
fn multiline_test() {
    use Kind::*;
    assert_eq!(
        kinds("\"\"\"", 0, &[MultilineStringTerminator]),
        [text("\"")]
    );
    assert_eq!(kinds("\"\"\"SomeMultiline\r\nString with many #tokens [\"inside\"] with\r\nnumbers 123 0x23 and\r\n stuff\"\"\"", 0, &[MultilineString]), [text("SomeMultiline\r\nString with many #tokens [\"inside\"] with\r\nnumbers 123 0x23 and\r\n stuff")]);
}

#[test]
fn boolean_test() {
    use Kind::*;
    assert_eq!(kinds("true", 0, &[Boolean]), [Value::Boolean(true)]);
    assert_eq!(kinds("True", 0, &[Boolean]), [Value::Boolean(true)]);
    assert_eq!(kinds("false", 0, &[Boolean]), [Value::Boolean(false)]);
    assert_eq!(kinds("False", 0, &[Boolean]), [Value::Boolean(false)]);
    assert_eq!(kinds("TRUE", 0, &[Literal]), [text("TRUE")]);
    assert_eq!(kinds("FaLse", 0, &[Literal]), [text("FaLse")]);
}

#[test]
fn null_test() {
    use Kind::*;
    assert_eq!(kinds("null", 0, &[Null]), [Value::Null]);
}

#[test]
fn integer_test() {
    use Kind::*;
    assert_eq!(
        kinds(
            "123465 -213245 +132432 1e5 -3e2 1e+3",
            0,
            &[
                DecimalInteger,
                DecimalInteger,
                DecimalInteger,
                DecimalInteger,
                DecimalInteger,
                DecimalInteger
            ]
        ),
        [
            Value::Integer(123465),
            Value::Integer(-213245),
            Value::Integer(132432),
            Value::Integer(100000),
            Value::Integer(-300),
            Value::Integer(1000)
        ]
    );
}

#[test]
fn float_test() {
    use Kind::*;
    assert_eq!(
        kinds(
            "145.5 -0.43 +45. 1.5e2 1.5e-2 -4.2e+3",
            0,
            &[Float, Float, Float, Float, Float, Float]
        ),
        [
            Value::Float(145.5),
            Value::Float(-0.43),
            Value::Float(45.0),
            Value::Float(150.0),
            Value::Float(0.015),
            Value::Float(-4200.0)
        ]
    );
}

#[test]
fn hexadecimal_test() {
    use Kind::*;
    assert_eq!(
        kinds(
            "0xabcdef 0x123469 0xABCDEF 0x123AbC",
            0,
            &[Hex, Hex, Hex, Hex]
        ),
        [
            Value::Integer(0xabcdef),
            Value::Integer(0x123469),
            Value::Integer(0xabcdef),
            Value::Integer(0x123abc)
        ]
    );
    assert_eq!(
        kinds(
            "0xgfd 123bcd",
            0,
            &[DecimalInteger, Literal, DecimalInteger, Literal]
        ),
        [
            Value::Integer(0),
            text("xgfd"),
            Value::Integer(123),
            text("bcd")
        ]
    );
}

#[test]
fn literal_test() {
    use Kind::*;
    assert_eq!(
        kinds(".Some.Literal-With?Extra:SignsIn.It:", 0, &[Literal]),
        [text(".Some.Literal-With?Extra:SignsIn.It:")]
    );
}

#[test]
fn time_interval_test() {
    use Kind::*;
    let result = tokenize("1 1. 1.2 1:2 1:2.3 1:2:3 0:1:2.3 1:2:3.4");
    assert_eq!(result.error, None);
    assert_eq!(result.unmatched_characters, 0);
    assert_eq!(
        result
            .tokens
            .iter()
            .map(|token| token.kind)
            .collect::<Vec<_>>(),
        [
            DecimalInteger,
            Float,
            Float,
            TimeInterval,
            TimeInterval,
            TimeInterval,
            TimeInterval,
            TimeInterval,
        ]
    );
    let expected = [
        1.0,
        1.0,
        1.2,
        60.0 + 2.0,
        60.0 + 2.3,
        (60.0 + 2.0) * 60.0 + 3.0,
        60.0 + 2.3,
        (60.0 + 2.0) * 60.0 + 3.4,
    ];
    let intervals: Vec<_> = result
        .tokens
        .iter()
        .map(|token| token.time_ticks().unwrap())
        .collect();
    assert_eq!(intervals.len(), expected.len());
    for (actual, expected) in intervals.into_iter().zip(expected) {
        assert_eq!(actual, (expected * 1_000_000_000.0) as u64);
    }
}
