use crate::repl_parser::{ListItem, SymbolicValue, list_value, quoted_value};
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

fn array(value: &str) -> Result<Vec<ListItem<'_>>, String> {
    list_value(value).map_err(str::to_owned)
}

fn array_integers(value: &str, nullable: bool, separator: &str) -> Result<String, String> {
    let values = array(value)?
        .into_iter()
        .map(|item| match item {
            ListItem::Number(value) => integer(&value).map(|n| n.to_string()),
            ListItem::Symbol(SymbolicValue::Reference("null")) if nullable => Ok("null".into()),
            _ => Err("expected integer array element".into()),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(values.join(separator))
}

fn array_strings(value: &str) -> Result<String, String> {
    let strings = array(value)?
        .into_iter()
        .map(|item| match item {
            ListItem::String(value) => Ok(format!("{value:?}")),
            _ => Err("expected string array element".to_owned()),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(strings.join(", "))
}

fn array_method(method: &str, words: &[&str], names: &[&'static str]) -> Result<String, String> {
    let parameters: Vec<_> = names
        .iter()
        .map(|&name| Parameter {
            name,
            default: None,
        })
        .collect();
    let arguments = bind(words, &parameters, false)?;
    match (method, arguments.fixed.as_slice()) {
        ("MethodWithArrayThenString", [numbers, separator]) => Ok(format!(
            "numbers: [{}] separator: {}",
            array_integers(numbers, false, ", ")?,
            quoted_value(separator)?,
        )),
        ("MethodWithStringThenIntArrayThenAnotherString", [first, numbers, last]) => Ok(format!(
            "s1:{} numbers:[{}] s2:{}",
            quoted_value(first)?,
            array_integers(numbers, false, ",")?,
            quoted_value(last)?,
        )),
        ("MethodWithListOfStrings", [words]) => Ok(format!("words: [{}]", array_strings(words)?)),
        ("MethodWithNullableIntArray" | "MethodWithNullableIntList", [numbers]) => Ok(format!(
            "numbers: [{}]",
            array_integers(numbers, true, ", ")?
        )),
        _ => Err("unsupported fixture array method".into()),
    }
}

fn variadic_strings(words: &[&str]) -> Result<String, String> {
    let values = match words {
        [word] if word.starts_with('[') || word.starts_with("args=[") => {
            array_strings(word.strip_prefix("args=").unwrap_or(word))?
        }
        words => words
            .iter()
            .map(|word| quoted_value(word).map(|word| format!("{word:?}")))
            .collect::<Result<Vec<_>, _>>()?
            .join(", "),
    };
    Ok(format!("args: [{values}]"))
}

fn fixture_emulation(_: &mut Machine, words: &[&str]) -> Result<String, String> {
    let (&method, words) = words.split_first().ok_or("missing method name")?;
    let (parameters, variadic) = match method {
        "MethodThatTakesParamsIntArray" => (&[][..], true),
        "MethodThatTakesParamsIntArrayAfterString" => (REQUIRED_STRING, true),
        "MethodThatTakesParamsIntArrayAfterOptionalString" => (OPTIONAL_STRING, true),
        "MethodWithOptionalParameters" => (OPTIONAL_INTEGERS, false),
        "MethodWithOptionalParametersAndParamArray" => (OPTIONAL_INTEGERS, true),
        "MethodWithParamsStringArray" => return variadic_strings(words),
        "MethodWithArrayThenString" => {
            return array_method(method, words, &["numbers", "separator"]);
        }
        "MethodWithStringThenIntArrayThenAnotherString" => {
            return array_method(method, words, &["s1", "numbers", "s2"]);
        }
        "MethodWithListOfStrings" => return array_method(method, words, &["words"]),
        "MethodWithNullableIntArray" | "MethodWithNullableIntList" => {
            return array_method(method, words, &["numbers"]);
        }
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

const SIGNATURES: &[&str] = &[
    "MethodThatTakesParamsIntArray(params int[] args)",
    "MethodThatTakesParamsIntArrayAfterString(string arg, params int[] args)",
    "MethodThatTakesParamsIntArrayAfterOptionalString(string arg = default, params int[] args)",
    "MethodWithOptionalParameters(int a = 1, int b = 2, int c = 3)",
    "MethodWithOptionalParametersAndParamArray(int a = 1, int b = 2, int c = 3, params int[] args)",
    "MethodWithArrayThenString(int[] numbers, string separator)",
    "MethodWithStringThenIntArrayThenAnotherString(string s1, int[] numbers, string s2)",
    "MethodWithListOfStrings(List<string> words)",
    "MethodWithParamsStringArray(params string[] args)",
    "MethodWithNullableIntArray(int?[] numbers)",
    "MethodWithNullableIntList(List<int?> numbers)",
];

fn execute(command: &str) -> Result<String, String> {
    let mut monitor = MonitorBuiltins::default();
    let mut machine = Machine::without_default_ram();
    AutoLoadCommand {
        name: "emulation",
        description: "MockExtension",
        method_signatures: SIGNATURES,
        execute: fixture_emulation,
    }
    .register(&mut monitor)
    .unwrap();
    monitor.execute(&mut machine, command).unwrap()
}

fn check(command: &str, expected: &str) {
    let output = execute(command).unwrap();
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

#[test]
fn array_as_non_last_parameter_positional() {
    check(
        "emulation MethodWithArrayThenString [1, 2, 3] \",\"",
        "numbers: [1, 2, 3] separator: ,",
    );
}

#[test]
fn array_as_non_last_parameter_named() {
    check(
        "emulation MethodWithArrayThenString numbers=[1, 2, 3] separator=\",\"",
        "numbers: [1, 2, 3] separator: ,",
    );
}

#[test]
fn array_as_non_last_parameter_mixed() {
    check(
        "emulation MethodWithArrayThenString [1, 2, 3] separator=\",\"",
        "numbers: [1, 2, 3] separator: ,",
    );
}

#[test]
fn empty_array_as_non_last_parameter() {
    check(
        "emulation MethodWithArrayThenString [] \",\"",
        "numbers: [] separator: ,",
    );
}

#[test]
fn array_with_trailing_comma() {
    check(
        "emulation MethodWithArrayThenString [1, 2, 3, ] \",\"",
        "numbers: [1, 2, 3] separator: ,",
    );
}

#[test]
fn list_of_strings_with_trailing_comma() {
    check(
        "emulation MethodWithListOfStrings words=[\"a\", \"b\",]",
        "words: [\"a\", \"b\"]",
    );
}

#[test]
fn list_of_strings_positional() {
    check(
        "emulation MethodWithListOfStrings [\"a\", \"b c\", \"d\"]",
        "words: [\"a\", \"b c\", \"d\"]",
    );
}

#[test]
fn list_of_strings_named() {
    check(
        "emulation MethodWithListOfStrings words=[\"a\", \"b\", \"c\"]",
        "words: [\"a\", \"b\", \"c\"]",
    );
}

#[test]
fn list_of_strings_empty_positional() {
    check("emulation MethodWithListOfStrings []", "words: []");
}

#[test]
fn list_of_strings_empty_named() {
    check("emulation MethodWithListOfStrings words=[]", "words: []");
}

#[test]
fn params_string_array_positional_expanded() {
    check(
        "emulation MethodWithParamsStringArray \"one\" \"two three\" \"four\"",
        "args: [\"one\", \"two three\", \"four\"]",
    );
}

#[test]
fn array_positional_non_last() {
    check(
        "emulation MethodWithStringThenIntArrayThenAnotherString \"abc\" [1, 2] \"def\"",
        "s1:abc numbers:[1,2] s2:def",
    );
}

#[test]
fn params_string_array_params() {
    check(
        "emulation MethodWithParamsStringArray \"one\" \"two three\"",
        "args: [\"one\", \"two three\"]",
    );
}

#[test]
fn params_string_array_positional_bracketed() {
    check(
        "emulation MethodWithParamsStringArray [\"one\", \"two three\"]",
        "args: [\"one\", \"two three\"]",
    );
}

#[test]
fn params_string_array_named() {
    check(
        "emulation MethodWithParamsStringArray args=[\"one\",\"two three\"]",
        "args: [\"one\", \"two three\"]",
    );
}

#[test]
fn params_string_array_empty_params() {
    check("emulation MethodWithParamsStringArray", "args: []");
}

#[test]
fn params_string_array_empty_positional() {
    check("emulation MethodWithParamsStringArray []", "args: []");
}

#[test]
fn params_string_array_empty_named() {
    check("emulation MethodWithParamsStringArray args=[]", "args: []");
}

#[test]
fn nullable_int_array_containing_nulls() {
    check(
        "emulation MethodWithNullableIntArray [1, null, 3]",
        "numbers: [1, null, 3]",
    );
}

#[test]
fn nullable_int_array_containing_nulls_named() {
    check(
        "emulation MethodWithNullableIntArray numbers=[1, null, 3]",
        "numbers: [1, null, 3]",
    );
}

#[test]
fn nullable_int_list_containing_nulls() {
    check(
        "emulation MethodWithNullableIntList [1, null, 3]",
        "numbers: [1, null, 3]",
    );
}

#[test]
fn nullable_int_list_containing_nulls_named() {
    check(
        "emulation MethodWithNullableIntList numbers=[1, null, 3]",
        "numbers: [1, null, 3]",
    );
}

fn check_error(command: &str) {
    let output = execute(command).expect_err("expected failed command");
    assert!(
        output.contains("The following methods are available"),
        "{output}"
    );
}

#[test]
fn optional_parameters_fail_on_duplicate_name() {
    check_error("emulation MethodWithOptionalParameters c=5 c=3");
}

#[test]
fn optional_parameters_fail_on_duplicate_positional_and_named() {
    check_error("emulation MethodWithOptionalParameters 1 a=4");
}

#[test]
fn optional_parameters_fail_on_positional_after_named() {
    check_error("emulation MethodWithOptionalParameters b=1 4");
}

#[test]
fn positional_after_named_with_mismatched_position() {
    check_error(
        "emulation MethodWithStringThenIntArrayThenAnotherString s2=\"a\" [1, 2, 3] s1=\"b\"",
    );
}

#[test]
fn type_mismatch_in_params_array() {
    check_error("emulation MethodThatTakesParamsIntArray 1 \"2\" 3");
}

#[test]
fn type_mismatch_in_array() {
    check_error("emulation MethodWithArrayThenString [1, \"2\", 3] \",\"");
}

#[test]
fn unclosed_bracket() {
    check_error("emulation MethodWithArrayThenString [1, 2, 3 \",\"");
}

#[test]
fn unclosed_bracket_with_trailing_comma() {
    check_error("emulation MethodThatTakesParamsIntArray [1,");
}

#[test]
fn empty_element() {
    check_error("emulation MethodWithArrayThenString [1, , 3] \",\"");
}

#[test]
fn two_trailing_commas() {
    check_error("emulation MethodWithArrayThenString [1, 2, 3, ,] \",\"");
}

#[test]
fn single_element_passed_as_list_parameter() {
    check_error("emulation MethodWithListOfStrings \"a\"");
}

#[test]
fn single_element_passed_as_non_params_array_parameter() {
    check_error("emulation MethodWithArrayThenString 1 \",\"");
}
