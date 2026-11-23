use super::prelex;

fn assert_output(source: &str, expected: &str) {
    assert_eq!(
        prelex(source).unwrap(),
        expected.split('\n').collect::<Vec<_>>()
    );
}

#[test]
fn renode_prelexer_process_empty_file() {
    assert_eq!(prelex("").unwrap().first().map(String::as_str), Some(""));
}

#[test]
fn renode_prelexer_process_simple_file() {
    assert_output(
        "first line\nsecond line\n    first indented\n    second indented\nthird line",
        "first line;\nsecond line{\n    first indented;\n    second indented};\nthird line",
    );
}

#[test]
fn renode_prelexer_process_double_dedent() {
    assert_output(
        "first line\nsecond line\n    first indented\n    second indented\n\nthird line",
        "first line;\nsecond line{\n    first indented;\n    second indented};\n\nthird line",
    );
}

#[test]
fn renode_prelexer_process_double_dedent_at_the_end_of_file() {
    assert_output(
        "first line\nsecond line\n    first indented",
        "first line;\nsecond line{\n    first indented}",
    );
}

#[test]
fn renode_prelexer_process_two_lines_with_no_indent() {
    assert_output("\nline1\nline2", "\nline1;\nline2");
}

#[test]
fn renode_prelexer_process_two_lines_with_no_indent_and_separation() {
    assert_output("\nline1\n\nline2", "\nline1;\n\nline2");
}

#[test]
fn renode_prelexer_handle_empty_lines_at_the_end_of_source() {
    assert_output("\nline1\nline2\n\n", "\nline1;\nline2\n\n");
}

#[test]
fn renode_prelexer_not_process_indent_in_braces() {
    assert_output("\nline1 { \n    line2 }", "\nline1 { \n    line2 }");
}

#[test]
fn renode_prelexer_handle_multiline_comments_in_one_line() {
    assert_output(
        "\nline1 used as a ruler1234            123456789                1234\nline 2/* first comment*/ \"string with /*\" /* second comment */ something",
        "\nline1 used as a ruler1234            123456789                1234;\nline 2                   \"string with /*\"                      something",
    );
}

#[test]
fn renode_prelexer_handle_multiline_comments() {
    assert_output(
        "\nline1/* here we begin\n    here it goes\nmore\nmore\n    here we finish*/\nline2",
        "\nline1;\n\n\n\n\nline2",
    );
}

#[test]
fn renode_prelexer_handle_multiline_comments_within_braces() {
    assert_output(
        "\nline1 { /* here we begin\n    here it goes\nhere the comment ends*/ x: 5 }\nline2",
        "\nline1 { \n\n                        x: 5 };\nline2",
    );
}

#[test]
fn renode_prelexer_process_brace_in_string() {
    assert_output(
        "\nline1 \"{ \\\" {\"\nline2",
        "\nline1 \"{ \\\" {\";\nline2",
    );
}

#[test]
fn renode_prelexer_handle_text_on_the_first_line() {
    assert_output("onlyLine", "onlyLine");
}

#[test]
fn renode_prelexer_handle_initial_indent() {
    assert_eq!(
        prelex(
            "\n    first line\n    second line\n        first indented\n        second indented\n    third line"
        ),
        Err("WrongIndent: First line with text cannot be indented.")
    );
}
