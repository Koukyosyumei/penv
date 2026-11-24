use std::collections::BTreeMap;
use std::io::Read;

#[derive(Debug, PartialEq, Eq)]
pub struct Register {
    pub name: String,
    pub width: u32,
    pub value: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub struct RegisterGroup {
    pub name: String,
    pub width: u32,
    pub index_value_map: BTreeMap<u32, i64>,
}

#[derive(Default)]
pub struct RegisterEnum {
    pub registers: Vec<Register>,
    pub groups: Vec<RegisterGroup>,
}

impl RegisterEnum {
    pub fn parse(mut input: impl Read, defines: &[&str]) -> Result<Self, String> {
        let mut source = String::new();
        input
            .read_to_string(&mut source)
            .map_err(|e| e.to_string())?;
        let mut active = vec![true];
        let mut filtered = String::new();
        for line in source.lines() {
            let line = line.split("//").next().unwrap_or("").trim();
            let mut words = line.split_whitespace();
            match words.next() {
                Some("#ifdef" | "#ifndef") => {
                    let name = words.next().ok_or("missing conditional symbol")?;
                    let defined = defines.contains(&name);
                    active.push(
                        active.last() == Some(&true) && (defined != line.starts_with("#ifndef")),
                    );
                }
                Some("#endif") if active.len() > 1 => {
                    active.pop();
                }
                Some("#endif") => return Err("unmatched #endif".into()),
                Some(_) if active.last() == Some(&true) => {
                    filtered.push_str(line);
                    filtered.push('\n');
                }
                _ => {}
            }
        }
        if active.len() != 1 {
            return Err("unclosed conditional".into());
        }

        let (header, rest) = filtered.split_once('{').ok_or("missing enum body")?;
        let (body, tail) = rest.split_once('}').ok_or("unclosed enum body")?;
        if !header.split_whitespace().any(|word| word == "enum")
            || tail.trim().trim_end_matches(';').trim() != "Registers"
        {
            return Err("expected Registers enum".into());
        }
        let mut result = Self::default();
        let mut values = BTreeMap::new();
        for entry in body
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
        {
            let (identifier, expression) = entry.split_once('=').ok_or("missing register value")?;
            let identifier = identifier.trim();
            let expression = expression.trim();
            let value = if let Some(value) = values.get(expression) {
                *value
            } else if let Some(hex) = expression
                .strip_prefix("0x")
                .or_else(|| expression.strip_prefix("0X"))
            {
                i64::from_str_radix(hex, 16).map_err(|_| format!("invalid value {expression}"))?
            } else {
                expression
                    .parse()
                    .map_err(|_| format!("invalid value {expression}"))?
            };
            let (name, width) = identifier
                .rsplit_once('_')
                .ok_or("missing register width")?;
            let width = width
                .parse()
                .map_err(|_| format!("invalid width in {identifier}"))?;
            values.insert(identifier, value);
            if let Some((name, index)) = name
                .rsplit_once('_')
                .and_then(|(name, index)| index.parse::<u32>().ok().map(|index| (name, index)))
            {
                result.insert_index(name, width, index, value)?;
            } else {
                result.registers.push(Register {
                    name: name.into(),
                    width,
                    value,
                });
            }
        }
        Ok(result)
    }

    fn insert_index(
        &mut self,
        name: &str,
        width: u32,
        index: u32,
        value: i64,
    ) -> Result<(), String> {
        if let Some(group) = self.groups.iter_mut().find(|group| group.name == name) {
            if group.width != width || group.index_value_map.contains_key(&index) {
                return Err(format!("inconsistent indexed register {name}_{index}"));
            }
            group.index_value_map.insert(index, value);
        } else {
            self.groups.push(RegisterGroup {
                name: name.into(),
                width,
                index_value_map: BTreeMap::from([(index, value)]),
            });
        }
        Ok(())
    }
}
