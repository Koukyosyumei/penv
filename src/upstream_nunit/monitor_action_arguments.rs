use crate::{
    Machine,
    monitor::{
        AutoLoadCommand, MonitorBuiltins,
        action_arguments::{Parameter, bind},
    },
};

const REQUIRED_STRING: &[Parameter] = &[Parameter {
    name: "arg",
    default: None,
}];
const OPTIONAL_STRING: &[Parameter] = &[Parameter {
    name: "arg",
    default: Some("\"default\""),
}];
const OPTIONAL_INTEGERS: &[Parameter] = &[
    Parameter {
        name: "a",
        default: Some("1"),
    },
    Parameter {
        name: "b",
        default: Some("2"),
    },
    Parameter {
        name: "c",
        default: Some("3"),
    },
];

fn integer(value: &str) -> Result<i32, String> {
    value
        .parse()
        .map_err(|_| format!("expected integer: {value}"))
}

fn integers(values: &[&str]) -> Result<String, String> {
    let values = values
        .iter()
        .map(|value| integer(value).map(|n| n.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(values.join(", "))
}

fn fixture_emulation(_: &mut Machine, words: &[&str]) -> Result<String, String> {
    let (&method, words) = words.split_first().ok_or("missing method name")?;
    let (parameters, variadic) = match method {
        "MethodThatTakesParamsIntArray" => (&[][..], true),
        "MethodThatTakesParamsIntArrayAfterString" => (REQUIRED_STRING, true),
        "MethodThatTakesParamsIntArrayAfterOptionalString" => (OPTIONAL_STRING, true),
        "MethodWithOptionalParameters" => (OPTIONAL_INTEGERS, false),
        "MethodWithOptionalParametersAndParamArray" => (OPTIONAL_INTEGERS, true),
        _ => return Err(format!("unknown fixture method {method}")),
    };
    let arguments = bind(words, parameters, variadic)?;
    let rest = integers(&arguments.rest)?;
    match arguments.fixed.as_slice() {
        [] => Ok(format!("args: [{rest}]")),
        [arg] => {
            let arg = arg
                .strip_prefix('"')
                .and_then(|arg| arg.strip_suffix('"'))
                .ok_or("expected quoted string")?;
            Ok(format!("arg: {arg} args: [{rest}]"))
        }
        [a, b, c] => {
            let mut output = format!("a: {}, b: {}, c: {}", integer(a)?, integer(b)?, integer(c)?);
            if variadic {
                output.push_str("; ");
                output.push_str(&rest);
            }
            Ok(output)
        }
        _ => unreachable!(),
    }
}

fn check(command: &str, expected: &str) {
    let mut monitor = MonitorBuiltins::default();
    let mut machine = Machine::without_default_ram();
    AutoLoadCommand {
        name: "emulation",
        description: "MockExtension",
        execute: fixture_emulation,
    }
    .register(&mut monitor)
    .unwrap();
    let output = monitor.execute(&mut machine, command).unwrap().unwrap();
    assert!(
        output.contains(expected),
        "{output:?} did not contain {expected:?}"
    );
}

#[test]
fn params_int_array_empty_test() {
    check("emulation MethodThatTakesParamsIntArray", "args: []");
}

#[test]
fn params_int_array_test() {
    check(
        "emulation MethodThatTakesParamsIntArray 1 2 3",
        "args: [1, 2, 3]",
    );
}

#[test]
fn params_int_array_after_string_test() {
    check(
        "emulation MethodThatTakesParamsIntArrayAfterString \"bcd\" 1 2 3",
        "arg: bcd args: [1, 2, 3]",
    );
}

#[test]
fn params_int_array_empty_after_string_passed_by_name() {
    check(
        "emulation MethodThatTakesParamsIntArrayAfterString arg=\"bcd\"",
        "arg: bcd args: []",
    );
}

#[test]
fn params_int_array_after_string_passed_by_name() {
    check(
        "emulation MethodThatTakesParamsIntArrayAfterString arg=\"bcd\" 1 2 3",
        "arg: bcd args: [1, 2, 3]",
    );
}

#[test]
fn params_int_array_after_optional_string_empty_test() {
    check(
        "emulation MethodThatTakesParamsIntArrayAfterOptionalString",
        "arg: default args: []",
    );
}

#[test]
fn params_int_array_after_optional_string_test() {
    check(
        "emulation MethodThatTakesParamsIntArrayAfterOptionalString \"bcd\" 1 2 3",
        "arg: bcd args: [1, 2, 3]",
    );
}

#[test]
fn optional_parameters_provide_none() {
    check("emulation MethodWithOptionalParameters", "a: 1, b: 2, c: 3");
}

#[test]
fn optional_parameters_provide_all() {
    check(
        "emulation MethodWithOptionalParameters 3 4 5",
        "a: 3, b: 4, c: 5",
    );
}

#[test]
fn optional_parameters_provide_first_by_name() {
    check(
        "emulation MethodWithOptionalParameters a=4",
        "a: 4, b: 2, c: 3",
    );
}

#[test]
fn optional_parameters_provide_middle_by_name() {
    check(
        "emulation MethodWithOptionalParameters 2 b=4",
        "a: 2, b: 4, c: 3",
    );
}

#[test]
fn optional_parameters_provide_first_by_name_rest_by_pos() {
    check(
        "emulation MethodWithOptionalParameters a=3 4 5",
        "a: 3, b: 4, c: 5",
    );
}

#[test]
fn optional_parameters_provide_last_by_name() {
    check(
        "emulation MethodWithOptionalParameters 3 c=4",
        "a: 3, b: 2, c: 4",
    );
}

#[test]
fn optional_parameters_provide_all_by_name() {
    check(
        "emulation MethodWithOptionalParameters a=3 b=4 c=5",
        "a: 3, b: 4, c: 5",
    );
}

#[test]
fn optional_parameters_provide_all_by_name_reverse_order() {
    check(
        "emulation MethodWithOptionalParameters c=5 b=4 a=3",
        "a: 3, b: 4, c: 5",
    );
}

#[test]
fn optional_parameters_provide_all_by_name_and_empty_array() {
    check(
        "emulation MethodWithOptionalParametersAndParamArray c=5 b=4 a=3",
        "a: 3, b: 4, c: 5; ",
    );
}

#[test]
fn optional_parameters_provide_all_by_name_and_array() {
    check(
        "emulation MethodWithOptionalParametersAndParamArray c=5 b=4 a=3 9 6",
        "a: 3, b: 4, c: 5; 9, 6",
    );
}
