use roxmltree::{Document, Node};
use std::{collections::BTreeMap, io::Read, path::Path};

#[derive(Clone, Copy, Default)]
struct Settings {
    size: Option<u32>,
    reset: u64,
    readable: Option<bool>,
}

impl Settings {
    fn apply(mut self, node: Node<'_, '_>) -> Result<Self, String> {
        if let Some(size) = optional_number(node, "size")? {
            self.size = Some(u32::try_from(size).map_err(|_| "invalid register size")?);
        }
        if let Some(reset) = optional_number(node, "resetValue")? {
            self.reset = reset;
        }
        if let Some(access) = text(node, "access") {
            self.readable = Some(match access.to_ascii_lowercase().as_str() {
                "read-only" | "read-write" | "read-writeonce" => true,
                "write-only" | "writeonce" => false,
                _ => return Err(format!("invalid access {access}")),
            });
        }
        Ok(self)
    }
}

#[derive(Clone, Debug)]
struct Register {
    name: String,
    address: u64,
    width: u64,
    value: u64,
    readable: bool,
}

#[derive(Clone, Debug)]
pub struct Svd {
    registers: Vec<Register>,
    bytes: BTreeMap<u64, usize>,
}

fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children().find(|child| child.has_tag_name(name))
}

fn text<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    child(node, name).map(|element| element.text().unwrap_or("").trim())
}

fn mandatory<'a>(node: Node<'a, '_>, name: &str) -> Result<&'a str, String> {
    text(node, name).ok_or_else(|| format!("missing {name} in {}", node.tag_name().name()))
}

fn optional_number(node: Node<'_, '_>, name: &str) -> Result<Option<u64>, String> {
    text(node, name)
        .map(|value| {
            if let Some(hex) = value
                .strip_prefix("0x")
                .or_else(|| value.strip_prefix("0X"))
            {
                u64::from_str_radix(hex, 16)
            } else {
                value.parse()
            }
            .map_err(|_| format!("invalid {name}: {value}"))
        })
        .transpose()
}

fn address(node: Node<'_, '_>, key: &str) -> Result<u64, String> {
    optional_number(node, key)?.ok_or_else(|| format!("missing {key}"))
}

fn named_child<'a, 'input>(
    scope: Node<'a, 'input>,
    tag: &str,
    name: &str,
) -> Option<Node<'a, 'input>> {
    scope
        .children()
        .find(|node| node.has_tag_name(tag) && text(*node, "name") == Some(name))
}

fn register_settings(
    node: Node<'_, '_>,
    inherited: Settings,
    visited: &mut Vec<roxmltree::NodeId>,
) -> Result<Settings, String> {
    let Some(reference) = node.attribute("derivedFrom") else {
        return inherited.apply(node);
    };
    if !node.has_tag_name("register") {
        return Err("derived cluster is unsupported".into());
    }
    if visited.contains(&node.id()) {
        return Err(format!("cyclic register derivation: {reference}"));
    }
    visited.push(node.id());
    let mut inherited = inherited;
    let mut parts: Vec<_> = reference.split('.').collect();
    let register = parts.pop().ok_or("empty derived register name")?;
    let missing = || format!("unknown derived register {reference}");
    let scope = if let [peripheral_name, clusters @ ..] = parts.as_slice() {
        let device = node.document().root_element();
        let peripherals = child(device, "peripherals").ok_or_else(missing)?;
        let peripheral =
            named_child(peripherals, "peripheral", peripheral_name).ok_or_else(missing)?;
        inherited = inherited.apply(peripheral)?;
        let mut scope = child(peripheral, "registers").ok_or_else(missing)?;
        for cluster_name in clusters {
            scope = named_child(scope, "cluster", cluster_name).ok_or_else(missing)?;
            inherited = inherited.apply(scope)?;
        }
        scope
    } else {
        node.parent().ok_or_else(missing)?
    };
    let source = named_child(scope, "register", register).ok_or_else(missing)?;
    let settings = register_settings(source, inherited, visited)?.apply(node)?;
    visited.pop();
    Ok(settings)
}

