use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Component, Path, PathBuf},
    rc::Rc,
};

use crate::{
    constant_mmio::ConstantMmio,
    machine::{MAX_HARTS, Machine},
    nvic::IRQ_LINES,
    stm32_uart::Stm32Irq,
    stm32h7_adc_platform,
    stm32h7_exti::H7Exti,
    stm32h7_flash::FlashController,
    stm32h7_gpio_platform, stm32h7_rtc_platform,
};

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
pub(crate) struct Entry {
    pub(crate) name: String,
    pub(crate) model: String,
    local: bool,
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

fn range_value(value: &str) -> Result<(u64, u64), &'static str> {
    let (begin, end) = value[1..]
        .strip_suffix('>')
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
            None if declaration[end + 1..].trim().is_empty() => {
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
    Number(&'a str),
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
                ListItem::Number(value)
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
        if text == "init:" && indent != 0 {
            if current.is_none() || !init.is_empty() {
                return Err(error("expected a single init section in a declaration"));
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

fn configure_sysbus_tags(entry: &Entry, bus: &mut crate::bus::Bus) -> Result<(), String> {
    let Some(((_, init), commands)) = entry.body.split_first() else {
        return Err(entry.error("expected sysbus init"));
    };
    if init != "init:" || entry.registration.is_some() {
        return Err(entry.error("expected sysbus init"));
    }
    for (source, command) in commands {
        let invalid =
            || format!("{source}: expected Tag <start, end> \"name\" [value] [silent=True]");
        let (range, description) = command
            .strip_prefix("Tag <")
            .and_then(|text| text.split_once('>'))
            .ok_or_else(invalid)?;
        let (start, end) = range.split_once(',').ok_or_else(invalid)?;
        let start = number(start.trim()).map_err(|e| format!("{source}: {e}"))?;
        let end = number(end.trim()).map_err(|e| format!("{source}: {e}"))?;
        if start > end {
            return Err(invalid());
        }
        let (_, arguments) = description
            .trim()
            .strip_prefix('"')
            .and_then(|text| text.split_once('"'))
            .ok_or_else(invalid)?;
        let mut arguments: Vec<_> = arguments.split_whitespace().collect();
        if matches!(arguments.last(), Some(&"silent=True" | &"silent=False")) {
            arguments.pop();
        }
        let value = match arguments.as_slice() {
            [] => 0,
            [value] => number(value).map_err(|e| format!("{source}: {e}"))?,
            _ => return Err(invalid()),
        };
        bus.tag(start..=end, value);
    }
    Ok(())
}

pub fn load(path: &Path, root: Option<&Path>) -> Result<Machine, String> {
    let mut entries = Vec::new();
    parse(
        path,
        root.unwrap_or(Path::new(".")),
        &mut HashSet::new(),
        &mut entries,
    )?;
    build_machine(entries, path)
}

pub fn check_text(content: &str, root: Option<&Path>) -> Result<(), String> {
    let mut entries = Vec::new();
    let source = Path::new("<inline REPL>");
    let root = root.unwrap_or(Path::new("."));
    parse_text(
        content,
        source,
        root,
        root,
        &mut HashSet::new(),
        &mut entries,
    )?;
    if !entries.is_empty() {
        build_machine(entries, source)?;
    }
    Ok(())
}

fn build_machine(entries: Vec<Entry>, path: &Path) -> Result<Machine, String> {
    if let Some(entry) = entries.iter().find(|entry| entry.local) {
        return Err(entry.error("file-local peripheral scope is unsupported"));
    }
    let entries = apply_overlays(entries);
    let mut machine = Machine::without_default_ram();
    let mut names = HashSet::new();
    let mut cpus = Vec::new();
    let mut clint = None;
    let mut dwt = None;
    let mut nvic = None;
    let mut exti = None;
    let mut flash_controller = None;
    let mut combiners = Vec::new();
    let mut ram = false;
    let mut uart = None;
    let mut stm32_uarts = Vec::new();
    let mut plic = None;
    let mut timer = None;
    for entry in &entries {
        if !names.insert(&entry.name) {
            return Err(entry.error("duplicate declarations and overrides are unsupported"));
        }
        if entry.model.is_empty() {
            if entry.name == "sysbus" {
                configure_sysbus_tags(entry, &mut machine.bus)?;
                continue;
            }
            let detail = if entry.registration.is_some() {
                "REPL registration overrides are unsupported"
            } else {
                "REPL overlay target has no earlier declaration"
            };
            return Err(entry.error(detail));
        }
        match entry.model.as_str() {
            "CPU.TranslationCPU" => {
                return Err(entry.error(
                    "Antmicro.Renode.Peripherals.CPU.TranslationCPU is an abstract type. Try constructing one of the following concrete types: CPU.CortexM, CPU.RiscV32, CPU.VexRiscv",
                ));
            }
            "CPU.RiscV32" | "CPU.VexRiscv" | "CPU.CortexM" => {
                if entry.sysbus()?.is_some() {
                    return Err(entry.error("expected CPU registered on sysbus without an address"));
                }
                cpus.push(entry);
            }
            "Python.PythonPeripheral" => {
                let peripheral =
                    ConstantMmio::from_platform(entry).map_err(|error| entry.error(&error))?;
                machine
                    .bus
                    .map_constant_mmio(peripheral)
                    .map_err(|error| entry.error(&error))?;
            }
            "Memory.MappedMemory" | "Memory.ArrayMemory" => {
                let init = entry.body.iter().position(|(_, text)| text == "init:");
                let (properties, commands) = init
                    .map_or((entry.body.as_slice(), &[][..]), |index| {
                        (&entry.body[..index], &entry.body[index + 1..])
                    });
                let size = match properties {
                    [(_, text)] => text
                        .split_once(':')
                        .filter(|(key, _)| key.trim() == "size")
                        .map(|(_, value)| value.trim()),
                    _ => None,
                }
                .ok_or_else(|| entry.error("expected one memory size property before init"))?;
                let size = number(size).map_err(|e| entry.error(&e))?;
                let address = if let Some((
                    "sysbus",
                    Some(RegistrationValue::Range { base, size: length }),
                )) = entry
                    .registration
                    .as_deref()
                    .and_then(|registration| registration_point(registration).ok())
                {
                    if length != u64::from(size) {
                        return Err(entry.error("memory range size disagrees with size property"));
                    }
                    u32::try_from(base)
                        .map_err(|_| entry.error("memory base exceeds 32-bit address space"))?
                } else {
                    entry.address()?
                };
                machine
                    .bus
                    .map_ram(address, size as usize)
                    .map_err(|e| entry.error(&e))?;
                for (line, command) in commands {
                    let error = |detail: &str| format!("{line}: {}: {detail}", entry.name);
                    let (offset, width, value) =
                        memory_init(command, address).map_err(|e| error(&e))?;
                    if offset
                        .checked_add(width as u32)
                        .is_none_or(|end| end > size)
                    {
                        return Err(error("memory init write exceeds the declared region"));
                    }
                    let target = address + offset;
                    machine
                        .bus
                        .memory(target, width)
                        .map_err(|e| error(&e.to_string()))?
                        .copy_from_slice(&value.to_le_bytes()[..width]);
                }
                ram = true;
            }
            "GPIOPort.Murax_GPIO" => {
                keys(entry, &[])?;
                machine
                    .bus
                    .map_murax_gpio(entry.address()?)
                    .map_err(|e| entry.error(&e))?;
            }
            "Timers.Murax_Timer" => {
                if timer.replace(entry).is_some() {
                    return Err(entry.error("only one Murax timer is currently supported"));
                }
            }
            "IRQControllers.PlatformLevelInterruptController" => {
                if plic.replace(entry).is_some() {
                    return Err(entry.error("only one PLIC is currently supported"));
                }
            }
            "UART.STM32F7_USART" => stm32_uarts.push(entry),
            "UART.Murax_UART" | "UART.NS16550" | "UART.LiteX_UART" => {
                if uart.replace(entry).is_some() {
                    return Err(entry.error("only one UART is currently supported"));
                }
            }
            "IRQControllers.STM32F4_EXTI" | "IRQControllers.STM32H7_EXTI" => {
                if exti.replace(entry).is_some() {
                    return Err(entry.error("only one STM32 EXTI is supported"));
                }
            }
            "Miscellaneous.CombinedInput" => combiners.push(entry),
            "MTD.STM32H7_FlashController" => {
                if flash_controller.replace(entry).is_some() {
                    return Err(entry.error("only one STM32H7 flash controller is supported"));
                }
            }
            "Miscellaneous.DWT" => {
                if dwt.replace(entry).is_some() {
                    return Err(entry.error("only one DWT is supported"));
                }
            }
            "IRQControllers.NVIC" => {
                if nvic.replace(entry).is_some() {
                    return Err(entry.error("only one NVIC is supported"));
                }
            }
            "IRQControllers.CoreLevelInterruptor" => {
                if clint.replace(entry).is_some() {
                    return Err(entry.error("only one CLINT is currently supported"));
                }
            }
            _ if crate::network_platform::is_network(entry)
                || stm32h7_adc_platform::is_device(&entry.model)
                || stm32h7_gpio_platform::is_device(&entry.model)
                || entry.model == stm32h7_rtc_platform::RTC
                || entry.model == crate::stm32h7_spi::SPI
                || crate::stm32h7_board_platform::is_device(&entry.model) => {}
            _ => return Err(entry.error(&format!("unsupported platform model {:?}", entry.model))),
        }
    }
    if cpus.is_empty() || !ram {
        return Err(format!(
            "{}: platform requires at least one CPU and mapped RAM",
            path.display()
        ));
    }
    if let Some(entry) = cpus.iter().find(|entry| entry.model == "CPU.CortexM") {
        if cpus.len() != 1 || clint.is_some() || plic.is_some() || timer.is_some() {
            return Err(
                entry.error("expected a single Cortex-M CPU without RISC-V interrupt controllers")
            );
        }
        let mut cpu_type = None;
        let mut mpu_regions = None;
        let mut vectors = None;
        for (key, value) in keys(
            entry,
            &[
                "cpuType",
                "vectorTableOffset",
                "InitVectorTableOffset",
                "numberOfMPURegions",
                "nvic",
            ],
        )? {
            match key {
                "cpuType" => cpu_type = Some(value),
                "nvic" if nvic.is_some_and(|controller| controller.name == value) => {}
                "nvic" => return Err(entry.error("unknown NVIC reference")),
                "numberOfMPURegions" => {
                    let count = number(value).map_err(|e| entry.error(&e))?;
                    if count > 16 {
                        return Err(entry.error("invalid MPU region count"));
                    }
                    mpu_regions = Some(count as u8);
                }
                "vectorTableOffset" | "InitVectorTableOffset" if vectors.is_none() => {
                    vectors = Some(number(value).map_err(|e| entry.error(&e))?);
                }
                "vectorTableOffset" | "InitVectorTableOffset" => {
                    return Err(entry.error("duplicate vector table offset"));
                }
                _ => unreachable!(),
            }
        }
        let thumb2 = match cpu_type {
            Some(r#""cortex-m0""# | r#""cortex-m0+""#) => false,
            Some(r#""cortex-m7""#) => true,
            _ => return Err(entry.error("expected Cortex-M0, Cortex-M0+ or Cortex-M7 cpuType")),
        };
        if !thumb2 && mpu_regions.is_some() {
            return Err(entry.error("MPU regions require Cortex-M7"));
        }
        if let Some(dwt) = dwt {
            let (_, frequency) = keys(dwt, &["frequency"])?
                .into_iter()
                .next()
                .ok_or_else(|| dwt.error("missing DWT frequency"))?;
            let frequency = number(frequency).map_err(|error| dwt.error(&error))?;
            if frequency == 0 {
                return Err(dwt.error("DWT frequency must be positive"));
            }
            machine
                .bus
                .map_dwt(dwt.address()?, frequency)
                .map_err(|error| dwt.error(&error))?;
        }
        machine.bus.map_systick().map_err(|e| entry.error(&e))?;
        let mut priority = None;
        let mut frequency = None;
        if let Some(controller) = nvic {
            if controller.address()? != crate::nvic::BASE {
                return Err(controller.error("NVIC must be mapped at 0xe000e000"));
            }
            let expected = format!("->{}@0", entry.name);
            let mut connected = false;
            for (_, text) in &controller.body {
                let wire: String = text.chars().filter(|c| !c.is_whitespace()).collect();
                if !connected && wire.strip_prefix("IRQ").unwrap_or(&wire) == expected {
                    connected = true;
                } else if let Some((key, value)) = text.split_once(':') {
                    let value = number(value.trim()).map_err(|e| controller.error(&e))?;
                    match key.trim() {
                        "priorityMask" if priority.is_none() && value <= 255 => {
                            priority = Some(value as u8)
                        }
                        "systickFrequency" if frequency.is_none() && value > 0 => {
                            frequency = Some(value)
                        }
                        _ => return Err(controller.error("unsupported or duplicate NVIC property")),
                    }
                } else {
                    return Err(controller.error("unsupported NVIC wiring"));
                }
            }
        }
        if let Some(frequency) = frequency {
            machine
                .bus
                .systick
                .as_mut()
                .unwrap()
                .set_frequency(frequency);
        }
        if let Some(flash) = flash_controller {
            configure_h7_flash(flash, &entries, nvic, &mut machine)?;
        }
        let mut uart_irq = None;
        if let Some(uart) = uart {
            if uart.model != "UART.LiteX_UART" {
                return Err(uart.error("unsupported Cortex-M UART model"));
            }
            if let [(_, wire)] = uart.body.as_slice()
                && let Some(controller) = nvic
                && let Some((pin, destination)) = wire.split_once("->")
                && matches!(pin.trim(), "IRQ" | "")
                && let Some((name, irq)) = destination.split_once('@')
                && name.trim() == controller.name
            {
                let irq = number(irq.trim()).map_err(|e| uart.error(&e))?;
                if irq >= IRQ_LINES as u32 {
                    return Err(uart.error("NVIC IRQ must be in 0–239"));
                }
                uart_irq = Some(irq as u8);
            } else if !uart.body.is_empty() {
                return Err(uart.error("expected UART IRQ wired to NVIC"));
            }
            machine
                .bus
                .map_litex_uart(uart.address()?)
                .map_err(|e| uart.error(&e))?;
        }
        let mut combined_routes = HashMap::new();
        for combiner in combiners {
            let mut count = None;
            let mut output = None;
            for (_, text) in &combiner.body {
                if let Some(value) = text.strip_prefix("numberOfInputs:")
                    && count.is_none()
                {
                    count = Some(number(value.trim()).map_err(|e| combiner.error(&e))?);
                } else if let Some(wire) = text.strip_prefix("->")
                    && let Some((target, index)) = wire.split_once('@')
                    && nvic.is_some_and(|controller| controller.name == target.trim())
                    && output.is_none()
                {
                    output =
                        Some(irq_index(index, IRQ_LINES as u32).map_err(|e| combiner.error(&e))?);
                } else {
                    return Err(combiner.error("unsupported combined input property or wiring"));
                }
            }
            match (count, output) {
                (Some(count), Some(output)) if count > 0 => {
                    combined_routes.insert(combiner.name.as_str(), (count, output));
                }
                _ => return Err(combiner.error("expected input count and an NVIC output")),
            }
        }
        let exti_lines = if let Some(controller) = exti {
            if controller.model == "IRQControllers.STM32H7_EXTI" {
                if controller.body.iter().any(|(_, text)| !text.contains("->")) {
                    return Err(controller.error("unsupported STM32H7 EXTI property"));
                }
                let mut outputs = [None; H7Exti::LINES];
                exti_outputs(controller, nvic, &combined_routes, &mut outputs)?;
                machine
                    .bus
                    .map_h7_exti(controller.address()?, outputs)
                    .map_err(|e| controller.error(&e))?;
                H7Exti::LINES as u32
            } else {
                let mut count = None;
                let mut direct = None;
                for (_, text) in &controller.body {
                    if text.contains("->") {
                        continue;
                    }
                    let (key, value) = text
                        .split_once(':')
                        .ok_or_else(|| controller.error("invalid EXTI property"))?;
                    let target = match key.trim() {
                        "numberOfOutputLines" if count.is_none() => &mut count,
                        "firstDirectLine" if direct.is_none() => &mut direct,
                        _ => return Err(controller.error("unsupported or duplicate EXTI property")),
                    };
                    *target = Some(number(value.trim()).map_err(|e| controller.error(&e))?);
                }
                let count = count.unwrap_or(14);
                let direct = direct.unwrap_or(23);
                if !(1..=32).contains(&count) || direct > 32 {
                    return Err(controller.error("EXTI supports 1–32 output lines"));
                }
                let mut outputs = vec![None; count as usize];
                exti_outputs(controller, nvic, &combined_routes, &mut outputs)?;
                if outputs.iter().flatten().any(|&line| line >= 32) {
                    return Err(controller.error("STM32F4 EXTI outputs require NVIC lines 0–31"));
                }
                machine
                    .bus
                    .map_exti(controller.address()?, outputs, direct)
                    .map_err(|e| controller.error(&e))?;
                count
            }
        } else {
            0
        };
        stm32h7_adc_platform::configure(&entries, nvic, exti, exti_lines, &mut machine)?;
        crate::stm32h7_board_platform::configure(&entries, nvic, exti, exti_lines, &mut machine)?;
        stm32h7_gpio_platform::configure(&entries, exti, exti_lines, &mut machine)?;
        stm32h7_rtc_platform::configure(&entries, nvic, exti, exti_lines, &mut machine)?;
        for uart in stm32_uarts {
            let mut irq = None;
            let mut low_power = None;
            let mut frequency = None;
            for (_, text) in &uart.body {
                if let Some((pin, destination)) = text.split_once("->")
                    && matches!(pin.trim(), "IRQ" | "")
                    && let Some((name, line)) = destination.split_once('@')
                    && irq.is_none()
                {
                    let name = name.trim();
                    let route = if nvic.is_some_and(|controller| controller.name == name) {
                        Stm32Irq::Nvic(
                            irq_index(line, IRQ_LINES as u32).map_err(|e| uart.error(&e))?,
                        )
                    } else if exti.is_some_and(|controller| controller.name == name) {
                        Stm32Irq::Exti(irq_index(line, exti_lines).map_err(|e| uart.error(&e))?)
                    } else if let Some(&(pins, output)) = combined_routes.get(name) {
                        irq_index(line, pins).map_err(|e| uart.error(&e))?;
                        Stm32Irq::Nvic(output)
                    } else {
                        return Err(uart.error("unsupported USART IRQ target"));
                    };
                    irq = Some(route);
                } else if let Some((key, value)) = text.split_once(':') {
                    let value = value.trim();
                    match key.trim() {
                        "lowPowerMode" if low_power.is_none() => {
                            low_power = Some(
                                value
                                    .parse::<bool>()
                                    .map_err(|_| uart.error("invalid lowPowerMode"))?,
                            );
                        }
                        "frequency" if frequency.is_none() => {
                            frequency = Some(number(value).map_err(|e| uart.error(&e))?);
                            if frequency == Some(0) {
                                return Err(uart.error("USART frequency must be positive"));
                            }
                        }
                        _ => return Err(uart.error("unsupported or duplicate USART property")),
                    }
                } else {
                    return Err(uart.error("unsupported USART IRQ wiring"));
                }
            }
            let (base, alias) = crate::stm32h7_board_platform::registration(uart, &entries)
                .map_err(|e| uart.error(&e))?;
            let clock = alias
                .map(crate::stm32_uart::Stm32UsartClock::from_alias)
                .transpose()
                .map_err(|e| uart.error(&e))?;
            let device = machine
                .bus
                .map_stm32_usart(base, irq, low_power.unwrap_or(false), clock)
                .map_err(|e| uart.error(&e))?;
            device.name = uart.name.clone();
            device.frequency = frequency;
        }
        machine
            .bus
            .map_nvic(priority.unwrap_or(0xc0), uart_irq)
            .map_err(|e| entry.error(&e))?;
        machine.primary.name = Some(entry.name.clone());
        machine.primary.configure_cortex_m(vectors, thumb2);
        if thumb2 {
            machine
                .bus
                .nvic
                .as_mut()
                .unwrap()
                .configure_mpu(mpu_regions.unwrap_or(16));
        }
        crate::network_platform::configure(
            &entries,
            &mut machine,
            None,
            nvic.map(|entry| entry.name.as_str()),
        )?;
        return Ok(machine);
    }
    if let Some(entry) = entries.iter().find(|entry| {
        stm32h7_adc_platform::is_device(&entry.model)
            || stm32h7_gpio_platform::is_device(&entry.model)
            || entry.model == stm32h7_rtc_platform::RTC
            || entry.model == crate::stm32h7_spi::SPI
            || crate::stm32h7_board_platform::is_device(&entry.model)
    }) {
        return Err(entry.error("STM32H7 peripherals require a Cortex-M CPU"));
    }
    if let Some(flash) = flash_controller {
        return Err(flash.error("STM32H7 flash controller requires a Cortex-M CPU"));
    }
    if let Some(dwt) = dwt {
        return Err(dwt.error("DWT requires a Cortex-M CPU"));
    }
    if let Some(nvic) = nvic {
        return Err(nvic.error("NVIC requires a Cortex-M CPU"));
    }
    if let Some(uart) = stm32_uarts.first() {
        return Err(uart.error("STM32 USART requires a Cortex-M platform"));
    }
    if let Some(controller) = exti.or(combiners.first().copied()) {
        return Err(controller.error("STM32 interrupt routing requires a Cortex-M platform"));
    }
    let mut configs = Vec::new();
    for entry in cpus {
        let mut isa = None;
        let mut id = 0;
        let mut pmp = entry.model == "CPU.RiscV32";
        for (key, value) in keys(
            entry,
            &[
                "cpuType",
                "hartId",
                "timeProvider",
                "privilegedArchitecture",
                "pmpEntryCount",
            ],
        )? {
            match (key, value) {
                ("cpuType", value) => {
                    isa = match value.trim_matches('"') {
                        "rv32i" => Some((false, false, false)),
                        "rv32im" | "rv32im_zicsr_zifencei" => Some((true, false, false)),
                        "rv32ima" | "rv32ima_zicsr_zifencei" => Some((true, true, false)),
                        "rv32imac" | "rv32imac_zicsr_zifencei" => Some((true, true, true)),
                        "rv32imc" | "rv32imc_zicsr_zifencei" => Some((true, false, true)),
                        _ => return Err(entry.error(&format!("unsupported CPU type {value}"))),
                    };
                }
                ("hartId", value) => id = number(value).map_err(|e| entry.error(&e))? as usize,
                ("privilegedArchitecture", "PrivilegedArchitecture.Priv1_10") => {}
                ("timeProvider", name) if clint.is_some_and(|clint| clint.name == name) => {}
                ("pmpEntryCount", "0") => pmp = false,
                ("pmpEntryCount", "16") => pmp = true,
                _ => return Err(entry.error(&format!("unsupported CPU property {key} = {value}"))),
            }
        }
        configs.push((
            id,
            entry,
            isa.ok_or_else(|| entry.error("missing cpuType"))?,
            pmp,
        ));
    }
    configs.sort_unstable_by_key(|config| config.0);
    if configs.len() > MAX_HARTS
        || configs
            .iter()
            .enumerate()
            .any(|(index, config)| index != config.0)
    {
        return Err(format!(
            "{}: hart IDs must be unique and contiguous from zero, up to {MAX_HARTS} harts",
            path.display()
        ));
    }
    machine.configure_harts(configs.len());
    for &(id, entry, (multiply, atomic, compressed), pmp) in &configs {
        let hart = machine.hart_mut(id);
        hart.name = Some(entry.name.clone());
        hart.set_integer_extensions(multiply, atomic, compressed);
        if entry.model == "CPU.VexRiscv" {
            hart.enable_vex_irq_controller();
        }
        hart.enable_user_mode();
        hart.enable_supervisor_mode();
        if pmp {
            hart.enable_pmp();
        }
    }
    let cpu = configs[0].1;
    if let Some(entry) = clint {
        let mut target_count = None;
        let mut frequency = None;
        let mut wires = HashSet::new();
        for (lineno, line) in &entry.body {
            if line.contains("->") {
                let wire: String = line.chars().filter(|c| !c.is_whitespace()).collect();
                if wires.insert(wire) {
                    continue;
                }
            } else if let Some((key, value)) = line.split_once(':') {
                let slot = match key.trim() {
                    "frequency" => Some(&mut frequency),
                    "numberOfTargets" => Some(&mut target_count),
                    _ => None,
                };
                if let Some(slot) = slot
                    && slot.is_none()
                {
                    *slot = Some(number(value.trim()).map_err(|e| entry.error(&e))?);
                    continue;
                }
            }
            return Err(format!(
                "{}:{lineno}: unsupported or duplicate CLINT property",
                entry.source.display()
            ));
        }
        let targets = target_count.unwrap_or(1) as usize;
        if frequency == Some(0) || targets == 0 || targets > configs.len() || wires.len() != targets
        {
            return Err(entry.error("invalid CLINT frequency or target wiring"));
        }
        for &(id, target, _, _) in &configs[..targets] {
            let (software, timer) = if target.model == "CPU.VexRiscv" {
                (101, 100)
            } else {
                (3, 7)
            };
            let expected = format!(
                "[{},{}]->{}@[{software},{timer}]",
                2 * id,
                2 * id + 1,
                target.name
            );
            if !wires.contains(&expected) {
                return Err(entry.error(&format!("expected CLINT connection {expected}")));
            }
        }
        machine
            .bus
            .map_clint(entry.address()?, targets)
            .map_err(|e| entry.error(&e))?;
    }
    let plic_config = plic
        .map(|entry| {
            let mut outputs = [0; 2];
            let mut sources = None;
            let mut contexts = None;
            for (_, line) in &entry.body {
                if let Some((context, target)) = line.split_once("->") {
                    let context = number(context.trim()).map_err(|e| entry.error(&e))? as usize;
                    let (name, line) = target
                        .split_once('@')
                        .ok_or_else(|| entry.error("invalid PLIC connection"))?;
                    let line = number(line.trim()).map_err(|e| entry.error(&e))?;
                    if name.trim() != cpu.name
                        || !matches!(line, 9 | 11)
                        || context >= outputs.len()
                        || outputs[context] != 0
                        || outputs.contains(&(1 << line))
                    {
                        return Err(entry.error("invalid or duplicate PLIC connection"));
                    }
                    outputs[context] = 1 << line;
                } else if let Some((key, value)) = line.split_once(':') {
                    match key.trim() {
                        "numberOfSources" if sources.is_none() => {
                            sources = Some(number(value.trim()).map_err(|e| entry.error(&e))?)
                        }
                        "numberOfContexts" if contexts.is_none() => {
                            contexts = Some(number(value.trim()).map_err(|e| entry.error(&e))?)
                        }
                        _ => return Err(entry.error("unsupported PLIC property")),
                    }
                } else {
                    return Err(entry.error("unsupported PLIC wiring"));
                }
            }
            let count = contexts
                .filter(|count| (1..=2).contains(count))
                .ok_or_else(|| entry.error("PLIC requires one or two contexts"))?
                as usize;
            if outputs[..count].iter().all(|&line| line == 0)
                || outputs[count..].iter().any(|&line| line != 0)
                || cpu.model != "CPU.RiscV32"
            {
                return Err(entry.error("expected PLIC context outputs wired to the RiscV32 CPU"));
            }
            let sources = sources.ok_or_else(|| entry.error("missing PLIC numberOfSources"))?;
            machine
                .bus
                .map_plic(entry.address()?, sources, &outputs[..count])
                .map_err(|e| entry.error(&e))?;
            Ok((entry, sources))
        })
        .transpose()?;
    if let Some(entry) = uart {
        if entry.model == "UART.Murax_UART" {
            let connected = direct_irq(entry, &cpu.name, 11)?;
            machine
                .bus
                .map_uart(entry.address()?, connected)
                .map_err(|e| entry.error(&e))?;
        } else if entry.model == "UART.LiteX_UART" {
            keys(entry, &[])?;
            machine
                .bus
                .map_litex_uart(entry.address()?)
                .map_err(|e| entry.error(&e))?;
        } else {
            let (plic, sources) =
                plic_config.ok_or_else(|| entry.error("NS16550 requires a PLIC"))?;
            let mut source = None;
            let mut wide = None;
            for (_, line) in &entry.body {
                if let Some(target) = line
                    .split_once("->")
                    .filter(|(pin, _)| pin.trim() == "IRQ")
                    .and_then(|(_, target)| target.trim().split_once('@'))
                    .filter(|(name, _)| name.trim() == plic.name)
                    && source.is_none()
                {
                    source = Some(number(target.1.trim()).map_err(|e| entry.error(&e))?);
                } else if let Some(("wideRegisters", value)) = line
                    .split_once(':')
                    .map(|(key, value)| (key.trim(), value.trim()))
                    && wide.is_none()
                {
                    wide = Some(
                        value
                            .parse::<bool>()
                            .map_err(|_| entry.error("invalid wideRegisters value"))?,
                    );
                } else {
                    return Err(entry.error("unsupported NS16550 property or wiring"));
                }
            }
            let source = source
                .filter(|source| (1..=sources).contains(source))
                .ok_or_else(|| entry.error("expected a valid NS16550 IRQ connection to PLIC"))?;
            machine
                .bus
                .map_ns16550(entry.address()?, source, wide.unwrap_or(false))
                .map_err(|e| entry.error(&e))?;
        }
    }
    if let Some(entry) = timer {
        let connected = direct_irq(entry, &cpu.name, 7)?;
        machine
            .bus
            .map_murax_timer(entry.address()?, connected)
            .map_err(|e| entry.error(&e))?;
    }
    let vex_cpu = (cpu.model == "CPU.VexRiscv").then_some(cpu.name.as_str());
    crate::network_platform::configure(&entries, &mut machine, vex_cpu, None)?;
    Ok(machine)
}

fn configure_h7_flash(
    flash: &Entry,
    entries: &[Entry],
    nvic: Option<&Entry>,
    machine: &mut Machine,
) -> Result<(), String> {
    let mut banks = [None, None];
    let mut irq = None;
    for (_, text) in &flash.body {
        if let Some((pin, target)) = text.split_once("->")
            && pin.trim().is_empty()
            && let Some((name, line)) = target.split_once('@')
            && nvic.is_some_and(|nvic| nvic.name == name.trim())
            && irq.is_none()
        {
            irq = Some(irq_index(line, IRQ_LINES as u32).map_err(|error| flash.error(&error))?);
            continue;
        }
        let (key, name) = text
            .split_once(':')
            .ok_or_else(|| flash.error("invalid flash controller property or IRQ"))?;
        let bank = match key.trim() {
            "flash1" if banks[0].is_none() => &mut banks[0],
            "flash2" if banks[1].is_none() => &mut banks[1],
            _ => return Err(flash.error("unsupported or duplicate flash bank")),
        };
        let entry = entries
            .iter()
            .find(|entry| entry.name == name.trim() && entry.model == "Memory.MappedMemory")
            .ok_or_else(|| flash.error("flash bank must reference mapped memory"))?;
        let start = entry.address()?;
        let (_, size) = keys(entry, &["size"])?
            .into_iter()
            .next()
            .ok_or_else(|| entry.error("missing memory size"))?;
        let size = number(size).map_err(|error| entry.error(&error))?;
        let end = start
            .checked_add(size)
            .ok_or_else(|| entry.error("flash bank range overflow"))?;
        *bank = Some(start..end);
    }
    let [Some(first), Some(second)] = banks else {
        return Err(flash.error("expected two flash banks"));
    };
    if first == second {
        return Err(flash.error("flash banks must reference distinct memory regions"));
    }
    let irq = irq.ok_or_else(|| flash.error("expected flash interrupt connected to NVIC"))?;
    machine
        .bus
        .map_h7_flash(FlashController::new(flash.address()?, irq, [first, second]))
        .map_err(|error| flash.error(&error))
}

fn exti_outputs(
    controller: &Entry,
    nvic: Option<&Entry>,
    combined_routes: &HashMap<&str, (u32, u8)>,
    outputs: &mut [Option<u8>],
) -> Result<(), String> {
    for (_, text) in &controller.body {
        let Some((from, to)) = text.split_once("->") else {
            continue;
        };
        let mut destinations = irq_destinations(to);
        for destination in destinations.by_ref().take(1) {
            let (target, pins) = destination.map_err(|error| controller.error(error))?;
            let direct_nvic = nvic.is_some_and(|nvic| nvic.name == target.trim());
            let combined = combined_routes.get(target.trim());
            let pin_count = if direct_nvic {
                IRQ_LINES as u32
            } else if let Some((count, _)) = combined {
                *count
            } else {
                return Err(controller.error("unknown EXTI output target"));
            };
            let from = irq_list(from, outputs.len() as u32).map_err(|e| controller.error(&e))?;
            let to = irq_list(pins, pin_count).map_err(|e| controller.error(&e))?;
            if from.len() != to.len() {
                return Err(controller.error("mismatched EXTI wiring widths"));
            }
            for (line, pin) in from.into_iter().zip(to) {
                let output = combined.map_or(pin, |&(_, output)| output);
                if outputs[usize::from(line)].replace(output).is_some() {
                    return Err(controller.error("duplicate EXTI output"));
                }
            }
        }
        if destinations.next().is_some() {
            return Err(controller.error("multiple EXTI destinations are unsupported"));
        }
    }
    Ok(())
}

pub(crate) fn irq_index(value: &str, count: u32) -> Result<u8, String> {
    let index = number(value.trim())?;
    if index >= count || index > u32::from(u8::MAX) {
        return Err(format!("IRQ index {index} is out of range"));
    }
    Ok(index as u8)
}

pub(crate) fn irq_list(value: &str, count: u32) -> Result<Vec<u8>, String> {
    Ok(irq_end_groups(value, count, |name| {
        Err(format!("invalid numeric IRQ {name:?}"))
    })?
    .into_iter()
    .flatten()
    .collect())
}

fn irq_end_groups<T: From<u8>>(
    value: &str,
    count: u32,
    named: impl Fn(&str) -> Result<T, String>,
) -> Result<Vec<Vec<T>>, String> {
    let value = value.trim();
    let mut ends = Vec::new();
    let (list, grouped) = value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .map_or((value, false), |list| (list, true));
    for item in list.split(',') {
        let item = item.trim();
        if grouped && let Some((start, end)) = item.split_once('-') {
            let start = irq_index(start, count)?;
            let end = irq_index(end, count)?;
            if start > end {
                return Err("descending IRQ range".into());
            }
            ends.push((start..=end).map(T::from).collect());
        } else if item.starts_with(|c: char| c.is_ascii_digit()) {
            ends.push(vec![T::from(irq_index(item, count)?)]);
        } else {
            ends.push(vec![named(item)?]);
        }
    }
    if !grouped && ends.len() != 1 {
        return Err("expected a single IRQ".into());
    }
    Ok(ends)
}

fn irq_destinations(text: &str) -> impl Iterator<Item = Result<(&str, &str), &'static str>> {
    text.split('|')
        .map(crate::stm32h7_board_platform::wire_destination)
}

pub(crate) fn irq_receiver(target: &str) -> Result<(&str, Option<u32>), String> {
    let Some((name, index)) = target.split_once('#') else {
        return Ok((target.trim(), None));
    };
    let name = name.trim();
    if name.is_empty() {
        return Err("missing local IRQ receiver name".into());
    }
    Ok((name, Some(number(index.trim())?)))
}

fn direct_irq(entry: &Entry, cpu: &str, line: u32) -> Result<bool, String> {
    let expected = format!("IRQ->{cpu}@{line}");
    match entry.body.as_slice() {
        [] => Ok(false),
        [(_, wiring)]
            if wiring
                .chars()
                .filter(|c| !c.is_whitespace())
                .eq(expected.chars()) =>
        {
            Ok(true)
        }
        _ => Err(entry.error(&format!("expected direct interrupt wiring {expected}"))),
    }
}

fn memory_init(command: &str, base: u32) -> Result<(u32, usize, u32), String> {
    const RANDOM_WORD: &str = r#"`py "print emulationManager.Instance.CurrentEmulation.RandomGenerator.NextUlong() & 0xffffffff"`"#;
    let (operation, arguments) = command
        .split_once(char::is_whitespace)
        .ok_or("expected a memory init write")?;
    let width = match operation {
        "WriteWord" => 2,
        "WriteDoubleWord" => 4,
        _ => return Err(format!("unsupported memory init command {operation:?}")),
    };
    let (offset, value) = arguments
        .trim_start()
        .split_once(char::is_whitespace)
        .ok_or("expected a memory init offset and value")?;
    let offset = number(offset)?;
    let value = match value.trim() {
        RANDOM_WORD if width == 4 => {
            // Mix the destination into a stable simulated ID without a Python runtime.
            let mut word = (u64::from(base) << 32) | u64::from(offset);
            word = word.wrapping_add(0x9e37_79b9_7f4a_7c15);
            word = (word ^ (word >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            word = (word ^ (word >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            (word ^ (word >> 31)) as u32
        }
        value => number(value)?,
    };
    if width == 2 && value > u32::from(u16::MAX) {
        return Err("memory init value exceeds 16 bits".into());
    }
    Ok((offset, width, value))
}

fn keys<'a>(entry: &'a Entry, allowed: &[&str]) -> Result<Vec<(&'a str, &'a str)>, String> {
    let mut seen = HashSet::new();
    entry
        .body
        .iter()
        .map(|(line, text)| {
            let pair = text
                .split_once(':')
                .map(|(key, value)| (key.trim(), value.trim()));
            pair.filter(|(key, _)| allowed.contains(key) && seen.insert(*key))
                .ok_or_else(|| format!("{line}: unknown or duplicate property {text:?}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_overlays_merge_includes_and_keep_duplicate_diagnostics() {
        let directory = std::env::temp_dir().join(format!("penv-overlay-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let base = directory.join("base.repl");
        let overlay = directory.join("overlay.repl");
        fs::write(
            &base,
            r#"cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m7"
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
exti: IRQControllers.STM32H7_EXTI @ sysbus 0x58000000
    0 -> nvic@6
ram: Memory.MappedMemory @ sysbus 0x20000000
    size: 4
"#,
        )
        .unwrap();
        let valid = r#"using "base.repl"
exti:
    1 -> nvic@7
ram: { size: 16 }
ram:
    size: 32
"#;
        fs::write(&overlay, valid).unwrap();
        let mut machine = load(&overlay, None).unwrap();
        assert_eq!(machine.bus.read_ram::<4>(0x2000_001c).unwrap(), 0);
        assert!(machine.bus.read_ram::<4>(0x2000_0020).is_err());
        machine
            .bus
            .write(0x5800_0080, 4, 3, &mut std::io::sink())
            .unwrap();
        machine
            .bus
            .write(0x5800_0008, 4, 3, &mut std::io::sink())
            .unwrap();
        machine
            .bus
            .sample_nvic(&crate::nvic::ActiveExceptions::default());
        assert_eq!(
            machine.bus.read::<4>(0xe000_e200).unwrap(),
            (1 << 6) | (1 << 7)
        );

        for (addition, expected) in [
            (
                "ram: Memory.MappedMemory @ sysbus 0x20001000\n    size: 16\n",
                "duplicate declarations and overrides are unsupported",
            ),
            (
                "missing:\n    size: 4\n",
                "REPL overlay target has no earlier declaration",
            ),
            (
                "nvic:\n    systickFrequency: 1\n    systickFrequency: 2\n",
                "unsupported or duplicate NVIC property",
            ),
        ] {
            fs::write(&overlay, format!("{valid}{addition}")).unwrap();
            let error = load(&overlay, None).err().unwrap();
            assert!(error.contains(expected), "{error}");
        }
        fs::remove_file(&base).unwrap();
        fs::remove_file(&overlay).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn memory_init_loads_system_words_and_rejects_cross_region_writes() {
        let path =
            std::env::temp_dir().join(format!("penv-memory-init-{}.repl", std::process::id()));
        let repl = r#"cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m0"
system_flash_ro: Memory.MappedMemory @ sysbus 0x1ff00000
    size: 0x20000
    init:
        WriteDoubleWord 0x1e800 `py "print emulationManager.Instance.CurrentEmulation.RandomGenerator.NextUlong() & 0xffffffff"`
        WriteDoubleWord 0x1e804 `py "print emulationManager.Instance.CurrentEmulation.RandomGenerator.NextUlong() & 0xffffffff"`
        WriteWord 0x1e880 0x800
"#;
        fs::write(&path, repl).unwrap();
        let first = load(&path, None).unwrap();
        let second = load(&path, None).unwrap();
        let uid = |machine: &Machine, offset: u32| {
            machine.bus.read_ram::<4>(0x1ff0_0000 + offset).unwrap()
        };
        assert_eq!(uid(&first, 0x1e800), uid(&second, 0x1e800));
        assert_ne!(uid(&first, 0x1e800), uid(&first, 0x1e804));
        assert_eq!(first.bus.read_ram::<2>(0x1ff1_e880).unwrap(), 0x800);
        assert_eq!(first.bus.read_ram::<2>(0x1ff1_e882).unwrap(), 0);

        fs::write(&path, repl.replace("0x1e880 0x800", "0x1ffff 0x800")).unwrap();
        let result = load(&path, None);
        fs::remove_file(&path).unwrap();
        assert!(
            result
                .err()
                .unwrap()
                .contains("memory init write exceeds the declared region")
        );
        assert!(memory_init("WriteWord 0 0x10000", 0).is_err());
        assert!(memory_init("WriteDoubleWord 0 `py \"print 1\"`", 0).is_err());
    }
}

#[cfg(test)]
mod using_cases {
    use super::*;

    fn parse_using_fixture(name: &str, source: &str, files: &[(&str, &str)]) -> String {
        let directory =
            std::env::temp_dir().join(format!("penv-using-{}-{name}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        for (name, content) in files {
            fs::write(directory.join(name), content).unwrap();
        }
        let result = check_text(source, Some(&directory));
        fs::remove_dir_all(directory).unwrap();
        result.unwrap_err()
    }

    #[test]
    fn should_fail_on_duplicate_using_entry() {
        let source = r#"
using "A"
using "B"
using "A"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
"#;
        let a = r#"
cpu:
    Placeholder: "set by A first"
cpu:
    Placeholder: "set by A second"
"#;
        let b = r#"
cpu:
    Placeholder: "set by B first"
cpu:
    Placeholder: "set by B second"
"#;
        assert!(
            parse_using_fixture("duplicate", source, &[("A", a), ("B", b)])
                .contains("Duplicate using entry.")
        );
    }

    #[test]
    fn should_fail_on_recurring_usings() {
        let source = "
using \"B\"\np3: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral";
        let a = "
using \"B\"\np2: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral";
        let b = "
using \"A\"\np: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral";
        assert!(
            parse_using_fixture("recursive", source, &[("A", a), ("B", b)])
                .contains("There is a cycle in using file dependency")
        );
    }

    #[test]
    fn should_fail_on_directly_recurring_usings() {
        let source = "
using \"A\"\np2: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral";
        let a = "
using \"A\"\np: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral";
        assert!(
            parse_using_fixture("self", source, &[("A", a)])
                .contains("There is a cycle in using file dependency")
        );
    }

    #[test]
    fn should_fail_on_non_existing_using_file() {
        assert!(
            parse_using_fixture("missing", "\nusing \"A\"", &[])
                .contains("Using 'A' does not exist")
        );
    }

    fn check_override(index: u8, source: &str, files: &[(&str, &str)], expected: &str) {
        let directory =
            std::env::temp_dir().join(format!("penv-using-{}-order-{index}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        for (name, content) in files {
            fs::write(directory.join(name), content).unwrap();
        }
        let mut entries = Vec::new();
        let result = parse_text(
            source,
            &directory.join("root"),
            &directory,
            &directory,
            &mut HashSet::new(),
            &mut entries,
        );
        fs::remove_dir_all(directory).unwrap();
        result.unwrap();
        let definitions = apply_overlays(entries);
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].name, "cpu");
        assert_eq!(definitions[0].body.len(), 1);
        assert_eq!(
            definitions[0].body[0].1,
            format!("Placeholder: \"{expected}\"")
        );
    }

    #[test]
    fn should_follow_attribute_override_order_0() {
        check_override(
            0,
            r#"
using "A"
using "B"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus 0x0
    Placeholder: "set by source first"

cpu:
    Placeholder: "set by source second"
"#,
            &[
                (
                    "A",
                    r#"
cpu:
    Placeholder: "set by A"
"#,
                ),
                (
                    "B",
                    r#"
using "C"
cpu:
    Placeholder: "set by B"
"#,
                ),
                (
                    "C",
                    r#"
cpu:
    Placeholder: "set by C"
"#,
                ),
            ],
            "set by source second",
        );
    }

    #[test]
    fn should_follow_attribute_override_order_1() {
        check_override(
            1,
            r#"
using "A"
using "B"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
"#,
            &[
                (
                    "A",
                    r#"
cpu:
    Placeholder: "set by A first"
cpu:
    Placeholder: "set by A second"
"#,
                ),
                (
                    "B",
                    r#"
using "C"
cpu:
    Placeholder: "set by B first"
cpu:
    Placeholder: "set by B second"
"#,
                ),
                (
                    "C",
                    r#"
cpu:
    Placeholder: "set by C first"
cpu:
    Placeholder: "set by C second"
"#,
                ),
            ],
            "set by B second",
        );
    }

    #[test]
    fn should_follow_attribute_override_order_2() {
        check_override(
            2,
            r#"
using "A"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus 0x0
"#,
            &[
                (
                    "A",
                    r#"
using "B"
cpu:
    Placeholder: "set by A"
"#,
                ),
                (
                    "B",
                    r#"
cpu:
    Placeholder: "set by B"
"#,
                ),
            ],
            "set by A",
        );
    }

    #[test]
    fn should_follow_attribute_override_order_3() {
        check_override(
            3,
            r#"
using "A"
using "B"
using "C"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
"#,
            &[
                (
                    "A",
                    r#"
cpu:
    Placeholder: "set by A first"
cpu:
    Placeholder: "set by A second"
"#,
                ),
                (
                    "B",
                    r#"
cpu:
    Placeholder: "set by B first"
cpu:
    Placeholder: "set by B second"
"#,
                ),
                (
                    "C",
                    r#"
using "A";
"#,
                ),
            ],
            "set by A second",
        );
    }

    #[test]
    fn should_follow_attribute_override_order_4() {
        check_override(
            4,
            r#"
using "A"
using "B"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
"#,
            &[
                (
                    "A",
                    r#"
cpu:
    Placeholder: "set by A first"
cpu:
    Placeholder: "set by A second"
"#,
                ),
                (
                    "B",
                    r#"
using "A"
cpu:
    Placeholder: "set by B first"
cpu:
    Placeholder: "set by B second"
"#,
                ),
            ],
            "set by B second",
        );
    }

    #[cfg(unix)]
    mod unix {
        use super::*;

        fn resolve(argument: &str, including: &str) -> PathBuf {
            resolve_using(
                argument,
                Path::new(including).parent().unwrap(),
                Path::new("."),
            )
            .unwrap()
        }

        #[test]
        fn should_handle_absolute_path_unix() {
            assert_eq!(
                resolve("/tmp/platform.repl", "/tmp/includer.repl"),
                Path::new("/tmp/platform.repl")
            );
        }

        #[test]
        fn should_handle_relative_path_in_same_directory_unix() {
            assert_eq!(
                resolve("./platform.repl", "/tmp/includer.repl"),
                Path::new("/tmp/platform.repl")
            );
        }

        #[test]
        fn should_handle_relative_path_in_parent_directory_unix() {
            assert_eq!(
                resolve("../abc/platform.repl", "/tmp/includer.repl"),
                Path::new("/abc/platform.repl")
            );
        }

        #[test]
        fn backslash_should_not_be_path_separator_on_unix() {
            assert_eq!(
                resolve(r"./plat\form.repl", "/tmp/includer.repl"),
                Path::new(r"/tmp/plat\form.repl")
            );
        }
    }
}

#[cfg(test)]
mod comment_cases {
    use super::*;

    fn lines(source: &str) -> Result<Vec<String>, &'static str> {
        let mut comments = ReplComments::default();
        source.lines().map(|line| comments.line(line)).collect()
    }

    // Render the implicit boundaries used by the parser in the format of
    // the fixed upstream prelexer fixtures.
    fn render_boundaries(source: &str) -> String {
        let mut lexer = ReplComments::default();
        let lines = source
            .split('\n')
            .map(|raw| {
                let line = lexer.first_line(raw).unwrap();
                (line, lexer.braces)
            })
            .collect::<Vec<_>>();
        let mut rendered = Vec::new();
        let mut from = 0;
        for (index, (line, braces)) in lines.iter().enumerate() {
            let mut line = line.clone();
            if !line.trim().is_empty() {
                let next = lines[index + 1..]
                    .iter()
                    .find(|(line, _)| !line.trim().is_empty());
                if *braces <= 0 {
                    let to = next.map_or(0, |(line, _)| indentation(line) / 4);
                    match indent_boundary(from, to) {
                        IndentBoundary::Open => line.extend(std::iter::repeat_n('{', to - from)),
                        IndentBoundary::Close => {
                            line.extend(std::iter::repeat_n('}', from - to));
                            if next.is_some() {
                                line.push(';');
                            }
                        }
                        IndentBoundary::Sibling if next.is_some() => line.push(';'),
                        IndentBoundary::Sibling => {}
                    }
                    from = to;
                }
            }
            rendered.push(line);
        }
        rendered.join("\n")
    }

    #[test]
    fn upstream_process_simple_file() {
        assert_eq!(
            render_boundaries(
                "first line\nsecond line\n    first indented\n    second indented\nthird line"
            ),
            "first line;\nsecond line{\n    first indented;\n    second indented};\nthird line"
        );
    }

    #[test]
    fn upstream_process_double_dedent() {
        assert_eq!(
            render_boundaries(
                "first line\nsecond line\n    first indented\n    second indented\n\nthird line"
            ),
            "first line;\nsecond line{\n    first indented;\n    second indented};\n\nthird line"
        );
    }

    #[test]
    fn upstream_process_double_dedent_at_eof() {
        assert_eq!(
            render_boundaries("first line\nsecond line\n    first indented"),
            "first line;\nsecond line{\n    first indented}"
        );
    }

    #[test]
    fn upstream_process_two_lines_no_indent() {
        assert_eq!(render_boundaries("\nline1\nline2"), "\nline1;\nline2");
    }

    #[test]
    fn upstream_process_two_lines_with_separation() {
        assert_eq!(render_boundaries("\nline1\n\nline2"), "\nline1;\n\nline2");
    }

    #[test]
    fn upstream_handle_empty_lines_at_eof() {
        assert_eq!(
            render_boundaries("\nline1\nline2\n\n"),
            "\nline1;\nline2\n\n"
        );
    }

    #[test]
    fn upstream_preserve_indent_in_braces() {
        let source = "\nline1 { \n    line2 }";
        assert_eq!(render_boundaries(source), source);
    }

    #[test]
    fn upstream_prelexer_empty_file() {
        let mut lexer = ReplComments::default();
        assert_eq!(lexer.first_line("").unwrap(), "");
    }

    #[test]
    fn upstream_prelexer_text_on_first_line() {
        let mut lexer = ReplComments::default();
        assert_eq!(lexer.first_line("onlyLine").unwrap(), "onlyLine");
    }

    #[test]
    fn upstream_prelexer_rejects_initial_indent() {
        let source = "\n    first line\n    second line\n        first indented\n        second indented\n    third line";
        let mut lexer = ReplComments::default();
        let outcome: Result<Vec<_>, _> =
            source.lines().map(|line| lexer.first_line(line)).collect();
        assert_eq!(outcome, Err("First line with text cannot be indented."));
    }

    #[test]
    fn upstream_correct_line_comments() {
        assert_eq!(
            lines("\nline1 { // something\n    line2 }").unwrap(),
            ["", "line1 { ", "    line2 }"]
        );
    }

    #[test]
    fn upstream_unrecognized_line_comments() {
        assert_eq!(
            lines("\nline1 {// something\n    line2 }").unwrap(),
            ["", "line1 {// something", "    line2 }"]
        );
    }

    #[test]
    fn upstream_beginning_line_comment() {
        assert_eq!(
            lines("\n// something\nline1 {\n    line2 }").unwrap(),
            ["", "", "line1 {", "    line2 }"]
        );
    }

    #[test]
    fn upstream_line_comments_and_strings() {
        let source = "\nline1 \"something with //\" \"another // pseudo comment\" { // and here goes real comment\n    line2 }";
        assert_eq!(
            lines(source).unwrap(),
            [
                "",
                "line1 \"something with //\" \"another // pseudo comment\" { ",
                "    line2 }"
            ]
        );
    }

    #[test]
    fn upstream_same_line_block_comments() {
        let source = "\nline1 used as a ruler1234            123456789                1234\nline 2/* first comment*/ \"string with /*\" /* second comment */ something";
        assert_eq!(
            lines(source).unwrap(),
            [
                "",
                "line1 used as a ruler1234            123456789                1234",
                "line 2                   \"string with /*\"                      something"
            ]
        );
    }

    #[test]
    fn upstream_multiline_comments() {
        let source =
            "\nline1/* here we begin\n    here it goes\nmore\nmore\n    here we finish*/\nline2";
        assert_eq!(
            lines(source).unwrap(),
            ["", "line1", "", "", "", "", "line2"]
        );
    }

    #[test]
    fn upstream_multiline_comments_within_braces() {
        let source =
            "\nline1 { /* here we begin\n    here it goes\nhere the comment ends*/ x: 5 }\nline2";
        assert_eq!(
            lines(source).unwrap(),
            [
                "",
                "line1 { ",
                "",
                "                        x: 5 }",
                "line2"
            ]
        );
    }

    #[test]
    fn upstream_brace_in_string() {
        let source = "\nline1 \"{ \\\" {\"\nline2";
        let mut comments = ReplComments::default();
        let output: Vec<_> = source
            .lines()
            .map(|line| comments.line(line).unwrap())
            .collect();
        assert_eq!(output, ["", "line1 \"{ \\\" {\"", "line2"]);
        assert_eq!(comments.braces, 0);
    }

    #[test]
    fn comment_columns_follow_utf16() {
        assert_eq!(lines("name/* 🦀 */value").unwrap(), ["name        value"]);
    }

    #[test]
    fn should_fail_on_unterminated_string() {
        assert_eq!(
            lines("\nline1 \"i'm unterminated"),
            Err("Unterminated string.")
        );
    }

    #[test]
    fn should_fail_if_the_multiline_comment_finishes_before_end_of_line() {
        let source =
            "\nline1 /* here we begin\n    here it goes\nhere the comment ends*/ x: 5\nline2";
        assert_eq!(
            lines(source),
            Err("Multiline comment in indent mode can only finish at the end of the line.")
        );
    }

    #[test]
    fn should_fail_on_single_line_multiline_comment() {
        assert_eq!(
            lines("\nfirst line\n    /*something*/ second line"),
            Err("Single line multiline comment cannot precede code in indent mode.")
        );
    }

    #[test]
    fn keeps_strings_and_brace_members_when_filtering_block_comments() {
        let source = "uart: Type { /* begin\n */ name: \"/* quoted */\"; /* gap */ field: 1 }";
        assert_eq!(
            lines(source).unwrap(),
            [
                "uart: Type { ",
                "    name: \"/* quoted */\";           field: 1 }"
            ]
        );
    }
}

#[cfg(test)]
mod inline_attribute_cases {
    use super::*;

    fn parse(source: &str) -> Result<Vec<Entry>, String> {
        let mut entries = Vec::new();
        let path = Path::new("fixture.repl");
        parse_text(
            source,
            path,
            Path::new("."),
            Path::new("."),
            &mut HashSet::new(),
            &mut entries,
        )?;
        Ok(entries)
    }

    #[test]
    fn upstream_quoted_properties() {
        let source = r#"
uart:
    string: "\\escaped backslash: \\, two in a row: \\\\, before quote: \\\", and one at the end: \\"
"#;
        let entries = parse(source).unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.name, "uart");
        assert!(entry.model.is_empty());
        let [(_, property)] = entry.body.as_slice() else {
            panic!("expected one attribute");
        };
        let (name, value) = property.split_once(':').unwrap();
        assert_eq!(name, "string");
        assert_eq!(
            quoted_value(value.trim()).unwrap(),
            r#"\escaped backslash: \, two in a row: \\, before quote: \", and one at the end: \"#
        );

        let entries =
            parse("\nuart:\n    friendlyName: \"some name\"\n    size: 0x1000\n").unwrap();
        assert_eq!(entries.len(), 1);
        let [(_, friendly_name), (_, size)] = entries[0].body.as_slice() else {
            panic!("expected two attributes");
        };
        assert_eq!(
            quoted_value(friendly_name.split_once(':').unwrap().1.trim()).unwrap(),
            "some name"
        );
        assert_eq!(size, "size: 0x1000");
    }

    #[test]
    fn upstream_using_paths() {
        for (source, expected) in [
            (
                "\nusing \"file.pl8\"\nusing \"other_file.pl8\"",
                &["file.pl8", "other_file.pl8"][..],
            ),
            (
                "\nusing \"file.pl8\";\nusing \"other_file.pl8\"",
                &["file.pl8", "other_file.pl8"],
            ),
            ("\nusing \"file.pl8\"", &["file.pl8"]),
            ("\nusing \"file.pl8\";", &["file.pl8"]),
            (
                "\nusing \"first.pl8\";\nusing \"second.pl8\";\nusing \"third.pl8\";",
                &["first.pl8", "second.pl8", "third.pl8"],
            ),
        ] {
            let paths = source
                .lines()
                .filter(|line| !line.is_empty())
                .map(|line| using_path(line.strip_prefix("using ").unwrap()).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(paths, expected);
        }
        assert_eq!(
            using_path(r#""directory\\name.repl"; ;"#).unwrap(),
            r"directory\name.repl"
        );
        assert!(using_path(r#""file.pl8" unexpected"#).is_err());
    }

    #[test]
    fn upstream_indented_init_commands() {
        for (source, expected) in [
            (
                "\nuart:\n    init:\n        DoSomething 3\n",
                &["DoSomething 3"][..],
            ),
            (
                "\nuart:\n    init:\n        Method1 a b\n        Method2 true\n        Method3",
                &["Method1 a b", "Method2 true", "Method3"],
            ),
            (
                "\nuart:\n    init:\n        CallMethod \"string ; with semicolon\"\n",
                &["CallMethod \"string ; with semicolon\""],
            ),
        ] {
            let entries = parse(source).unwrap();
            assert_eq!(entries.len(), 1);
            let (header, commands) = entries[0].body.split_first().unwrap();
            assert_eq!(header.1, "init:");
            assert!(
                commands
                    .iter()
                    .map(|(_, line)| line.as_str())
                    .eq(expected.iter().copied())
            );
        }
    }

    #[test]
    fn upstream_single_registration_points() {
        for (source, point) in [
            ("\nuart: @ sysbus", None),
            (
                "\nuart: @ sysbus <0x2000A000, +0x1000>",
                Some(RegistrationValue::Range {
                    base: 0x2000a000,
                    size: 0x1000,
                }),
            ),
            (
                "\nuart: @ sysbus \"something\"",
                Some(RegistrationValue::String("something".into())),
            ),
        ] {
            let entries = parse(source).unwrap();
            assert_eq!(entries.len(), 1);
            let entry = &entries[0];
            assert_eq!(entry.name, "uart");
            assert!(entry.model.is_empty());
            assert_eq!(
                registration_point(entry.registration.as_deref().unwrap()).unwrap(),
                ("sysbus", point)
            );
        }
    }

    #[test]
    fn upstream_registration_lists() {
        for source in [
            "\nuart: @{ sysbus 0x100 }",
            "\nuart: @{ sysbus 0x100; }",
            "\nuart: @{ sysbus 0x100;\n         sysbus 0x200;}",
        ] {
            assert!(parse(source).is_ok(), "{source}");
        }

        let entries = parse("\nuart: @{ sysbus 0x100;\n         sysbus 0x200}").unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.name, "uart");
        assert!(entry.model.is_empty());
        let list = entry.registration.as_deref().unwrap();
        assert_eq!(
            registration_points(&list[1..list.len() - 1]).unwrap(),
            [
                ("sysbus", Some(RegistrationValue::Other("0x100"))),
                ("sysbus", Some(RegistrationValue::Other("0x200")))
            ]
        );

        for source in [
            "\nuart: @{ _nonIdentifier }",
            "\nuart: @{ _nonIdentifier; }",
            "\nuart: @{ ;_nonIdentifier }",
            "\nuart: @{\n    sysbus 0x100;\n    sysbus 0x200;\n    _nonIdentifier\n}",
        ] {
            assert!(parse(source).is_err(), "{source}");
        }
    }

    #[test]
    fn one_attribute_with_or_without_semicolon() {
        for source in [
            "\nuart: {baudRate: BaudRate.B9600}\n",
            "\nuart: {baudRate: BaudRate.B9600;}\n",
        ] {
            let entries = parse(source).unwrap();
            assert_eq!(entries.len(), 1);
            let entry = &entries[0];
            assert_eq!(entry.name, "uart");
            assert!(entry.model.is_empty());
            assert_eq!(entry.body.len(), 1);
            let (name, variant) = entry.body[0].1.split_once(':').unwrap();
            assert_eq!(name, "baudRate");
            assert_eq!(variant.trim().strip_prefix("BaudRate."), Some("B9600"));
        }
    }

    #[test]
    fn two_attributes_with_or_without_final_semicolon() {
        for source in [
            "\nuart: {\n    friendlyName: \"some name\";\n    size: 0x1000\n}",
            "\nuart: {\n    friendlyName: \"some name\";\n    size: 0x1000;\n}",
        ] {
            let entries = parse(source).unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].body.len(), 2);
        }
    }

    #[test]
    fn upstream_parse_entry_with_all_attributes() {
        let entries = parse(
            r#"
uart:
    model: "ABC666"
    numberOfBits: 32
    sleepTime: 0.01
    friendUart: otherUart
    range: <0x1000, +0x100>
"#,
        )
        .unwrap();
        let [entry] = entries.as_slice() else {
            panic!("expected one entry")
        };
        let attributes = entry
            .body
            .iter()
            .map(|(_, line)| {
                let (name, value) = line.split_once(':').unwrap();
                let (value, tail) = scalar_prefix(value.trim()).unwrap();
                assert!(tail.is_empty());
                (name, value)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            attributes,
            [
                ("model", ListItem::String("ABC666".into())),
                ("numberOfBits", ListItem::Number("32")),
                ("sleepTime", ListItem::Number("0.01")),
                (
                    "friendUart",
                    ListItem::Symbol(SymbolicValue::Reference("otherUart"))
                ),
                ("range", ListItem::Range(0x1000, 0x100)),
            ]
        );
    }

    #[test]
    fn upstream_parse_example() {
        let entries = parse(
            r#"
screen: Display@graphicsCard1
    resolution: new Point { x: 5; y: 6 }
    refreshMode: Automatic
    model: "SuperDisplay"
    -> ic@3
    init:
        DrawCircle 20 20
        DrawCircle 30 40

screen:
    init:
        DrawRect 40 50 1

screen:
    init:
        base init
        DrawRect 40 50 1

other: Display @ sysbus <0x0, +0x100> { refreshMode: Automatic; -> ic@3 }
"#,
        )
        .unwrap();
        assert_eq!(entries.len(), 4);
    }

    #[test]
    fn upstream_parse_using_and_entry() {
        let source = "\nusing \"other_file\"\n\ndevice: SomeDevice";
        let directory =
            std::env::temp_dir().join(format!("penv-using-and-entry-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("other_file"), "").unwrap();
        let mut entries = Vec::new();
        let parsed = parse_text(
            source,
            &directory.join("main.repl"),
            &directory,
            &directory,
            &mut HashSet::new(),
            &mut entries,
        );
        fs::remove_dir_all(&directory).unwrap();
        parsed.unwrap();
        let [entry] = entries.as_slice() else {
            panic!("expected one entry")
        };
        assert_eq!(
            using_path(
                source
                    .lines()
                    .nth(1)
                    .unwrap()
                    .strip_prefix("using ")
                    .unwrap()
            )
            .unwrap(),
            "other_file"
        );
        assert_eq!(
            (entry.name.as_str(), entry.model.as_str()),
            ("device", "SomeDevice")
        );
    }

    #[test]
    fn upstream_parse_object_value() {
        let entries = parse(
            "\ndisplay: Display\n    resolution: new Point\n        x: 640\n        y: 480\n",
        )
        .unwrap();
        let [entry] = entries.as_slice() else {
            panic!("expected one entry")
        };
        let [(_, property)] = entry.body.as_slice() else {
            panic!("expected one property")
        };
        let (_, value) = property.split_once(':').unwrap();
        let (object, tail) = object_prefix(value.trim()).unwrap();
        assert!(tail.is_empty());
        assert_eq!(object.model, "Point");
        assert_eq!(object.fields, [("x", "640"), ("y", "480")]);
    }

    #[test]
    fn upstream_parse_list_with_inline_object() {
        let entries = parse("\ndisplay: Display\n    resolutions: [new Point { x: 640; y: 480 }, new Point { x: 5120; y: 2160 }]\n").unwrap();
        let [entry] = entries.as_slice() else {
            panic!("expected one entry")
        };
        let [(_, property)] = entry.body.as_slice() else {
            panic!("expected one property")
        };
        let values = list_fixture(property);
        assert_eq!(
            values,
            [
                ListItem::Object(ObjectValue {
                    model: "Point",
                    fields: vec![("x", "640"), ("y", "480")]
                }),
                ListItem::Object(ObjectValue {
                    model: "Point",
                    fields: vec![("x", "5120"), ("y", "2160")]
                }),
            ]
        );
    }

    fn list_fixture(property: &str) -> Vec<ListItem<'_>> {
        let (_, value) = property.split_once(':').unwrap();
        list_value(value.trim()).unwrap()
    }

    fn assert_list_fixture(property: &str, expected: &[ListItem<'_>]) {
        let source = format!("\ndevice: Something\n    {property}");
        let entries = parse(&source).unwrap();
        let [entry] = entries.as_slice() else {
            panic!("expected one entry")
        };
        let [(_, property)] = entry.body.as_slice() else {
            panic!("expected one property")
        };
        assert_eq!(list_fixture(property), expected);
    }

    #[test]
    fn upstream_parse_empty_list() {
        assert_list_fixture("list: []", &[]);
    }

    #[test]
    fn upstream_parse_list_of_numbers_with_trailing_comma() {
        use ListItem::*;
        assert_list_fixture(
            "list: [1, 0x10, 3,]",
            &[Number("1"), Number("0x10"), Number("3")],
        );
    }

    #[test]
    fn upstream_parse_nested_lists() {
        use ListItem::*;
        assert_list_fixture(
            "eye: [[1, 0, 0], [0, 1, 0], [0, 0, 1]]",
            &[
                List(vec![Number("1"), Number("0"), Number("0")]),
                List(vec![Number("0"), Number("1"), Number("0")]),
                List(vec![Number("0"), Number("0"), Number("1")]),
            ],
        );
    }

    #[test]
    fn upstream_parse_list_with_mixed_simple_values() {
        use ListItem::*;
        assert_list_fixture(
            r#"list: [1, "hello", true, other, <0, +10>]"#,
            &[
                Number("1"),
                String("hello".into()),
                Symbol(SymbolicValue::Boolean(true)),
                Symbol(SymbolicValue::Reference("other")),
                Range(0, 10),
            ],
        );
    }

    fn dictionary_fixture(value: &str) -> Result<Vec<Entry>, String> {
        parse(&format!("\ndevice: Something\n    dict: {value}"))
    }

    fn assert_dictionary(value: &str, expected: &[(&str, &str)]) {
        let entries = dictionary_fixture(value).unwrap();
        let [entry] = entries.as_slice() else {
            panic!("expected one entry")
        };
        let [(_, property)] = entry.body.as_slice() else {
            panic!("expected one attribute")
        };
        let (_, value) = property.split_once(':').unwrap();
        let pairs = dictionary_value(value.trim()).unwrap();
        let decoded = pairs
            .iter()
            .map(|(key, value)| (quoted_value(key).unwrap(), *value))
            .collect::<Vec<_>>();
        assert!(
            decoded
                .iter()
                .map(|(key, value)| (key.as_str(), *value))
                .eq(expected.iter().copied())
        );
    }

    #[test]
    fn upstream_parse_empty_dict() {
        assert_dictionary("{}", &[]);
    }

    #[test]
    fn upstream_parse_simple_dict() {
        assert_dictionary(r#"{"key0": 0; "key1": 1}"#, &[("key0", "0"), ("key1", "1")]);
    }

    #[test]
    fn upstream_parse_dict_with_trailing_separator() {
        assert_dictionary(
            r#"{"key0": 0; "key1": 1;}"#,
            &[("key0", "0"), ("key1", "1")],
        );
    }

    #[test]
    fn upstream_reject_dict_with_missing_closing_brace() {
        assert!(
            dictionary_fixture(r#"{"key0": 0; "key1": 1"#)
                .unwrap_err()
                .contains("Unexpected end of input")
        );
    }

    #[test]
    fn upstream_reject_dict_with_comma() {
        assert!(
            dictionary_fixture(r#"{"key0": 0, "key1": 1}"#)
                .unwrap_err()
                .contains("unexpected ','")
        );
    }

    #[test]
    fn upstream_reject_dict_with_missing_separator() {
        assert!(
            dictionary_fixture(r#"{"key0": 0 "key1": 1}"#)
                .unwrap_err()
                .contains("unexpected '\"'")
        );
    }

    #[test]
    fn upstream_reject_dict_with_missing_colon() {
        assert!(
            dictionary_fixture(r#"{"key0" 0, "key1": 1}"#)
                .unwrap_err()
                .contains("unexpected '0'")
        );
    }

    #[test]
    fn upstream_parse_empty_file() {
        assert!(parse("").unwrap().is_empty());
    }

    #[test]
    fn upstream_parse_simple_entry() {
        let entries = parse("\nuart: UART").unwrap();
        let [entry] = entries.as_slice() else {
            panic!("expected one entry")
        };
        assert_eq!((&*entry.name, &*entry.model), ("uart", "UART"));
    }

    #[test]
    fn upstream_parse_two_entries() {
        let source = r#"
uart1: UART @ sysbus <0x1000, +0x100>
    someProperty: someValue

uart2: UART @ sysbus <0x2000, +0x100>
    otherProperty: "otherValue"
"#;
        assert_eq!(parse(source).unwrap().len(), 2);
    }

    #[test]
    fn upstream_parse_two_short_entries() {
        assert_eq!(
            parse("\nuart: UART\n\nuart: @sysbus <0x100, +0x100>\n")
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn upstream_parse_local_and_nonlocal_entry() {
        let entries = parse("\nperipheral: SomeHub\nlocal other: Other").unwrap();
        assert_eq!(
            entries.iter().map(|entry| entry.local).collect::<Vec<_>>(),
            [false, true]
        );
    }

    #[test]
    fn upstream_parse_mixed_line_endings() {
        let source =
            "peripheral: SomeHub\nlocal first_other: Other\r\nlocal second_other: Other\r\n";
        assert_eq!(parse(source).unwrap().len(), 3);
    }

    #[test]
    fn upstream_boolean_properties() {
        let entries = parse("\ndevice: Something @ somewhere\n    BoolProp: true\n    BoolProp2: false\n    BoolProp3: True\n    BoolProp4: False\n    BoolProp5: tRue\n    BoolProp6: FALSE\n").unwrap();
        assert_eq!(entries.len(), 1);
        let values = entries[0]
            .body
            .iter()
            .map(|(_, property)| {
                let value = property.split_once(':').unwrap().1.trim();
                symbolic_value(value).unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            values,
            [
                SymbolicValue::Boolean(true),
                SymbolicValue::Boolean(false),
                SymbolicValue::Boolean(true),
                SymbolicValue::Boolean(false),
                SymbolicValue::Reference("tRue"),
                SymbolicValue::Reference("FALSE"),
            ]
        );
    }

    #[test]
    fn upstream_enum_properties() {
        for (source, family) in [
            ("\nuart:\n    baudRate: BaudRate.B9600\n", Some("BaudRate")),
            ("\nuart:\n    baudRate: .B9600\n", None),
        ] {
            let entries = parse(source).unwrap();
            let [entry] = entries.as_slice() else {
                panic!("expected one entry")
            };
            let [(_, property)] = entry.body.as_slice() else {
                panic!("expected one attribute")
            };
            let (name, value) = property.split_once(':').unwrap();
            assert_eq!(entry.name, "uart");
            assert!(entry.model.is_empty());
            assert_eq!(name, "baudRate");
            assert_eq!(
                symbolic_value(value),
                Some(SymbolicValue::Enum(family, "B9600"))
            );
        }
    }

    #[test]
    fn upstream_simple_irq_receivers() {
        let fixtures: &[(&str, &[&str], &[&[(&str, Option<u32>, u8)]])] = &[
            (
                "\nuart:\n    1 -> pic@2\n    IRQ -> pic@3\n    -> pic@4",
                &["1", "IRQ", ""],
                &[
                    &[("pic", None, 2)],
                    &[("pic", None, 3)],
                    &[("pic", None, 4)],
                ],
            ),
            (
                "\nuart:\n    1 -> pic@2 | pic1@3\n    -> pic2@4 | pic3@5\n    IRQ -> pic4@6 | pic5@7 | pic6@8 | pic7@9",
                &["1", "", "IRQ"],
                &[
                    &[("pic", None, 2), ("pic1", None, 3)],
                    &[("pic2", None, 4), ("pic3", None, 5)],
                    &[
                        ("pic4", None, 6),
                        ("pic5", None, 7),
                        ("pic6", None, 8),
                        ("pic7", None, 9),
                    ],
                ],
            ),
            (
                "\nuart:\n    1 -> something#3@2",
                &["1"],
                &[&[("something", Some(3), 2)]],
            ),
        ];
        for &(source, expected_sources, expected_targets) in fixtures {
            let entries = parse(source).unwrap();
            assert_eq!(entries.len(), 1);
            let body = &entries[0].body;
            assert_eq!(body.len(), expected_sources.len());
            for ((_, wire), (expected_source, expected_targets)) in body
                .iter()
                .zip(expected_sources.iter().zip(expected_targets))
            {
                let (from, to) = wire.split_once("->").unwrap();
                assert_eq!(from.trim(), *expected_source);
                if from.trim() == "1" {
                    assert_eq!(irq_list(from, 256).unwrap(), [1]);
                }
                let targets = irq_destinations(to)
                    .map(|target| {
                        let (receiver, pins) = target.unwrap();
                        let (name, index) = irq_receiver(receiver).unwrap();
                        (name, index, irq_index(pins, 256).unwrap())
                    })
                    .collect::<Vec<_>>();
                assert_eq!(&targets, expected_targets);
            }
        }
    }

    #[test]
    fn upstream_irq_ranges_and_multiplexing() {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        enum Source {
            Index(u8),
            Named,
        }
        impl From<u8> for Source {
            fn from(index: u8) -> Self {
                Self::Index(index)
            }
        }

        for (source, expected_sources, expected_targets) in [
            (
                "\nuart:\n    [1-3, IRQ] -> pic @ [2-5]",
                &[
                    Source::Index(1),
                    Source::Index(2),
                    Source::Index(3),
                    Source::Named,
                ][..],
                &[("pic", &[&[2, 3, 4, 5][..]][..])][..],
            ),
            (
                "\nuart:\n    [1-2, IRQ] -> pic0@[4-6] | pic1@[7-8, 9] | pic2@[10, 11, 12]",
                &[Source::Index(1), Source::Index(2), Source::Named],
                &[
                    ("pic0", &[&[4, 5, 6]]),
                    ("pic1", &[&[7, 8], &[9]]),
                    ("pic2", &[&[10], &[11], &[12]]),
                ],
            ),
        ] {
            let entries = parse(source).unwrap();
            assert_eq!(entries.len(), 1);
            let [(_, wiring)] = entries[0].body.as_slice() else {
                panic!("expected one IRQ attribute");
            };
            let (from, to) = wiring.split_once("->").unwrap();
            let sources = irq_end_groups(from, 256, |name| {
                assert_eq!(name, "IRQ");
                Ok(Source::Named)
            })
            .unwrap();
            assert!(
                sources
                    .into_iter()
                    .flatten()
                    .eq(expected_sources.iter().cloned())
            );
            let targets = irq_destinations(to)
                .map(|target| {
                    let (name, pins) = target.unwrap();
                    (
                        name,
                        irq_end_groups::<u8>(pins, 256, |name| {
                            Err(format!("unexpected IRQ {name}"))
                        })
                        .unwrap(),
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(targets.len(), expected_targets.len());
            for ((name, pins), (expected_name, expected_pins)) in
                targets.iter().zip(expected_targets)
            {
                assert_eq!(name, expected_name);
                assert_eq!(
                    pins.iter().map(Vec::as_slice).collect::<Vec<_>>(),
                    *expected_pins
                );
            }
        }
    }

    #[test]
    fn upstream_empty_and_mixed_inline_attributes() {
        assert!(parse("\nuart: {}").unwrap()[0].body.is_empty());
        let entries =
            parse("\nother: Display @ sysbus <0x0, +0x100> { refreshMode: Automatic; -> ic@3; }\n")
                .unwrap();
        assert_eq!(entries.len(), 1);
        assert!(
            entries[0]
                .body
                .iter()
                .map(|(_, text)| text.as_str())
                .eq(["refreshMode: Automatic", "-> ic@3"])
        );
        for source in [
            "\nuart: {;}",
            "\nuart: {_nonIdentifier}",
            "\nuart: {;_nonIdentifier}",
            "\nuart: {\n    friendlyName: \"some name\";\n    size: 0x1000;\n    _nonIdentifier\n}",
            "\nuart: { _invalid: 1 }",
        ] {
            assert!(parse(source).is_err(), "{source}");
        }
    }

    #[test]
    fn rejects_invalid_upstream_inline_entries() {
        for source in [
            "\nuart: {baudRate: BaudRate.B9600;;}\n",
            "\nuart: {_nonIdentifier;;}\n",
            "\nuart: {; baudRate: BaudRate.B9600;}\n",
            "\nuart: {\n    friendlyName: \"some name\";\n    _nonIdentifier\n}",
            "\nuart: {\n    friendlyName: \"some name\";\n    size: 0x1000;\n};",
        ] {
            assert!(parse(source).is_err(), "{source}");
        }
    }
}
