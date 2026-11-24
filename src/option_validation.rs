use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Integer,
    NullableInteger,
    Text,
    Enum(&'static [&'static str]),
    IntegerList {
        delimiter: char,
        max_elements: Option<usize>,
    },
    TextList {
        delimiter: char,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    Integer(i32),
    Text(String),
    Enum(String),
    Integers(Vec<i32>),
    Texts(Vec<String>),
}

impl Kind {
    fn accepts(self, value: &Value) -> bool {
        matches!(
            (self, value),
            (Self::Bool, Value::Bool(_))
                | (Self::Integer | Self::NullableInteger, Value::Integer(_))
                | (Self::Text, Value::Text(_))
                | (Self::Enum(_), Value::Enum(_))
                | (Self::IntegerList { .. }, Value::Integers(_))
                | (Self::TextList { .. }, Value::Texts(_))
        )
    }

    fn parse(self, argument: &str) -> Option<Value> {
        match self {
            Self::Bool => argument.parse().ok().map(Value::Bool),
            Self::Integer | Self::NullableInteger => argument.parse().ok().map(Value::Integer),
            Self::Text => Some(Value::Text(argument.into())),
            Self::Enum(names) => names
                .contains(&argument)
                .then(|| Value::Enum(argument.into())),
            Self::IntegerList {
                delimiter,
                max_elements,
            } => argument
                .splitn(max_elements.unwrap_or(usize::MAX), delimiter)
                .map(str::parse)
                .collect::<Result<_, _>>()
                .ok()
                .map(Value::Integers),
            Self::TextList { delimiter } => Some(Value::Texts(
                argument.split(delimiter).map(str::to_owned).collect(),
            )),
        }
    }

    fn type_name(self) -> &'static str {
        match self {
            Self::Bool => "Boolean",
            Self::Integer => "Int32",
            Self::NullableInteger => "Nullable<Int32>",
            Self::Text => "String",
            Self::Enum(_) => "Enum",
            Self::IntegerList { .. } => "Int32[]",
            Self::TextList { .. } => "String[]",
        }
    }
}

pub struct OptionSpec<'a> {
    pub long: &'a str,
    pub short: Option<char>,
    pub kind: Kind,
    pub required: bool,
    pub default: Option<Value>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    Configuration(String),
    Validation(String),
}

pub type Values = BTreeMap<String, Value>;

#[derive(Debug, PartialEq, Eq)]
pub struct ParsedOption {
    pub long: String,
    pub short: Option<char>,
    pub value: Value,
}

#[derive(Default)]
pub struct OptionsParser {
    pub throw_validation_exception: bool,
    pub allow_unexpected_arguments: bool,
    parsed: Vec<ParsedOption>,
    positionals: Vec<Value>,
    unexpected: Vec<String>,
    unparsed_tokens: Vec<String>,
}

impl OptionsParser {
    pub fn parsed_options(&self) -> &[ParsedOption] {
        &self.parsed
    }

    pub fn positional_values(&self) -> &[Value] {
        &self.positionals
    }

    pub fn unexpected_arguments(&self) -> &[String] {
        &self.unexpected
    }

