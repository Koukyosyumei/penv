use regex::Regex;
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Comment,
    Execution,
    Variable,
    LeftBrace,
    RightBrace,
    MultilineString,
    MultilineStringTerminator,
    String,
    AbsoluteRange,
    RelativeRange,
    Path,
    Hex,
    TimeInterval,
    Float,
    DecimalInteger,
    Boolean,
    Null,
    Equality,
    ConditionalEquality,
    CommandSplit,
    Comma,
    Literal,
}

#[derive(Debug, PartialEq)]
pub enum Value {
    Text(String),
    Integer(i64),
    Float(f32),
    Boolean(bool),
    Null,
    Range(u64, u64),
    TimeTicks(u64),
}

#[derive(Debug, PartialEq)]
pub struct Token {
    pub kind: Kind,
    pub value: Value,
    source: String,
}

impl Token {
    pub fn time_ticks(&self) -> Option<u64> {
        match self.kind {
            Kind::TimeInterval | Kind::DecimalInteger | Kind::Float => time_ticks(&self.source),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Tokenization {
    pub tokens: Vec<Token>,
    pub unmatched_characters: usize,
    pub error: Option<&'static str>,
}

static RULES: LazyLock<Vec<(Option<Kind>, Regex)>> = LazyLock::new(|| {
    use Kind::*;
    [
        (Some(Comment), r"^#.*"),
        (Some(Execution), r"^`[^`\n]*`"),
        (Some(Variable), r"^\$[0-9a-zA-Z_.]+"),
        (Some(LeftBrace), r"^\["),
        (Some(RightBrace), r"^\]"),
        (Some(MultilineString), r#"(?s)^""".+?""""#),
        (Some(MultilineStringTerminator), r#"^""""#),
        (Some(String), r"^'[^'\\]*(?:\\.[^'\\]*)*'"),
        (Some(String), r#"^"[^"\\]*(?:\\.[^"\\]*)*""#),
        (
            Some(AbsoluteRange),
            r"^<\s*(?:0x[0-9a-fA-F]+|[+-]?\d+)\s*,\s*(?:0x[0-9a-fA-F]+|[+-]?\d+)\s*>",
        ),
        (
            Some(RelativeRange),
            r"^<\s*(?:0x[0-9a-fA-F]+|[+-]?\d+)\s+(?:0x[0-9a-fA-F]+|\+?\d+)\s*>",
        ),
        (Some(Path), r"^@(?:\\ |[^\s;])+"),
        (Some(Hex), r"^0x[0-9a-fA-F]+"),
        (Some(TimeInterval), r"^(?:[0-9]+:){1,2}[0-9]+(?:\.[0-9]+)?"),
        (Some(Float), r"^[+-]?\d+\.\d*(?:e[+-]?\d+)?"),
        (Some(DecimalInteger), r"^[+-]?\d+(?:e[+-]?\d+)?"),
        (Some(Boolean), r"^(?:true|false|True|False)"),
        (Some(Null), r"^null"),
        (Some(Equality), r"^="),
        (Some(ConditionalEquality), r"^\?="),
        (Some(CommandSplit), r"^;"),
        (Some(Comma), r"^,"),
        (Some(Literal), r"^[\w.\-?][\w.\-?:]*"),
        (None, r"^\s+"),
        (Some(Comment), r"^:.*"),
    ]
    .into_iter()
    .map(|(kind, pattern)| (kind, Regex::new(pattern).unwrap()))
    .collect()
});

pub fn tokenize(mut input: &str) -> Tokenization {
    let mut tokens = Vec::new();
    let mut error = None;
    while let Some((kind, found)) = RULES
        .iter()
        .find_map(|(kind, rule)| rule.find(input).map(|found| (*kind, found)))
    {
        if let Some(kind) = kind {
            match value(kind, found.as_str()) {
                Ok(value) => tokens.push(Token {
                    kind,
                    value,
                    source: found.as_str().into(),
                }),
                Err(reason) => {
                    error = Some(reason);
                    break;
                }
            }
        }
        input = &input[found.end()..];
    }
    Tokenization {
        tokens,
        unmatched_characters: input.encode_utf16().count(),
        error,
    }
}

fn value(kind: Kind, source: &str) -> Result<Value, &'static str> {
    use Kind::*;
    let number_error = "invalid numeric token";
    let text = |value: &str| Ok(Value::Text(value.into()));
    match kind {
        Comment => text(source.trim_start_matches('#')),
        Variable => text(&source[1..]),
        Execution => text(&source[1..source.len() - 1]),
        MultilineString => text(&source[3..source.len() - 3]),
        MultilineStringTerminator => text("\""),
        String => {
            let delimiter = source.as_bytes()[0] as char;
            Ok(Value::Text(source[1..source.len() - 1].replace(
                &format!("\\{delimiter}"),
                &delimiter.to_string(),
            )))
        }
        Path => Ok(Value::Text(
            source.trim_start_matches('@').replace("\\ ", " "),
        )),
        DecimalInteger => {
            let parsed = if let Some((mantissa, exponent)) = source.split_once('e') {
                let mantissa: i64 = mantissa.parse().map_err(|_| number_error)?;
                let exponent: u32 = exponent.parse().map_err(|_| number_error)?;
                10i64
                    .checked_pow(exponent)
                    .and_then(|factor| mantissa.checked_mul(factor))
            } else {
                source.parse().ok()
            };
            parsed.map(Value::Integer).ok_or(number_error)
        }
        Hex => u64::from_str_radix(&source[2..], 16)
            .map(|n| Value::Integer(n as i64))
            .map_err(|_| number_error),
        Float => source.parse().map(Value::Float).map_err(|_| number_error),
        Boolean => Ok(Value::Boolean(source.eq_ignore_ascii_case("true"))),
        Null => Ok(Value::Null),
        TimeInterval => time_ticks(source).map(Value::TimeTicks).ok_or(number_error),
        AbsoluteRange | RelativeRange => {
            let mut parts = source[1..source.len() - 1]
                .split(|c: char| c.is_whitespace() || c == ',')
                .filter(|p| !p.is_empty());
            let mut read = || {
                let part = parts.next().ok_or(number_error)?;
                if let Some(hex) = part.strip_prefix("0x") {
                    u64::from_str_radix(hex, 16).map_err(|_| number_error)
                } else {
                    part.parse().map_err(|_| number_error)
                }
            };
            let start = read()?;
            let end = read()?;
            let length = if kind == AbsoluteRange {
                end.checked_sub(start)
                    .and_then(|distance| distance.checked_add(1))
                    .ok_or("invalid absolute range")?
            } else {
                end
            };
            Ok(Value::Range(start, length))
        }
        _ => text(source),
    }
}

fn time_ticks(source: &str) -> Option<u64> {
    static PATTERN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?:(?:([0-9]+):)?([0-9]+):)?([0-9]+)(\.[0-9]+)?").unwrap());
    let parts = PATTERN.captures(source)?;
    let unit = |index: usize| {
        parts
            .get(index)
            .map_or(Some(0), |m| m.as_str().parse::<u64>().ok())
    };
    let seconds = unit(1)?
        .checked_mul(3600)?
        .checked_add(unit(2)?.checked_mul(60)?)?
        .checked_add(unit(3)?)?;
    let fraction = parts.get(4).map_or(Some(0), |m| {
        m.as_str()
            .parse::<f64>()
            .ok()
            .map(|fraction| (fraction * 1_000_000_000.0) as u64)
    })?;
    seconds.checked_mul(1_000_000_000)?.checked_add(fraction)
}
