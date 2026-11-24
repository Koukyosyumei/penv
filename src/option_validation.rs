use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Integer,
    NullableInteger,
    Text,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    Integer(i32),
    Text(String),
}

impl Value {
    fn kind(&self) -> Kind {
        match self {
            Self::Bool(_) => Kind::Bool,
            Self::Integer(_) => Kind::Integer,
            Self::Text(_) => Kind::Text,
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
    unexpected: Vec<String>,
    unparsed_tokens: Vec<String>,
}

impl OptionsParser {
    pub fn parsed_options(&self) -> &[ParsedOption] {
        &self.parsed
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
        self.parsed.clear();
        self.unexpected.clear();
        self.unparsed_tokens.clear();
        for option in schema {
            if let Some(default) = &option.default {
                if default.kind() != option.kind
                    && !(option.kind == Kind::NullableInteger && default.kind() == Kind::Integer)
                {
                    return Err(ParseError::Configuration(format!(
                        "Default value for option '{}' is of unexpected type.",
                        option.long
                    )));
                }
                values.insert(option.long.into(), default.clone());
            }
        }
        match self
            .bind(schema, args, values)
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
                self.unexpected.push(arg.into());
                self.unparsed_tokens.push(arg.into());
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
        if !specified.insert(option.long) {
            return Err(format!("Option '{}' occurs more than once", option.long));
        }
        values.insert(option.long.into(), value.clone());
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
        let argument = inline.or_else(|| {
            if args.peek().is_some_and(|arg| {
                !arg.starts_with('-')
                    || (matches!(self.kind, Kind::Integer | Kind::NullableInteger)
                        && arg.parse::<i32>().is_ok())
            }) {
                args.next()
            } else {
                None
            }
        });
        let value = match (self.kind, argument) {
            (Kind::Integer | Kind::NullableInteger, Some(argument)) => {
                argument.parse().ok().map(Value::Integer)
            }
            (Kind::Text, Some(argument)) => Some(Value::Text(argument.into())),
            _ => None,
        };
        value.ok_or_else(|| {
            let expected = match self.kind {
                Kind::Bool => "Boolean",
                Kind::Integer => "Int32",
                Kind::NullableInteger => "Nullable<Int32>",
                Kind::Text => "String",
            };
            format!(
                "Option '{}' requires parameter of type '{expected}'",
                self.long
            )
        })
    }
}
