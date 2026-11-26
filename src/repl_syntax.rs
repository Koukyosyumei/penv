pub(crate) fn unquoted_delimiters<'a>(
    text: &'a str,
    delimiters: &'static [char],
) -> impl Iterator<Item = (usize, char)> + 'a {
    let mut quoted = false;
    let mut escaped = false;
    text.char_indices().filter(move |&(_, character)| {
        match character {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            _ if !quoted && delimiters.contains(&character) => return true,
            _ => {}
        }
        false
    })
}

pub(crate) fn without_comment(line: &str) -> &str {
    unquoted_delimiters(line, &['/'])
        .find(|(index, _)| {
            let prefix = &line[..*index];
            line[*index..].starts_with("//")
                && !prefix.ends_with("@http:")
                && !prefix.ends_with("@https:")
        })
        .map_or(line, |(index, _)| &line[..index])
}

pub(crate) fn quoted_value(value: &str) -> Option<String> {
    let value = value.trim();
    if let Some(body) = value
        .strip_prefix("'''")
        .and_then(|body| body.strip_suffix("'''"))
    {
        Some(decode_multiline(body))
    } else {
        quoted_string(value)
    }
}

fn decode_multiline(body: &str) -> String {
    let mut decoded = String::new();
    let mut start = 0;
    for (index, _) in body.match_indices("\\'''") {
        if index != 0 && body.as_bytes()[index - 1] == b'\\' {
            continue;
        }
        decoded.push_str(&body[start..index]);
        start = index + 1;
    }
    decoded.push_str(&body[start..]);
    decoded
}

pub(crate) fn quoted_string(value: &str) -> Option<String> {
    let mut chars = value.trim().strip_prefix('"')?.chars();
    let mut decoded = String::new();
    while let Some(character) = chars.next() {
        match character {
            '"' => return chars.as_str().trim().is_empty().then_some(decoded),
            '\\' => match chars.next()? {
                escaped @ ('"' | '\\') => decoded.push(escaped),
                '\'' if chars.as_str().starts_with("''") => decoded.push('\''),
                _ => return None,
            },
            character => decoded.push(character),
        }
    }
    None
}

pub(crate) fn number_prefix(input: &str) -> Option<(std::borrow::Cow<'_, str>, &str)> {
    static NUMBER: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let matched = NUMBER
        .get_or_init(|| {
            regex::Regex::new(
                r"^(?:-\s*)?(?:0x[0-9a-fA-F]+(?:_[0-9a-fA-F]+)*|[0-9]+(?:_[0-9]+)*(?:\.[0-9]+)?)",
            )
            .expect("valid REPL number pattern")
        })
        .find(input)?;
    let (number, tail) = input.split_at(matched.end());
    let number = if number.contains(|c: char| c == '_' || c.is_whitespace()) {
        number.replace(|c: char| c == '_' || c.is_whitespace(), "").into()
    } else {
        number.into()
    };
    Some((number, tail))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renode_quoted_fixture_single() {
        assert_eq!(
            quoted_value(r#""one with \''' escaped quote""#).as_deref(),
            Some("one with ''' escaped quote"),
        );
    }

    #[test]
    fn renode_quoted_fixture_closing() {
        assert_eq!(
            quoted_value(r"'''one with escaped quote\\'''").as_deref(),
            Some(r"one with escaped quote\\"),
        );
    }

    #[test]
    fn renode_quoted_fixture_interior() {
        assert_eq!(
            quoted_value("'''this is \\\\\\'''\nnot a single line '''").as_deref(),
            Some("this is \\\\\\'''\nnot a single line "),
        );
    }

    #[test]
    fn repl_comments_preserve_urls_and_strings_in_inline_properties() {
        let source =
            r#"p: Device @ sysbus { asset: @https://example.invalid/file; name: "//;}" } "#;
        assert_eq!(without_comment(&format!("{source}// tail")), source);
        assert_eq!(unquoted_delimiters(source, &[';']).count(), 1);
        assert_eq!(without_comment("name:// comment"), "name:");
    }
}