    pub fn recreate_unparsed_arguments(&self) -> String {
        self.unparsed_tokens
            .iter()
            .filter(|token| !token.is_empty() && *token != "-")
            .map(|token| {
                let escaped = token.replace('"', "\\\"");
                if token.contains(' ') {
                    format!("\"{escaped}\"")
                } else {
                    escaped
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn parse(
        &mut self,
        schema: &[OptionSpec<'_>],
        args: &[&str],
        values: &mut Values,
        validate: impl FnOnce(&Values) -> Result<(), String>,
    ) -> Result<bool, ParseError> {
        self.parse_with_positionals(schema, &[], args, values, validate)
    }

    pub fn parse_with_positionals(
        &mut self,
        schema: &[OptionSpec<'_>],
        positionals: &[OptionSpec<'_>],
        args: &[&str],
        values: &mut Values,
        validate: impl FnOnce(&Values) -> Result<(), String>,
    ) -> Result<bool, ParseError> {
        self.parsed.clear();
        self.positionals.clear();
        self.unexpected.clear();
        self.unparsed_tokens.clear();
        for option in schema {
            if let Some(default) = &option.default {
                if !option.kind.accepts(default) {
                    return Err(ParseError::Configuration(format!(
                        "Default value for option '{}' is of unexpected type.",
                        option.long
                    )));
                }
                values.insert(option.long.into(), default.clone());
            }
        }
        match self
            .bind(schema, positionals, args, values)
            .and_then(|()| validate(values))
            .and_then(|()| {
                if self.allow_unexpected_arguments || self.unexpected.is_empty() {
                    Ok(())
                } else {
                    Err(format!(
                        "Unexpected options detected: {}",
                        self.recreate_unparsed_arguments()
                    ))
                }
            }) {
            Ok(()) => Ok(true),
            Err(error) if self.throw_validation_exception => Err(ParseError::Validation(error)),
            Err(_) => Ok(false),
        }
    }

    fn bind(
        &mut self,
        schema: &[OptionSpec<'_>],
        positionals: &[OptionSpec<'_>],
        args: &[&str],
        values: &mut Values,
    ) -> Result<(), String> {
        let mut specified = HashSet::new();
        let mut args = args.iter().copied().peekable();
        let mut escaped = false;
        while let Some(arg) = args.next() {
            if !escaped && arg == "--" {
                escaped = true;
            } else if escaped || !arg.starts_with('-') || arg == "-" {
                if let Some(positional) = positionals.get(self.positionals.len()) {
                    let value = positional.kind.parse(arg).ok_or_else(|| {
                        format!(
                            "Invalid value '{}': expected {}",
                            positional.long,
                            positional.kind.type_name()
                        )
                    })?;
                    values.insert(positional.long.into(), value.clone());
                    self.positionals.push(value);
                } else {
                    self.unexpected.push(arg.into());
                    self.unparsed_tokens.push(arg.into());
                }
            } else if let Some(long) = arg.strip_prefix("--") {
                let (name, inline) = long
                    .split_once('=')
                    .map_or((long, None), |(name, value)| (name, Some(value)));
                if let Some(option) = schema.iter().find(|option| option.long == name) {
                    let value = option.read_value(inline, &mut args)?;
                    self.record(option, value, &mut specified, values)?;
                } else {
                    self.unexpected.push(name.into());
                    self.unparsed_tokens.push(arg.into());
                }
            } else {
                let mut chars = arg[1..].chars();
                let mut remainder = String::from("-");
                while let Some(name) = chars.next() {
                    let Some(option) = schema.iter().find(|option| option.short == Some(name))
                    else {
                        self.unexpected.push(name.to_string());
                        remainder.push(name);
                        continue;
                    };
                    let value = if option.kind == Kind::Bool {
                        Value::Bool(true)
                    } else {
                        let rest = chars.as_str();
                        chars = "".chars();
                        option.read_value((!rest.is_empty()).then_some(rest), &mut args)?
                    };
                    self.record(option, value, &mut specified, values)?;
                }
                if remainder != "-" {
                    self.unparsed_tokens.push(remainder);
                }
            }
        }
        if let Some(missing) = positionals
            .iter()
            .skip(self.positionals.len())
            .find(|value| value.required)
        {
            return Err(format!("Required value '{}' is missing.", missing.long));
        }
        if let Some(missing) = schema
            .iter()
            .find(|option| option.required && !specified.contains(option.long))
        {
            return Err(format!("Required option '{}' is missing.", missing.long));
        }
        Ok(())
    }

    fn record<'a>(
        &mut self,
        option: &OptionSpec<'a>,
        value: Value,
        specified: &mut HashSet<&'a str>,
        values: &mut Values,
    ) -> Result<(), String> {
        if specified.insert(option.long) {
            values.insert(option.long.into(), value.clone());
        } else {
            match (values.get_mut(option.long), &value) {
                (Some(Value::Integers(previous)), Value::Integers(next)) => {
                    previous.extend(next.iter().copied())
                }
                (Some(Value::Texts(previous)), Value::Texts(next)) => {
                    previous.extend(next.iter().cloned())
                }
                _ => return Err(format!("Option '{}' occurs more than once", option.long)),
            }
        }
        self.parsed.push(ParsedOption {
            long: option.long.into(),
            short: option.short,
            value,
        });
        Ok(())
    }
}

impl OptionSpec<'_> {
    fn read_value<'a>(
        &self,
        inline: Option<&'a str>,
        args: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
    ) -> Result<Value, String> {
        if self.kind == Kind::Bool {
            return Ok(Value::Bool(true));
        }
        if let Some(value) = inline.and_then(|arg| self.kind.parse(arg)) {
            return Ok(value);
        }
        if inline.is_none() {
            if let Some(value) = args
                .peek()
                .filter(|arg| {
                    !arg.starts_with('-')
                        || matches!(self.kind, Kind::Integer | Kind::NullableInteger)
                })
                .and_then(|arg| self.kind.parse(arg))
            {
                args.next();
                return Ok(value);
            }
            if let Some(default) = &self.default {
                return Ok(default.clone());
            }
        }
        Err(format!(
            "Option '{}' requires parameter of type '{}'",
            self.long,
            self.kind.type_name()
        ))
    }
}
