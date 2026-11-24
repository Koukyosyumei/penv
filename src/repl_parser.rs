use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Component, Path, PathBuf},
    rc::Rc,
};

use crate::platform::validate_entry_properties;

type Properties = Vec<(SourceLine, String)>;

#[derive(Clone, Debug)]
pub(crate) struct SourceLine {
    path: Rc<Path>,
    line: usize,
}

impl std::fmt::Display for SourceLine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}:{}", self.path.display(), self.line)
    }
}

#[derive(Debug)]
struct Entry {
    pub(crate) name: String,
    pub(crate) model: String,
    local: bool,
    alias: Option<String>,
    pub(crate) registration: Option<String>,
    pub(crate) body: Properties,
    source: PathBuf,
    line: usize,
}

impl Entry {
    pub(crate) fn error(&self, detail: &str) -> String {
        format!(
            "{}:{}: {}: {detail}",
            self.source.display(),
            self.line,
            self.name
        )
    }

    pub(crate) fn sysbus(&self) -> Result<Option<u32>, String> {
        let registration = self
            .registration
            .as_deref()
            .ok_or_else(|| self.error("missing sysbus registration"))?;
        if registration == "sysbus" {
            return Ok(None);
        }
        let address = registration
            .strip_prefix("sysbus")
            .filter(|s| s.starts_with(char::is_whitespace))
            .ok_or_else(|| self.error("unsupported bus registration"))?;
        number(address.trim()).map(Some).map_err(|e| self.error(&e))
    }

    pub(crate) fn sized_sysbus_address(&self, size: u32) -> Result<u32, String> {
        let registration = self
            .registration
            .as_deref()
            .ok_or("missing sysbus registration")?;
        sized_address(registration_point(registration)?, size)
    }

    fn address(&self) -> Result<u32, String> {
        self.sysbus()?
            .ok_or_else(|| self.error("expected a sysbus address"))
    }
}

pub(crate) fn number(value: &str) -> Result<u32, String> {
    let digits = value.replace('_', "");
    match digits.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => digits.parse(),
    }
    .map_err(|_| format!("invalid 32-bit number {value:?}"))
}

pub(crate) enum IrqEnd<'a> {
    Number(u32),
    Range(u32, u32),
    Property(&'a str),
}

pub(crate) fn irq_range(start: u32, end: u32) -> impl Iterator<Item = u32> {
    let mut bounds = start.min(end)..=start.max(end);
    std::iter::from_fn(move || {
        if start <= end {
            bounds.next()
        } else {
            bounds.next_back()
        }
    })
}

pub(crate) fn irq_end(input: &str, source: bool) -> Result<IrqEnd<'_>, String> {
    let input = input.trim();
    if let Some((start, end)) = input.split_once('-') {
        return Ok(IrqEnd::Range(number(start.trim())?, number(end.trim())?));
    }
    if source {
        let mut chars = input.chars();
        if chars.next().is_some_and(char::is_alphabetic)
            && chars.all(|c| c.is_alphanumeric() || c == '_')
        {
            return Ok(IrqEnd::Property(input));
        }
    }
    number(input).map(IrqEnd::Number)
}

pub(crate) fn irq_ends(input: &str, source: bool) -> Result<Vec<IrqEnd<'_>>, String> {
    input
        .trim()
        .strip_prefix('[')
        .and_then(|list| list.strip_suffix(']'))
        .ok_or("expected IRQ list")?
        .split(',')
        .map(|item| irq_end(item, source))
        .collect()
}

#[derive(Default)]
struct ReplComments {
    in_block: bool,
    seen_text: bool,
    braces: i32,
}