impl Svd {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let data = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut source = String::new();
        if data.starts_with(&[0x1f, 0x8b]) {
            flate2::read::GzDecoder::new(data.as_slice())
                .read_to_string(&mut source)
                .map_err(|error| format!("{}: {error}", path.display()))?;
        } else {
            source =
                String::from_utf8(data).map_err(|error| format!("{}: {error}", path.display()))?;
        }
        Self::parse(&source)
    }

    pub fn parse(source: &str) -> Result<Self, String> {
        let document = Document::parse(source).map_err(|error| error.to_string())?;
        let device = document.root_element();
        if !device.has_tag_name("device") {
            return Err("missing device element".into());
        }
        mandatory(device, "name")?;
        mandatory(device, "description")?;
        let big_endian = if let Some(cpu) = child(device, "cpu") {
            match mandatory(cpu, "endian")? {
                "big" => true,
                "little" | "selectable" | "other" => false,
                _ => return Err("invalid CPU endianness".into()),
            }
        } else {
            false
        };
        let defaults = Settings::default().apply(device)?;
        let peripherals = child(device, "peripherals").ok_or("missing peripherals")?;
        let mut result = Self {
            registers: Vec::new(),
            bytes: BTreeMap::new(),
        };
        for peripheral in peripherals
            .children()
            .filter(|node| node.has_tag_name("peripheral"))
        {
            let name = mandatory(peripheral, "name")?;
            let base = address(peripheral, "baseAddress")?;
            let mut sources = vec![peripheral];
            while let Some(reference) = sources.last().unwrap().attribute("derivedFrom") {
                let source = named_child(peripherals, "peripheral", reference)
                    .ok_or_else(|| format!("unknown derived peripheral {reference}"))?;
                if sources.contains(&source) {
                    return Err(format!("cyclic peripheral derivation: {reference}"));
                }
                sources.push(source);
            }
            let settings = sources
                .iter()
                .rev()
                .try_fold(defaults, |settings, source| settings.apply(*source))?;
            if let Some(registers) = sources
                .into_iter()
                .find_map(|source| child(source, "registers"))
            {
                result.scan(registers, base, settings, big_endian, name)?;
            }
        }
        Ok(result)
    }

    fn scan(
        &mut self,
        scope: Node<'_, '_>,
        base: u64,
        inherited: Settings,
        big_endian: bool,
        prefix: &str,
    ) -> Result<(), String> {
        for node in scope
            .children()
            .filter(|node| node.has_tag_name("register") || node.has_tag_name("cluster"))
        {
            let name = mandatory(node, "name")?;
            let address = base
                .checked_add(address(node, "addressOffset")?)
                .ok_or("address overflow")?;
            let settings = register_settings(node, inherited, &mut Vec::new())?;
            let count = optional_number(node, "dim")?.unwrap_or(1);
            let stride = optional_number(node, "dimIncrement")?.unwrap_or(0);
            if count > 65_536 {
                return Err(format!("SVD dimension is too large for {name}"));
            }
            for index in 0..count {
                let address = index
                    .checked_mul(stride)
                    .and_then(|delta| address.checked_add(delta))
                    .ok_or("address overflow")?;
                let name = format!("{prefix}:{}", name.replace("%s", &index.to_string()));
                if node.has_tag_name("cluster") {
                    self.scan(node, address, settings, big_endian, &name)?;
                } else {
                    let size = settings
                        .size
                        .filter(|size| *size > 0)
                        .ok_or("missing or invalid register size")?;
                    let width = u64::from(size.div_ceil(8));
                    let value = match size {
                        1..=63 => settings.reset & ((1u64 << size) - 1),
                        64 => settings.reset,
                        _ => 0,
                    };
                    let value = if big_endian && width <= 8 {
                        value.swap_bytes() >> (64 - width * 8)
                    } else {
                        value
                    };
                    let end = address
                        .checked_add(width)
                        .ok_or("register interval overflow")?;
                    // CMSIS-SVD reset values are at most 64 bits. Wider registers
                    // have zero reset data and are clipped to the supported width.
                    let end = end.min(address.saturating_add(8));
                    let index = self.registers.len();
                    self.registers.push(Register {
                        name,
                        address,
                        width,
                        value,
                        readable: settings.readable.unwrap_or(true),
                    });
                    for byte in address..end {
                        self.bytes.entry(byte).or_insert(index);
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn contains(&self, address: u32, width: usize) -> bool {
        self.bytes
            .range(u64::from(address)..u64::from(address) + width as u64)
            .next()
            .is_some()
    }

    pub(crate) fn access(&self, address: u32, width: usize, write: Option<u32>) -> Option<u32> {
        let kind = match width {
            1 => "Byte",
            2 => "Word",
            4 => "DoubleWord",
            _ => return None,
        };
        let address = u64::from(address);
        let (_, index) = self.bytes.range(address..address + width as u64).next()?;
        let name = &self.registers[*index].name;
        let value = self.read(address, width).unwrap_or(0) as u32;
        let message = if let Some(value) = write {
            format!(
                "Write{kind} of value 0x{value:X} to an unimplemented register {name} (0x{address:X}) generated from SVD"
            )
        } else {
            format!(
                "Read{kind} from an unimplemented register {name} (0x{address:X}), returning a value from SVD: 0x{value:X}"
            )
        };
        crate::monitor_log::record(2, &message);
        Some(value)
    }

    pub fn read(&self, address: u64, width: usize) -> Option<u64> {
        if !matches!(width, 1 | 2 | 4 | 8) {
            return None;
        }
        if let Some(index) = self.bytes.get(&address) {
            let register = &self.registers[*index];
            if register.address == address && register.width == width as u64 {
                return Some(if register.readable { register.value } else { 0 });
            }
        }
        let mut found = false;
        let mut value = 0;
        for byte in 0..width {
            let offset = address.checked_add(byte as u64)?;
            if let Some(index) = self.bytes.get(&offset) {
                let register = &self.registers[*index];
                if register.readable {
                    found = true;
                    value |= ((register.value >> ((offset - register.address) * 8)) & 0xff)
                        << (byte * 8);
                }
            }
        }
        found.then_some(value)
    }
}
