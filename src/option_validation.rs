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
    parsed: Vec<ParsedOption>,
}

impl OptionsParser {
    pub fn parsed_options(&self) -> &[ParsedOption] {
        &self.parsed
    }

    pub fn parse(
        &mut self,
        schema: &[OptionSpec<'_>],
        args: &[&str],
        values: &mut Values,
        validate: impl FnOnce(&Values) -> Result<(), String>,
    ) -> Result<bool, ParseError> {
        self.parsed.clear();
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
        {
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
        while let Some(arg) = args.next() {
            let (option, inline) = if let Some(long) = arg.strip_prefix("--") {
                let (name, value) = long
                    .split_once('=')
                    .map_or((long, None), |(name, value)| (name, Some(value)));
                (schema.iter().find(|option| option.long == name), value)
            } else if let Some(short) = arg.strip_prefix('-') {
                let mut chars = short.chars();
                let name = chars.next();
                let rest = chars.as_str();
                (
                    schema
                        .iter()
                        .find(|option| name.is_some() && option.short == name),
                    (!rest.is_empty()).then_some(rest),
                )
            } else {
                return Err(format!("unexpected argument '{arg}'"));
            };
            let Some(option) = option else {
                return Err(format!("unexpected option '{arg}'"));
            };
            if !specified.insert(option.long) {
                return Err(format!("Option '{}' occurs more than once", option.long));
            }
            let argument = inline.or_else(|| {
                if option.kind != Kind::Bool
                    && args.peek().is_some_and(|arg| {
                        !arg.starts_with('-')
                            || (matches!(option.kind, Kind::Integer | Kind::NullableInteger)
                                && arg.parse::<i32>().is_ok())
                    })
                {
                    args.next()
                } else {
                    None
                }
            });
            let value = match (option.kind, argument) {
                (Kind::Bool, None) => Some(Value::Bool(true)),
                (Kind::Bool, Some(argument)) => {
                    argument.to_ascii_lowercase().parse().ok().map(Value::Bool)
                }
                (Kind::Integer | Kind::NullableInteger, Some(argument)) => {
                    argument.parse().ok().map(Value::Integer)
                }
                (Kind::Text, Some(argument)) => Some(Value::Text(argument.into())),
                _ => None,
            }
            .ok_or_else(|| {
                let expected = match option.kind {
                    Kind::Bool => "Boolean",
                    Kind::Integer => "Int32",
                    Kind::NullableInteger => "Nullable<Int32>",
                    Kind::Text => "String",
                };
                format!(
                    "Option '{}' requires parameter of type '{expected}'",
                    option.long
                )
            })?;
            values.insert(option.long.into(), value.clone());
            self.parsed.push(ParsedOption {
                long: option.long.into(),
                short: option.short,
                value,
            });
        }
        if let Some(missing) = schema
            .iter()
            .find(|option| option.required && !specified.contains(option.long))
        {
            return Err(format!("Required option '{}' is missing.", missing.long));
        }
        Ok(())
    }
}
