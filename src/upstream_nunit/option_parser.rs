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

fn parse_all(schema: &[OptionSpec<'_>], args: &[&str]) -> (OptionsParser, Values) {
    let mut parser = OptionsParser::default();
    let mut values = Values::new();
    parser.parse(schema, args, &mut values, |_| Ok(())).unwrap();
    (parser, values)
}

#[test]
fn should_detect_short_switch() {
    let (parser, _) = parse(spec("c", Some('c'), Kind::Bool), &["-c"]);
    assert_eq!(parser.parsed_options().len(), 1);
    assert_eq!(parser.parsed_options()[0].short, Some('c'));
}

#[test]
fn should_set_short_switch_in_class() {
    let (parser, values) = parse(spec("switch", Some('s'), Kind::Bool), &["-s"]);
    assert_eq!(parser.parsed_options().len(), 1);
    assert_eq!(values["switch"], Value::Bool(true));
}

#[test]
fn should_detect_long_switch() {
    let (parser, _) = parse(spec("long", None, Kind::Bool), &["--long"]);
    assert_eq!(parser.parsed_options().len(), 1);
    assert_eq!(parser.parsed_options()[0].long, "long");
}

#[test]
fn should_detect_option_having_both_short_and_long_name() {
    for args in [["-l"], ["--long"]] {
        let (parser, _) = parse(spec("long", Some('l'), Kind::Bool), &args);
        assert_eq!(parser.parsed_options().len(), 1);
        assert_eq!(parser.parsed_options()[0].short, Some('l'));
        assert_eq!(parser.parsed_options()[0].long, "long");
    }
}

#[test]
fn should_detect_unexpected_short_switch() {
    let (parser, _) = parse_all(&[spec("c", Some('c'), Kind::Bool)], &["-xc"]);
    assert_eq!(parser.parsed_options().len(), 1);
    assert_eq!(parser.parsed_options()[0].short, Some('c'));
    assert_eq!(parser.unexpected_arguments(), ["x"]);
}

#[test]
fn should_detect_unexpected_long_switch() {
    let (parser, _) = parse_all(
        &[spec("long", None, Kind::Bool)],
        &["--long", "--secondLong"],
    );
    assert_eq!(parser.parsed_options().len(), 1);
    assert_eq!(parser.parsed_options()[0].long, "long");
    assert_eq!(parser.unexpected_arguments(), ["secondLong"]);
}

#[test]
fn should_detected_unexpected_short_switch_with_value() {
    let (parser, _) = parse_all(
        &[spec("c", Some('c'), Kind::Bool)],
        &["-cx", "value with whitespace"],
    );
    assert_eq!(parser.parsed_options().len(), 1);
    assert_eq!(parser.parsed_options()[0].short, Some('c'));
    assert_eq!(
        parser.unexpected_arguments(),
        ["x", "value with whitespace"]
    );
}

#[test]
fn should_parse_option_value_with_hyphen() {
    let (parser, _) = parse(
        spec("c", Some('c'), Kind::Text),
        &["-c", "value-with-hyphen"],
    );
    assert_eq!(parser.parsed_options().len(), 1);
    assert_eq!(parser.parsed_options()[0].short, Some('c'));
    assert_eq!(
        parser.parsed_options()[0].value,
        Value::Text("value-with-hyphen".into())
    );
    assert!(parser.unexpected_arguments().is_empty());
}

#[test]
fn should_parse_short_switch_with_string_value() {
    let (parser, _) = parse_all(
        &[
            spec("n", Some('n'), Kind::Bool),
            spec("m", Some('m'), Kind::Text),
        ],
        &["-nmValue"],
    );
    assert_eq!(parser.parsed_options().len(), 2);
    assert_eq!(parser.parsed_options()[0].short, Some('n'));
    assert_eq!(parser.parsed_options()[1].short, Some('m'));
    assert_eq!(
        parser.parsed_options()[1].value,
        Value::Text("Value".into())
    );
}

#[test]
fn should_recreate_unparsed_arguments() {
    let schema = [
        spec("expected", None, Kind::Bool),
        spec("another-expected", None, Kind::Bool),
        spec("A", Some('A'), Kind::Bool),
    ];
    let args = [
        "--expected",
        "-x",
        "value",
        "-y1",
        "--another-expected",
        "-Aw",
        "-z'this was unexpected'",
    ];
    let (parser, _) = parse_all(&schema, &args);
    assert_eq!(parser.parsed_options().len(), 3);
    assert_eq!(parser.parsed_options()[0].long, "expected");
    assert_eq!(parser.parsed_options()[1].long, "another-expected");
    assert_eq!(
        parser.recreate_unparsed_arguments(),
        r#"-x value -y1 -w "-z'this was unexpected'""#
    );
}

fn check_pair(arg: &str, expected_remainder: &str) {
    let (parser, _) = parse_all(
        &[
            spec("A", Some('A'), Kind::Bool),
            spec("B", Some('B'), Kind::Bool),
        ],
        &[arg],
    );
    assert_eq!(parser.parsed_options().len(), 2);
    assert_eq!(parser.parsed_options()[0].short, Some('A'));
    assert_eq!(parser.parsed_options()[1].short, Some('B'));
    assert_eq!(parser.recreate_unparsed_arguments(), expected_remainder);
}

#[test]
fn should_recreate_mixed_short_flag() {
    check_pair("-AwB", "-w");
}

#[test]
fn should_remove_empty_short_flag_prefix() {
    check_pair("-AB", "");
}

#[test]
fn should_not_recreated_parsed_value() {
    let args = ["--switch", "value"];
    let (parser, _) = parse(spec("switch", None, Kind::Text), &args);
    assert_eq!(parser.parsed_options().len(), 1);
    assert_eq!(
        parser.parsed_options()[0].value,
        Value::Text(args[1].into())
    );
    assert_eq!(parser.recreate_unparsed_arguments(), "");
}

#[test]
fn should_not_parse_after_double_hyphen() {
    let mut option = spec("value", None, Kind::Integer);
    option.default = Some(Value::Integer(-1));
    let mut parser = OptionsParser::default();
    parser.allow_unexpected_arguments = true;
    let mut values = Values::new();
    parser
        .parse(&[option], &["--", "--value", "test"], &mut values, |_| {
            Ok(())
        })
        .unwrap();
    assert_eq!(values["value"], Value::Integer(-1));
    assert_eq!(parser.parsed_options().len(), 0);
    assert_eq!(parser.unexpected_arguments(), ["--value", "test"]);
    assert_eq!(parser.recreate_unparsed_arguments(), "--value test");
}

fn check_unparsed(args: &[&str], option: OptionSpec<'_>, number: i32, unknown: &str) {
    let expected = ParsedOption {
        long: option.long.into(),
        short: option.short,
        value: Value::Integer(number),
    };
    let (parser, _) = parse_all(&[option], args);
    assert_eq!(parser.parsed_options(), &[expected]);
    assert_eq!(parser.unexpected_arguments(), [unknown]);
    assert_eq!(parser.recreate_unparsed_arguments(), format!("--{unknown}"));
}

macro_rules! unparsed {
    ($test:ident, $args:expr, $long:literal, $short:expr, $value:literal, $unknown:literal) => {
        #[test]
        fn $test() {
            check_unparsed(&$args, spec($long, $short, Kind::Integer), $value, $unknown);
        }
    };
}

unparsed!(
    should_recreate_unparsed_argument8,
    ["--wrong-arg", "-n123"],
    "n",
    Some('n'),
    123,
    "wrong-arg"
);
unparsed!(
    should_recreate_unparsed_argument9,
    ["--wrong-arg", "-n", "123"],
    "n",
    Some('n'),
    123,
    "wrong-arg"
);
unparsed!(
    should_recreate_unparsed_argument10,
    ["--no-xwt", "--port", "8888"],
    "port",
    None,
    8888,
    "no-xwt"
);
unparsed!(
    should_recreate_unparsed_argument11,
    ["--no-xwt", "--port=8888"],
    "port",
    None,
    8888,
    "no-xwt"
);
unparsed!(
    should_recreate_unparsed_argument12,
    ["-n123", "--wrong-arg"],
    "n",
    Some('n'),
    123,
    "wrong-arg"
);
unparsed!(
    should_recreate_unparsed_argument13,
    ["-n", "123", "--wrong-arg"],
    "n",
    Some('n'),
    123,
    "wrong-arg"
);
unparsed!(
    should_recreate_unparsed_argument14,
    ["--port", "8888", "--no-xwt"],
    "port",
    None,
    8888,
    "no-xwt"
);
unparsed!(
    should_recreate_unparsed_argument15,
    ["--port=8888", "--no-xwt"],
    "port",
    None,
    8888,
    "no-xwt"
);

fn parse_positionals(
    schema: &[OptionSpec<'_>],
    positionals: &[OptionSpec<'_>],
    args: &[&str],
) -> (OptionsParser, Values, bool) {
    let mut parser = OptionsParser::default();
    let mut values = Values::new();
    let success = parser
        .parse_with_positionals(schema, positionals, args, &mut values, |_| Ok(()))
        .unwrap();
    (parser, values, success)
}

#[test]
fn should_accept_expected_values() {
    let args = ["-a", "-b", "value-1", "-c", "value-2", "-d", "value-3"];
    let flags = ["a", "b", "c", "d"].map(|name| spec(name, name.chars().next(), Kind::Bool));
    let (parser, _, _) = parse_positionals(
        &flags,
        &[
            spec("val1", None, Kind::Text),
            spec("val2", None, Kind::Text),
        ],
        &args,
    );
    let ordered_flags: Vec<_> = parser
        .parsed_options()
        .iter()
        .map(|option| option.short.unwrap())
        .collect();
    assert_eq!(ordered_flags, ['a', 'b', 'c', 'd']);
    assert_eq!(
        parser.positional_values(),
        [Value::Text("value-1".into()), Value::Text("value-2".into())]
    );
    assert_eq!(parser.recreate_unparsed_arguments(), "value-3");
}

#[test]
fn should_parse_positional_values() {
    let (parser, values, _) = parse_positionals(
        &[],
        &[spec("value", None, Kind::Text)],
        &["value", "value-2"],
    );
    assert_eq!(parser.positional_values().len(), 1);
    assert_eq!(parser.unexpected_arguments(), ["value-2"]);
    assert_eq!(values["value"], Value::Text("value".into()));
}

#[test]
fn shoul_not_parse_on_missing_required_positional_values() {
    let mut positional = spec("value", None, Kind::Text);
    positional.required = true;
    let (_, _, parsed) = parse_positionals(&[], &[positional], &[]);
    assert!(!parsed);
}

const TEST_ENUM: Kind = Kind::Enum(&["X", "Y", "Z"]);

#[test]
fn should_parse_enum() {
    let (_, values) = parse(spec("value", Some('v'), TEST_ENUM), &["-v", "Y"]);
    assert_eq!(values["value"], Value::Enum("Y".into()));
}

#[test]
fn should_parse_enum_positional_argument() {
    let (_, values, _) = parse_positionals(&[], &[spec("value", None, TEST_ENUM)], &["Y"]);
    assert_eq!(values["value"], Value::Enum("Y".into()));
}

const INT_LIST: Kind = Kind::IntegerList {
    delimiter: ':',
    max_elements: Some(3),
};

#[test]
fn should_parse_option_with_multiple_values() {
    let (parser, values) = parse(spec("values", Some('v'), INT_LIST), &["-v", "1:2:3"]);
    assert_eq!(values["values"], Value::Integers(vec![1, 2, 3]));
    assert!(parser.positional_values().is_empty());
    assert!(parser.unexpected_arguments().is_empty());
}

#[test]
fn should_parse_option_with_multiple_values_with_default_delimiter_value() {
    let (parser, values) = parse(
        spec("values", Some('m'), Kind::TextList { delimiter: ';' }),
        &["-m", "this;is;a;test"],
    );
    assert_eq!(
        values["values"],
        Value::Texts(["this", "is", "a", "test"].map(str::to_owned).to_vec())
    );
    assert!(parser.positional_values().is_empty());
    assert!(parser.unexpected_arguments().is_empty());
}

#[test]
fn should_handle_multiple_occurences_of_argument() {
    let (_, values) = parse(spec("values", Some('v'), INT_LIST), &["-v", "5", "-v", "6"]);
    assert_eq!(values["values"], Value::Integers(vec![5, 6]));
}

#[test]
fn should_handle_int_option_with_default_value_followed_by_string() {
    let mut option = spec("value", None, Kind::Integer);
    option.default = Some(Value::Integer(-1));
    let mut parser = OptionsParser::default();
    parser.allow_unexpected_arguments = true;
    let mut values = Values::new();
    parser
        .parse(&[option], &["--value", "test"], &mut values, |_| Ok(()))
        .unwrap();
    assert_eq!(values["value"], Value::Integer(-1));
    assert_eq!(parser.unexpected_arguments(), ["test"]);
    assert_eq!(parser.recreate_unparsed_arguments(), "test");
}

fn check_unparsed_with_positionals(args: &[&str], option: OptionSpec<'_>) {
    let expected = ParsedOption {
        long: option.long.into(),
        short: option.short,
        value: Value::Integer(8888),
    };
    let positionals = ["value", "arg2", "arg3", "arg4"].map(|name| spec(name, None, Kind::Text));
    let (parser, values, _) = parse_positionals(&[option], &positionals, args);
    assert_eq!(values["value"], Value::Text("positional argument".into()));
    assert_eq!(parser.parsed_options(), &[expected]);
    assert_eq!(parser.unexpected_arguments(), ["wrong-arg"]);
    assert_eq!(parser.recreate_unparsed_arguments(), "--wrong-arg");
}

macro_rules! with_positional {
    ($test:ident, $args:expr, $long:literal, $short:expr) => {
        #[test]
        fn $test() {
            check_unparsed_with_positionals(&$args, spec($long, $short, Kind::Integer));
        }
    };
}

with_positional!(
    should_recreate_unparsed_argument1,
    ["positional argument", "--wrong-arg", "-p8888"],
    "p",
    Some('p')
);
with_positional!(
    should_recreate_unparsed_argument2,
    ["positional argument", "--wrong-arg", "-p", "8888"],
    "p",
    Some('p')
);
with_positional!(
    should_recreate_unparsed_argument3,
    ["positional argument", "-p8888", "--wrong-arg"],
    "p",
    Some('p')
);
with_positional!(
    should_recreate_unparsed_argument4,
    ["positional argument", "--wrong-arg", "--port", "8888"],
    "port",
    None
);
with_positional!(
    should_recreate_unparsed_argument5,
    ["positional argument", "--wrong-arg", "--port=8888"],
    "port",
    None
);
with_positional!(
    should_recreate_unparsed_argument6,
    ["positional argument", "--port", "8888", "--wrong-arg"],
    "port",
    None
);
with_positional!(
    should_recreate_unparsed_argument7,
    ["positional argument", "--port=8888", "--wrong-arg"],
    "port",
    None
);
