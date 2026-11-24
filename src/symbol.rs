use crate::address_range::{AddressRange, RangeError};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SymbolType {
    File,
    Object,
    Section,
    ProcessorSpecific,
    NotSpecified,
    Function,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SymbolBinding {
    Weak,
    ProcessorSpecific,
    Local,
    Global,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolType,
    pub binding: SymbolBinding,
    range: AddressRange,
    end: u64,
}

impl Symbol {
    /// The end address is exclusive. A zero-length symbol occupies its start address.
    pub fn new(start: u64, end: u64, name: impl Into<String>) -> Result<Self, RangeError> {
        Self::with_metadata(
            start,
            end,
            name,
            SymbolType::NotSpecified,
            SymbolBinding::Global,
        )
    }

    pub fn with_metadata(
        start: u64,
        end: u64,
        name: impl Into<String>,
        kind: SymbolType,
        binding: SymbolBinding,
    ) -> Result<Self, RangeError> {
        let size = end.checked_sub(start).ok_or(RangeError::InvalidEnd)?;
        let range = AddressRange::by(start, size.max(1))?;
        Ok(Self {
            name: demangle_symbol(&name.into()),
            kind,
            binding,
            range,
            end,
        })
    }

    pub fn start(&self) -> u64 {
        self.range.start
    }

    pub fn end(&self) -> u64 {
        self.end
    }

    pub fn contains_address(&self, address: u64) -> bool {
        self.range.contains(address)
    }

    pub fn overlaps(&self, other: &Self) -> bool {
        self.range.intersect(other.range).is_some()
    }

    pub fn contains(&self, other: &Self) -> bool {
        self.range.contains_range(other.range)
    }
}

/// Drop linkage suffixes and parameter lists from Itanium C++ symbol names.
pub fn demangle_symbol(name: &str) -> String {
    if name.starts_with("_GLOBAL__sub_I") {
        if let Some(initializer) = name.get(21..).filter(|rest| !rest.is_empty()) {
            return format!("static initializers for {}", demangle_symbol(initializer));
        }
    }
    if !name.starts_with("_Z") {
        return name.into();
    }
    let mangled = name.strip_suffix("@plt").unwrap_or(name);
    let output = std::process::Command::new("c++filt")
        .args([
            "--no-strip-underscore",
            "--no-params",
            "--format=gnu-v3",
            "--",
            mangled,
        ])
        .output();
    let Ok(output) = output else {
        return name.into();
    };
    if !output.status.success() {
        return name.into();
    }
    let Ok(decoded) = std::str::from_utf8(&output.stdout) else {
        return name.into();
    };
    let decoded = decoded.trim_end_matches(['\n', '\r']);
    if decoded.is_empty() || decoded == mangled {
        return name.into();
    }

    if let Some(label) = constructor_label(mangled) {
        if let Some((path, member)) = decoded.rsplit_once("::") {
            let owner = path.rsplit("::").next().unwrap_or(path);
            if owner.split('<').next() == Some(member) {
                return format!("{path}::{label}");
            }
        }
    }
    decoded.into()
}

// Scan the encoded components of an untemplated nested name. Length-prefixed
// identifiers can themselves contain strings such as C1, so skip their bodies.
fn constructor_label(mangled: &str) -> Option<&'static str> {
    let mut components = mangled.strip_prefix("_ZN")?;
    while !components.is_empty() {
        match components.as_bytes()[0] {
            b'0'..=b'9' => {
                let end = components.find(|character: char| !character.is_ascii_digit())?;
                let length: usize = components[..end].parse().ok()?;
                components = components.get(end.checked_add(length)?..)?;
            }
            b'S' => {
                if let Some(rest) = components.strip_prefix("St") {
                    components = rest;
                } else {
                    components = components.split_once('_')?.1;
                }
            }
            b'C' => {
                return match components.get(..3)? {
                    "C1E" => Some("complete object constructor"),
                    "C2E" => Some("base object constructor"),
                    "C3E" => Some("complete object allocating constructor"),
                    _ => None,
                };
            }
            b'K' | b'V' | b'r' => components = &components[1..],
            _ => return None,
        }
    }
    None
}