impl ReplComments {
    fn first_line(&mut self, raw: &str) -> Result<String, &'static str> {
        let line = self.line(raw)?;
        if !self.seen_text && !line.trim().is_empty() {
            if line.starts_with(char::is_whitespace) {
                return Err("First line with text cannot be indented.");
            }
            self.seen_text = true;
        }
        Ok(line)
    }

    fn line(&mut self, raw: &str) -> Result<String, &'static str> {
        let mut result = String::new();
        let mut rest = raw;
        loop {
            if self.in_block {
                let Some(end) = rest.find("*/") else {
                    break;
                };
                let tail = &rest[end + 2..];
                if self.braces == 0 && !tail.trim().is_empty() {
                    return Err(
                        "Multiline comment in indent mode can only finish at the end of the line.",
                    );
                }
                self.in_block = false;
                if self.braces == 0 {
                    break;
                }
                result.extend(std::iter::repeat_n(
                    ' ',
                    rest[..end + 2].encode_utf16().count(),
                ));
                rest = tail;
            }
            let mut quoted = false;
            let mut chars = rest.char_indices().peekable();
            let mut next = None;
            while let Some((index, ch)) = chars.next() {
                match ch {
                    '\\' if quoted => {
                        chars.next();
                    }
                    '"' => quoted = !quoted,
                    '/' if !quoted && rest[index..].starts_with("/*") => {
                        result.push_str(&rest[..index]);
                        if let Some(end) = rest[index + 2..].find("*/") {
                            let end = index + 4 + end;
                            if self.braces == 0
                                && result.trim().is_empty()
                                && !rest[end..].trim().is_empty()
                            {
                                return Err(
                                    "Single line multiline comment cannot precede code in indent mode.",
                                );
                            }
                            result.extend(std::iter::repeat_n(
                                ' ',
                                rest[index..end].encode_utf16().count(),
                            ));
                            next = Some(&rest[end..]);
                        } else {
                            self.in_block = true;
                        }
                        rest = "";
                        break;
                    }
                    '/' if !quoted
                        && rest[index..].starts_with("//")
                        && (index == 0 || rest[..index].ends_with(char::is_whitespace)) =>
                    {
                        rest = &rest[..index];
                        break;
                    }
                    _ => {}
                }
            }
            if quoted {
                return Err("Unterminated string.");
            }
            result.push_str(rest);
            if let Some(tail) = next {
                rest = tail;
            } else {
                break;
            }
        }
        self.braces += brace_depth(&result);
        Ok(result)
    }
}

fn unquoted_delimiters<'a>(
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

#[derive(Debug, PartialEq, Eq)]
enum RegistrationValue<'a> {
    Range { base: u64, size: u64 },
    String(String),
    Other(&'a str),
}

fn registration_point(item: &str) -> Result<(&str, Option<RegistrationValue<'_>>), &'static str> {
    let mut chars = item.char_indices();
    if !chars.next().is_some_and(|(_, c)| c.is_alphabetic()) {
        return Err("expected a register reference");
    }
    let end = chars
        .find(|&(_, c)| !c.is_alphanumeric() && c != '_')
        .map_or(item.len(), |(index, _)| index);
    let (register, value) = item.split_at(end);
    let value = value.trim();
    let point = match value.as_bytes().first() {
        None => None,
        Some(b'<') => {
            let (base, size) = range_value(value)?;
            Some(RegistrationValue::Range { base, size })
        }
        Some(b'"') => Some(RegistrationValue::String(quoted_value(value)?)),
        Some(_) if value.len() < item.len() - end => Some(RegistrationValue::Other(value)),
        Some(_) => return Err("expected a registration point after register reference"),
    };
    Ok((register, point))
}

fn sized_address(point: (&str, Option<RegistrationValue<'_>>), size: u32) -> Result<u32, String> {
    match point {
        ("sysbus", Some(RegistrationValue::Range { base, size: length })) => {
            if length != u64::from(size) {
                return Err("registration range size disagrees with size property".into());
            }
            u32::try_from(base).map_err(|_| "registration base exceeds 32-bit address space".into())
        }
        ("sysbus", Some(RegistrationValue::Other(address))) => number(address),
        _ => Err("expected an address registered on sysbus".into()),
    }
}

pub(crate) fn range_value(value: &str) -> Result<(u64, u64), &'static str> {
    let (begin, end) = value
        .strip_prefix('<')
        .and_then(|range| range.strip_suffix('>'))
        .and_then(|range| range.split_once(','))
        .ok_or("invalid registration range")?;
    let base = range_number(begin.trim())?;
    let end = end.trim();
    let size = if let Some(length) = end.strip_prefix('+') {
        let length = range_number(length.trim())?;
        base.checked_add(length)
            .ok_or("registration range overflow")?;
        length
    } else {
        range_number(end)?
            .checked_sub(base)
            .ok_or("reversed registration range")?
    };
    Ok((base, size))
}

fn range_number(value: &str) -> Result<u64, &'static str> {
    let value = value.replace('_', "");
    match value.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => value.parse(),
    }
    .map_err(|_| "invalid registration range number")
}

