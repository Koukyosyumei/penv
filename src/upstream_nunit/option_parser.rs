use crate::option_validation::{
    Kind, OptionSpec, OptionsParser, ParseError, ParsedOption, Value, Values,
};

fn spec(long: &str, short: Option<char>, kind: Kind) -> OptionSpec<'_> {
    OptionSpec {
        long,
        short,
        kind,
        required: false,
        default: None,
    }
}

fn parse(spec: OptionSpec<'_>, args: &[&str]) -> (OptionsParser, Values) {
    let mut parser = OptionsParser::default();
    let mut values = Values::new();
    assert!(
        parser
            .parse(&[spec], args, &mut values, |_| Ok(()))
            .unwrap()
    );
    (parser, values)
}

fn check_option(spec: OptionSpec<'_>, args: &[&str], value: Value) {
    let expected = ParsedOption {
        long: spec.long.into(),
        short: spec.short,
        value,
    };
    let (parser, _) = parse(spec, args);
    assert_eq!(parser.parsed_options(), &[expected]);
}

fn check_missing(spec: OptionSpec<'_>, arg: &str) {
    let mut parser = OptionsParser::default();
    parser.throw_validation_exception = true;
    let error = parser
        .parse(&[spec], &[arg], &mut Values::new(), |_| Ok(()))
        .unwrap_err();
    let ParseError::Validation(message) = error else {
        panic!("expected validation exception")
    };
    assert!(message.contains("requires parameter of type"));
}

#[test]
fn should_throw_an_exception_for_short_option_requiring_value_when_there_is_none() {
    check_missing(spec("n", Some('n'), Kind::Integer), "-n");
}

#[test]
fn should_throw_an_exception_for_long_option_requiring_value_when_there_is_none() {
    check_missing(spec("long", None, Kind::Integer), "--long");
}

#[test]
fn should_parse_short_switch_with_value() {
    check_option(
        spec("n", Some('n'), Kind::Integer),
        &["-n123"],
        Value::Integer(123),
    );
}

#[test]
fn should_parse_long_switch_with_value() {
    check_option(
        spec("number", None, Kind::Integer),
        &["--number", "123"],
        Value::Integer(123),
    );
}

#[test]
fn should_parse_nullable_int_option_with_value() {
    check_option(
        spec("number", None, Kind::NullableInteger),
        &["--number", "-1"],
        Value::Integer(-1),
    );
}

#[test]
fn should_keep_nullable_int_option_null_when_missing() {
    let (parser, values) = parse(spec("value", None, Kind::NullableInteger), &[]);
    assert_eq!(parser.parsed_options().len(), 0);
    assert_eq!(values.get("value"), None);
}

#[test]
fn should_set_nullable_int_option_in_class() {
    let (parser, values) = parse(
        spec("value", None, Kind::NullableInteger),
        &["--value", "-1"],
    );
    assert_eq!(parser.parsed_options().len(), 1);
    assert_eq!(values["value"], Value::Integer(-1));
}

#[test]
fn should_parse_long_switch_with_string_with_assignment_operator() {
    check_option(
        spec("long", None, Kind::Text),
        &["--long=test"],
        Value::Text("test".into()),
    );
}

#[test]
fn should_parse_option_with_spaces_in_value() {
    check_option(
        spec("option", None, Kind::Text),
        &["--option=Value with spaces"],
        Value::Text("Value with spaces".into()),
    );
}

#[test]
fn should_parse_short_option_with_separated_value() {
    check_option(
        spec("n", Some('n'), Kind::Integer),
        &["-n", "123"],
        Value::Integer(123),
    );
}

#[test]
fn should_throw_an_exception_on_unexpected_multiple_occurences_of_argument() {
    let mut parser = OptionsParser::default();
    parser.throw_validation_exception = true;
    let result = parser.parse(
        &[spec("value", None, Kind::Integer)],
        &["--value", "5", "--value", "6"],
        &mut Values::new(),
        |_| Ok(()),
    );
    assert!(matches!(result, Err(ParseError::Validation(_))));
}
