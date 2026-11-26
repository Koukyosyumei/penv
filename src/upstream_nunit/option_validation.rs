use crate::option_validation::{Kind, OptionSpec, OptionsParser, ParseError, Value, Values};

#[test]
fn should_throw_exception_when_required_paramter_is_not_found() {
    let schema = [OptionSpec {
        long: "requiredParameter",
        short: None,
        kind: Kind::Bool,
        required: true,
        default: None,
    }];
    let mut parser = OptionsParser::default();
    parser.throw_validation_exception = true;
    assert!(matches!(
        parser.parse(&schema, &[], &mut Values::new(), |_| Ok(())),
        Err(ParseError::Validation(_))
    ));
}

#[test]
fn should_set_default_value_when_parameter_is_not_found() {
    let schema = [OptionSpec {
        long: "numericParameter",
        short: None,
        kind: Kind::Integer,
        required: false,
        default: Some(Value::Integer(147)),
    }];
    let mut parser = OptionsParser::default();
    let mut values = Values::new();
    parser.parse(&schema, &[], &mut values, |_| Ok(())).unwrap();
    assert_eq!(parser.parsed_options().len(), 0);
    assert_eq!(values["numericParameter"], Value::Integer(147));
}

#[test]
fn should_call_custom_validator() {
    let schema = [
        OptionSpec {
            long: "numericValue",
            short: Some('n'),
            kind: Kind::Integer,
            required: false,
            default: None,
        },
        OptionSpec {
            long: "stringValue",
            short: None,
            kind: Kind::Text,
            required: false,
            default: None,
        },
    ];
    let mut parser = OptionsParser::default();
    parser.throw_validation_exception = true;
    let mut values = Values::new();
    let result = parser.parse(
        &schema,
        &["--stringValue=foo", "-n1"],
        &mut values,
        |values| {
            if values["numericValue"] == Value::Integer(1)
                && values["stringValue"] == Value::Text("foo".into())
            {
                Err("Field cannot have values: 1 and 'foo' at the same time".into())
            } else {
                Ok(())
            }
        },
    );
    assert!(matches!(result, Err(ParseError::Validation(_))));
    assert_eq!(values["numericValue"], Value::Integer(1));
    assert_eq!(values["stringValue"], Value::Text("foo".into()));
}

#[test]
fn should_throw_an_exception_when_default_value_is_of_a_wrong_type() {
    let schema = [OptionSpec {
        long: "arg",
        short: None,
        kind: Kind::Text,
        required: false,
        default: Some(Value::Integer(0)),
    }];
    let error = OptionsParser::default()
        .parse(&schema, &["--arg"], &mut Values::new(), |_| Ok(()))
        .unwrap_err();
    let ParseError::Configuration(message) = error else {
        panic!("expected argument error")
    };
    assert!(message.contains("is of unexpected type"));
}