fn registration_points(
    list: &str,
) -> Result<Vec<(&str, Option<RegistrationValue<'_>>)>, &'static str> {
    let mut points = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (end, delimiter) in
        unquoted_delimiters(list, &['{', '}', '[', ']', ';']).chain([(list.len(), ';')])
    {
        match delimiter {
            '{' | '[' => depth += 1,
            '}' | ']' => depth -= 1,
            ';' if depth == 0 => {
                let item = list[start..end].trim();
                if item.is_empty() && end == list.len() && !points.is_empty() {
                    break;
                }
                points.push(registration_point(item)?);
                start = end + 1;
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err("unbalanced registration value");
    }
    Ok(points)
}

fn inline_body<'a>(
    declaration: &'a str,
    source: &SourceLine,
) -> Result<(&'a str, Properties), &'static str> {
    let mut braces = unquoted_delimiters(declaration, &['{', '}']);
    let Some((mut begin, '{')) = braces.next() else {
        return if unquoted_delimiters(declaration, &['}']).next().is_some() {
            Err("unexpected closing brace")
        } else {
            Ok((declaration, Vec::new()))
        };
    };
    if declaration[..begin].trim_end().ends_with('@') {
        let mut depth = 1;
        let end = braces
            .find_map(|(index, brace)| {
                depth += if brace == '{' { 1 } else { -1 };
                (depth == 0).then_some(index)
            })
            .ok_or("expected a closing brace for registrations")?;
        registration_points(&declaration[begin + 1..end])?;
        match braces.next() {
            Some((index, '{')) if declaration[end + 1..index].trim().is_empty() => {
                begin = index;
            }
            None if declaration[end + 1..].trim().is_empty()
                || declaration[end + 1..].trim_start().starts_with("as ") =>
            {
                return Ok((declaration, Vec::new()));
            }
            _ => return Err("unexpected text after registrations"),
        }
    }
    let Some((end, '}')) = braces.next() else {
        return Err("expected a closing brace for inline properties");
    };
    if braces.next().is_some() || !declaration[end + 1..].trim().is_empty() {
        return Err("unexpected text after inline properties");
    }
    let text = &declaration[begin + 1..end];
    let mut body = Vec::new();
    let mut start = 0;
    for end in unquoted_delimiters(text, &[';'])
        .map(|(index, _)| index)
        .chain([text.len()])
    {
        let item = text[start..end].trim();
        if item.is_empty() {
            if end != text.len() {
                return Err("empty inline attribute");
            }
        } else if property_key(item).is_some() || item.contains("->") {
            body.push((source.clone(), item.into()));
        } else {
            return Err("expected an inline property or connection");
        }
        start = end + 1;
    }
    Ok((&declaration[..begin], body))
}

fn parse(
    path: &Path,
    root: &Path,
    visiting: &mut HashSet<PathBuf>,
    entries: &mut Vec<Entry>,
) -> Result<(), String> {
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if visiting.len() >= 64 {
        return Err(format!(
            "{}: excessively deep platform include",
            path.display()
        ));
    }
    if !visiting.insert(canonical.clone()) {
        return Err(format!(
            "{}: recursive or excessively deep platform include: There is a cycle in using file dependency.",
            path.display()
        ));
    }
    let content = fs::read_to_string(&canonical).map_err(|e| format!("{}: {e}", path.display()))?;
    let result = parse_text(
        &content,
        path,
        canonical.parent().unwrap(),
        root,
        visiting,
        entries,
    );
    visiting.remove(&canonical);
    result
}

fn quoted_prefix(text: &str) -> Result<(String, &str), &'static str> {
    let invalid = "expected a quoted string";
    let mut value = String::new();
    let input = text.strip_prefix('"').ok_or(invalid)?;
    let mut characters = input.char_indices();
    while let Some((index, character)) = characters.next() {
        match character {
            '"' => return Ok((value, &input[index + 1..])),
            '\\' => match characters.next() {
                Some((_, escaped @ ('"' | '\\'))) => value.push(escaped),
                _ => return Err("invalid escape in quoted string"),
            },
            character => value.push(character),
        }
    }
    Err(invalid)
}

pub(crate) fn quoted_value(text: &str) -> Result<String, &'static str> {
    let (value, tail) = quoted_prefix(text)?;
    if !tail.trim().is_empty() {
        return Err("unexpected text after quoted string");
    }
    Ok(value)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ListItem<'a> {
    List(Vec<ListItem<'a>>),
    Object(ObjectValue<'a>),
    Number(std::borrow::Cow<'a, str>),
    String(String),
    Symbol(SymbolicValue<'a>),
    Range(u64, u64),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ObjectValue<'a> {
    model: &'a str,
    fields: Vec<(&'a str, &'a str)>,
}

pub(crate) fn object_fields(input: &str) -> Result<Vec<(&str, &str)>, &'static str> {
    let mut fields = Vec::new();
    let mut start = 0;
    for end in unquoted_delimiters(input, &[';'])
        .map(|(index, _)| index)
        .chain([input.len()])
    {
        let text = input[start..end].trim();
        if text.is_empty() && end == input.len() {
            break;
        }
        let name = property_key(text).ok_or("expected REPL object property")?;
        let (_, value) = text.split_once(':').unwrap();
        fields.push((name, value.trim()));
        start = end + 1;
    }
    Ok(fields)
}

fn object_prefix(input: &str) -> Result<(ObjectValue<'_>, &str), &'static str> {
    let input = input
        .strip_prefix("new ")
        .ok_or("expected REPL object")?
        .trim_start();
    let (model, body) = input
        .split_once('{')
        .ok_or("expected REPL object attributes")?;
    let model = model.trim_end();
    if model.is_empty() || model.contains(char::is_whitespace) {
        return Err("expected REPL object type");
    }
    let (end, delimiter) = unquoted_delimiters(body, &['{', '}'])
        .next()
        .ok_or("expected closing brace in REPL object")?;
    if delimiter != '}' {
        return Err("nested REPL object attributes are unsupported");
    }
    Ok((
        ObjectValue {
            model,
            fields: object_fields(&body[..end])?,
        },
        &body[end + 1..],
    ))
}

pub(crate) fn list_value(input: &str) -> Result<Vec<ListItem<'_>>, &'static str> {
    let (items, tail) = list_prefix(input, 0)?;
    if !tail.trim().is_empty() {
        return Err("unexpected text after REPL list");
    }
    Ok(items)
}

fn list_prefix(input: &str, depth: usize) -> Result<(Vec<ListItem<'_>>, &str), &'static str> {
    if depth >= 64 {
        return Err("REPL list nesting exceeds 64 levels");
    }
    let mut input = input
        .strip_prefix('[')
        .ok_or("expected REPL list")?
        .trim_start();
    let mut items = Vec::new();
    loop {
        if let Some(tail) = input.strip_prefix(']') {
            return Ok((items, tail));
        }
        let (item, tail) = match input.as_bytes().first() {
            _ if input.starts_with("new ") => {
                let (object, tail) = object_prefix(input)?;
                (ListItem::Object(object), tail)
            }
            Some(b'[') => {
                let (values, tail) = list_prefix(input, depth + 1)?;
                (ListItem::List(values), tail)
            }
            _ => scalar_prefix(input)?,
        };
        items.push(item);
        let tail = tail.trim_start();
        if let Some(rest) = tail.strip_prefix(',') {
            input = rest.trim_start();
        } else if tail.starts_with(']') {
            input = tail;
        } else {
            return Err("expected comma or closing bracket in REPL list");
        }
    }
}

fn scalar_prefix(input: &str) -> Result<(ListItem<'_>, &str), &'static str> {
    let parsed = match input.as_bytes().first() {
        Some(b'"') => {
            let (value, tail) = quoted_prefix(input)?;
            (ListItem::String(value), tail)
        }
        Some(b'<') => {
            let end = input.find('>').ok_or("expected end of REPL range")? + 1;
            let (base, size) = range_value(&input[..end])?;
            (ListItem::Range(base, size), &input[end..])
        }
        _ => {
            if let Some((value, tail)) = crate::repl_syntax::number_prefix(input)
                && tail.trim_start().starts_with([',', ']'])
            {
                return Ok((ListItem::Number(value), tail));
            }
            let end = input
                .find(|c: char| c.is_whitespace() || c == ',' || c == ']')
                .unwrap_or(input.len());
            let (value, tail) = input.split_at(end);
            let item = if let Some(symbol) = symbolic_value(value) {
                ListItem::Symbol(symbol)
            } else if value.parse::<f64>().is_ok_and(f64::is_finite)
                || value
                    .trim_start_matches(['+', '-'])
                    .strip_prefix("0x")
                    .is_some_and(|digits| {
                        !digits.is_empty() && digits.chars().all(|c| c.is_ascii_hexdigit())
                    })
            {
                ListItem::Number(value.into())
            } else {
                return Err("expected REPL scalar value");
            };
            (item, tail)
        }
    };
    Ok(parsed)
}

fn dictionary_value(value: &str) -> Result<Vec<(&str, &str)>, String> {
    fn unexpected(input: &str) -> String {
        input.chars().next().map_or_else(
            || "Unexpected end of input in dictionary".into(),
            |character| format!("unexpected '{character}' in dictionary"),
        )
    }

    fn token(input: &str) -> Result<(&str, &str), String> {
        if input.starts_with('"') {
            let (_, tail) = quoted_prefix(input).map_err(str::to_owned)?;
            return Ok((&input[..input.len() - tail.len()], tail.trim_start()));
        }
        let end = if input.starts_with('<') {
            input.find('>').map_or(0, |index| index + 1)
        } else {
            input
                .find(|c: char| c.is_whitespace() || ":;,}\"{[".contains(c))
                .unwrap_or(input.len())
        };
        if end == 0 {
            return Err(unexpected(input));
        }
        Ok((&input[..end], input[end..].trim_start()))
    }

    let mut input = value
        .strip_prefix('{')
        .ok_or_else(|| unexpected(value))?
        .trim_start();
    let mut items = Vec::new();
    loop {
        if let Some(tail) = input.strip_prefix('}') {
            if tail.trim().is_empty() {
                return Ok(items);
            }
            return Err(unexpected(tail.trim_start()));
        }
        let (key, tail) = token(input)?;
        let value = tail
            .strip_prefix(':')
            .ok_or_else(|| unexpected(tail))?
            .trim_start();
        let (value, tail) = token(value)?;
        for scalar in [key, value] {
            let (_, tail) = scalar_prefix(scalar).map_err(str::to_owned)?;
            if !tail.is_empty() {
                return Err(unexpected(tail));
            }
        }
        items.push((key, value));
        if let Some(rest) = tail.strip_prefix(';') {
            input = rest.trim_start();
        } else if tail.starts_with('}') {
            input = tail;
        } else {
            return Err(unexpected(tail));
        }
    }
}

fn using_path(line: &str) -> Result<String, &'static str> {
    let invalid = "expected using \"path.repl\"";
    let (path, tail) = quoted_prefix(line.trim()).map_err(|_| invalid)?;
    if path.is_empty() || !tail.chars().all(|c| c == ';' || c.is_whitespace()) {
        return Err(invalid);
    }
    Ok(path)
}

fn resolve_using(argument: &str, include_dir: &Path, root: &Path) -> std::io::Result<PathBuf> {
    let path = Path::new(argument);
    let explicit_relative = matches!(
        path.components().next(),
        Some(Component::CurDir | Component::ParentDir)
    );
    if !path.is_absolute() && !explicit_relative {
        let local = include_dir.join(path);
        return Ok(if local.is_file() {
            local
        } else {
            root.join(path)
        });
    }

    let joined = if path.is_absolute() {
        path.to_owned()
    } else {
        include_dir.join(path)
    };
    let mut resolved = PathBuf::new();
    for component in std::path::absolute(joined)?.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            _ => resolved.push(component),
        }
    }
    Ok(resolved)
}

enum IndentBoundary {
    Open,
    Sibling,
    Close,
}

fn indentation(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn indent_boundary(from: usize, to: usize) -> IndentBoundary {
    match from.cmp(&to) {
        std::cmp::Ordering::Less => IndentBoundary::Open,
        std::cmp::Ordering::Equal => IndentBoundary::Sibling,
        std::cmp::Ordering::Greater => IndentBoundary::Close,
    }
}

fn declaration_alias(declaration: &str) -> Result<(&str, Option<String>), &'static str> {
    let mut depth = 0;
    for (index, delimiter) in unquoted_delimiters(declaration, &['{', '}', ' ', '\t', '\n']) {
        match delimiter {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ if depth == 0 => {
                if let Some(alias) = declaration[index..]
                    .trim_start()
                    .strip_prefix("as")
                    .filter(|tail| tail.starts_with(char::is_whitespace))
                {
                    let alias = quoted_value(alias.trim())?;
                    return Ok((declaration[..index].trim_end(), Some(alias)));
                }
            }
            _ => {}
        }
    }
    Ok((declaration, None))
}

fn parse_text(
    content: &str,
    path: &Path,
    include_dir: &Path,
    root: &Path,
    visiting: &mut HashSet<PathBuf>,
    entries: &mut Vec<Entry>,
) -> Result<(), String> {
    let source_path = Rc::<Path>::from(path);
    let mut current: Option<Entry> = None;
    let mut init = Vec::new();
    let mut init_indent = None;
    let mut definitions = Vec::new();
    let mut seen_usings = HashSet::new();
    let mut comments = ReplComments::default();
    let mut lines = content.lines().enumerate().peekable();
    while let Some((index, raw)) = lines.next() {
        let line = comments
            .first_line(raw)
            .map_err(|error| format!("{}:{}: {error}", path.display(), index + 1))?;
        let text = line.trim();
        let lineno = index + 1;
        let source = SourceLine {
            path: Rc::clone(&source_path),
            line: lineno,
        };
        let error = |detail: &str| format!("{}:{lineno}: {detail}", path.display());
        if text.is_empty() {
            continue;
        }
        let indent = indentation(&line);
        if init_indent
            .is_some_and(|parent| matches!(indent_boundary(parent, indent), IndentBoundary::Open))
        {
            init.push((source, text.into()));
            continue;
        }
        init_indent = None;
        if matches!(text, "init:" | "init add:") && indent != 0 {
            if current.is_none() {
                return Err(error("init section has no declaration"));
            }
            if !init.is_empty() {
                return Err(error("more than one init section in a declaration"));
            }
            init_indent = Some(indent);
            init.push((source, text.into()));
            continue;
        }
        if text == "script: '''"
            && let Some(entry) = current
                .as_mut()
                .filter(|entry| entry.model == "Python.PythonPeripheral")
        {
            let mut property = String::from("script:");
            loop {
                let (_, raw) = lines
                    .next()
                    .ok_or_else(|| error("unterminated Python peripheral script"))?;
                if raw.trim() == "'''" {
                    break;
                }
                property.push('\n');
                property.push_str(raw);
            }
            entry.body.push((source, property));
            continue;
        }
        if line.starts_with(char::is_whitespace) {
            let mut property = text.to_owned();
            if let Some((_, value)) = text.split_once(':')
                && value.trim_start().starts_with('{')
            {
                let mut depth = brace_depth(value);
                while depth > 0 {
                    let Some((index, raw)) = lines.next() else {
                        break;
                    };
                    let continuation = comments
                        .line(raw)
                        .map_err(|error| format!("{}:{}: {error}", path.display(), index + 1))?;
                    depth += brace_depth(&continuation);
                    property.push(' ');
                    property.push_str(continuation.trim());
                }
                let (_, value) = property.split_once(':').unwrap();
                dictionary_value(value.trim()).map_err(|detail| error(&detail))?;
            }
            if let Some((_, value)) = text.split_once(':')
                && let Some(model) = value.trim().strip_prefix("new ")
                && !model.contains(char::is_whitespace)
                && !model.contains('{')
            {
                property.push_str(" {");
                while lines.peek().is_some_and(|(_, raw)| {
                    raw.trim().is_empty()
                        || matches!(
                            indent_boundary(indent, indentation(raw)),
                            IndentBoundary::Open
                        )
                }) {
                    let (index, raw) = lines.next().unwrap();
                    let continuation = comments
                        .line(raw)
                        .map_err(|error| format!("{}:{}: {error}", path.display(), index + 1))?;
                    let continuation = continuation.trim();
                    if !continuation.is_empty() {
                        property.push_str(continuation);
                        property.push(';');
                    }
                }
                property.push('}');
            }
            if let Some((_, value)) = property.split_once(':')
                && value.trim().starts_with("new ")
                && value.contains('{')
            {
                let (_, tail) = object_prefix(value.trim()).map_err(error)?;
                if !tail.trim().is_empty() {
                    return Err(error("unexpected text after REPL object"));
                }
            }
            current
                .as_mut()
                .ok_or_else(|| error("property has no declaration"))?
                .body
                .push((source, property));
            continue;
        }
        if let Some(mut entry) = current.take() {
            entry.body.append(&mut init);
            definitions.push(entry);
        }
        if let Some(include) = text.strip_prefix("using ") {
            let filename = using_path(include).map_err(error)?;
            let include_path = resolve_using(&filename, include_dir, root)
                .map_err(|detail| error(&detail.to_string()))?;
            if !seen_usings.insert(filename.clone()) {
                return Err(error("Duplicate using entry."));
            }
            match parse(&include_path, root, visiting, entries) {
                Err(_) if matches!(include_path.try_exists(), Ok(false)) => {
                    return Err(error(&format!("Using '{filename}' does not exist")));
                }
                Err(detail) => return Err(error(&detail)),
                Ok(()) => {}
            }
            continue;
        }
        let (name, declaration) = text
            .split_once(':')
            .ok_or_else(|| error("expected a platform declaration"))?;
        let name = name.trim();
        let local_name = name
            .strip_prefix("local")
            .filter(|tail| tail.starts_with(char::is_whitespace));
        let name = local_name.map_or(name, str::trim_start);
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(error("unsupported declaration name"));
        }
        let mut block = String::new();
        let mut depth = brace_depth(declaration);
        let declaration = if depth > 0 {
            block.push_str(declaration);
            while depth > 0 {
                let (index, raw) = lines
                    .next()
                    .ok_or_else(|| error("expected a closing brace for inline declaration"))?;
                let line = comments
                    .line(raw)
                    .map_err(|error| format!("{}:{}: {error}", path.display(), index + 1))?;
                depth += brace_depth(&line);
                block.push('\n');
                block.push_str(&line);
            }
            &block
        } else {
            declaration
        };
        let (declaration, body) = inline_body(declaration, &source).map_err(error)?;
        let (declaration, alias) = declaration_alias(declaration).map_err(error)?;
        let (model, registration) = match declaration.split_once('@') {
            Some((model, registration)) => {
                let registration = registration.trim();
                if !registration.starts_with('{') {
                    registration_point(registration).map_err(error)?;
                }
                (
                    model.trim(),
                    Some(
                        registration
                            .lines()
                            .map(str::trim)
                            .collect::<Vec<_>>()
                            .join(" "),
                    ),
                )
            }
            None => (declaration.trim(), None),
        };
        current = Some(Entry {
            name: name.into(),
            model: model.into(),
            local: local_name.is_some(),
            alias,
            registration,
            body,
            source: path.into(),
            line: lineno,
        });
    }
    if comments.in_block {
        return Err(format!("{}: unterminated block comment", path.display()));
    }
    if let Some(mut entry) = current {
        entry.body.append(&mut init);
        definitions.push(entry);
    }
    entries.extend(definitions);
    Ok(())
}

fn brace_depth(text: &str) -> i32 {
    unquoted_delimiters(text, &['{', '}'])
        .map(|(_, brace)| if brace == '{' { 1 } else { -1 })
        .sum()
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SymbolicValue<'a> {
    Boolean(bool),
    Reference(&'a str),
    Enum(Option<&'a str>, &'a str),
}

fn symbolic_value(value: &str) -> Option<SymbolicValue<'_>> {
    fn identifier(text: &str) -> bool {
        let mut chars = text.chars();
        chars.next().is_some_and(char::is_alphabetic)
            && chars.all(|c| c.is_alphanumeric() || c == '_')
    }

    match value.trim() {
        "true" | "True" => Some(SymbolicValue::Boolean(true)),
        "false" | "False" => Some(SymbolicValue::Boolean(false)),
        reference if identifier(reference) => Some(SymbolicValue::Reference(reference)),
        value => {
            let (family, member) = value.split_once('.')?;
            (identifier(member) && (family.is_empty() || identifier(family))).then_some(
                SymbolicValue::Enum((!family.is_empty()).then_some(family), member),
            )
        }
    }
}

pub(crate) fn boolean_value(value: &str) -> Option<bool> {
    match symbolic_value(value)? {
        SymbolicValue::Boolean(value) => Some(value),
        _ => None,
    }
}

fn property_key(text: &str) -> Option<&str> {
    let (key, value) = text.split_once(':')?;
    let key = key.trim();
    let mut characters = key.chars();
    (!value.trim().is_empty()
        && characters.next().is_some_and(char::is_alphabetic)
        && characters.all(|c| c.is_alphanumeric() || c == '_'))
    .then_some(key)
}

fn merge_properties(body: &mut Properties, incoming: Properties) {
    // Keep same-entry duplicate properties for the peripheral validator.
    let mut replaced = HashSet::new();
    for (_, text) in &incoming {
        if let Some(key) = property_key(text)
            && replaced.insert(key)
        {
            let mut matches = body
                .iter()
                .enumerate()
                .filter(|(_, (_, property))| property_key(property) == Some(key));
            if let Some((index, _)) = matches.next()
                && matches.next().is_none()
            {
                body.remove(index);
            }
        }
    }
    let insertion = body
        .iter()
        .position(|(_, text)| text == "init:")
        .unwrap_or(body.len());
    body.splice(insertion..insertion, incoming);
}

fn apply_overlays(entries: Vec<Entry>) -> Vec<Entry> {
    let mut names = HashMap::new();
    for (index, entry) in entries.iter().enumerate() {
        if !entry.model.is_empty() {
            names.entry(entry.name.clone()).or_insert(index);
        }
    }
    let mut bodies: HashMap<usize, Properties> = HashMap::new();
    let mut definitions = Vec::new();
    for (index, mut entry) in entries.into_iter().enumerate() {
        let target = names.get(&entry.name).copied();
        if entry.model.is_empty()
            && entry.registration.is_none()
            && let Some(target) = target
        {
            merge_properties(bodies.entry(target).or_default(), entry.body);
            continue;
        }
        if target == Some(index) {
            merge_properties(
                bodies.entry(index).or_default(),
                std::mem::take(&mut entry.body),
            );
        }
        definitions.push((index, entry));
    }
    definitions
        .into_iter()
        .map(|(index, mut entry)| {
            if let Some(body) = bodies.remove(&index) {
                entry.body = body;
            }
            entry
        })
        .collect()
}

pub(crate) fn load(path: &Path, root: &Path) -> Result<Vec<crate::network_repl::Entry>, String> {
    let mut entries = Vec::new();
    parse(path, root, &mut HashSet::new(), &mut entries)?;
    convert(entries)
}

pub(crate) fn parse_inline(
    content: &str,
    root: &Path,
) -> Result<Vec<crate::network_repl::Entry>, String> {
    let source = Path::new("<inline REPL>");
    let mut entries = Vec::new();
    parse_text(
        content,
        source,
        root,
        root,
        &mut HashSet::new(),
        &mut entries,
    )?;
    convert(entries)
}

fn convert(entries: Vec<Entry>) -> Result<Vec<crate::network_repl::Entry>, String> {
    for entry in &entries {
        validate_entry_properties(&entry.body, |source, key| {
            format!("{source}: {}: duplicate REPL property {key:?}", entry.name)
        })?;
        if entry.local {
            return Err(entry.error("local REPL declaration requires a scope"));
        }
    }
    Ok(apply_overlays(entries)
        .into_iter()
        .map(|entry| {
            crate::network_repl::Entry::from_parsed(
                entry.name,
                entry.model,
                entry.registration,
                entry
                    .body
                    .into_iter()
                    .map(|(source, text)| (source.path, source.line, text))
                    .collect(),
                entry.source,
                entry.line,
            )
        })
        .collect())
}

pub(crate) fn sized_sysbus_address(registration: &str, size: u32) -> Result<u32, String> {
    sized_address(registration_point(registration)?, size)
}
