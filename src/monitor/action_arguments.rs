#[derive(Clone, Copy)]
pub(crate) struct Parameter {
    pub(crate) name: &'static str,
    pub(crate) default: Option<&'static str>,
}

pub(crate) struct Arguments<'a> {
    pub(crate) fixed: Vec<&'a str>,
    pub(crate) rest: Vec<&'a str>,
}

pub(crate) fn bind<'a>(
    words: &[&'a str],
    parameters: &[Parameter],
    variadic: bool,
) -> Result<Arguments<'a>, String> {
    let mut bound = vec![None; parameters.len()];
    let mut rest = Vec::new();
    let mut named_out_of_order = false;
    for &word in words {
        if let Some((name, value)) = word.split_once('=')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            let index = parameters
                .iter()
                .position(|parameter| parameter.name == name)
                .ok_or_else(|| format!("unknown named parameter {name}"))?;
            if bound.iter().position(Option::is_none) != Some(index) {
                named_out_of_order = true;
            }
            if bound[index].replace(value).is_some() {
                return Err(format!("duplicate parameter {name}"));
            }
        } else if let Some(slot) = bound.iter_mut().find(|slot| slot.is_none()) {
            if named_out_of_order {
                return Err("positional argument after an out-of-order named parameter".into());
            }
            *slot = Some(word);
        } else if variadic {
            rest.push(word);
        } else {
            return Err("too many arguments".into());
        }
    }
    let fixed = bound
        .into_iter()
        .zip(parameters)
        .map(|(value, parameter)| {
            value
                .or(parameter.default)
                .ok_or_else(|| format!("missing parameter {}", parameter.name))
        })
        .collect::<Result<_, _>>()?;
    Ok(Arguments { fixed, rest })
}

/// Split registered command arguments while keeping a bracketed value intact.
pub(crate) fn words(text: &str) -> Result<Vec<&str>, String> {
    let mut result = Vec::new();
    let mut start = None;
    let mut quote = None;
    let mut escaped = false;
    let mut brackets = 0usize;
    for (index, ch) in text.char_indices() {
        if let Some(delimiter) = quote {
            if std::mem::take(&mut escaped) {
                continue;
            }
            match ch {
                '\\' => escaped = true,
                ch if ch == delimiter => quote = None,
                _ => {}
            }
            continue;
        }
        if brackets == 0 && ch.is_whitespace() {
            if let Some(start) = start.take() {
                result.push(&text[start..index]);
            }
            continue;
        }
        if ch == '#' && start.is_none() {
            break;
        }
        start.get_or_insert(index);
        match ch {
            '"' | '\'' => quote = Some(ch),
            '[' => brackets += 1,
            ']' => {
                brackets = brackets
                    .checked_sub(1)
                    .ok_or("unexpected closing bracket")?
            }
            _ => {}
        }
    }
    if quote.is_some() || brackets != 0 {
        return Err("unterminated quote or bracket".into());
    }
    if let Some(start) = start {
        result.push(&text[start..]);
    }
    Ok(result)
}

pub(crate) fn commands(text: &str) -> impl Iterator<Item = &str> {
    let mut quote = None;
    let mut escaped = false;
    let mut brackets = 0usize;
    text.split(move |ch| {
        if let Some(delimiter) = quote {
            if !std::mem::take(&mut escaped) {
                if ch == '\\' {
                    escaped = true;
                } else if ch == delimiter {
                    quote = None;
                }
            }
        } else {
            match ch {
                '"' | '\'' => quote = Some(ch),
                '[' => brackets += 1,
                ']' => brackets = brackets.saturating_sub(1),
                ';' => return brackets == 0,
                _ => {}
            }
        }
        false
    })
}
