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
    for &word in words {
        if let Some((name, value)) = word.split_once('=')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            let index = parameters
                .iter()
                .position(|parameter| parameter.name == name)
                .ok_or_else(|| format!("unknown named parameter {name}"))?;
            if bound[index].replace(value).is_some() {
                return Err(format!("duplicate parameter {name}"));
            }
        } else if let Some(slot) = bound.iter_mut().find(|slot| slot.is_none()) {
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
