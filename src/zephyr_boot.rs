use std::{fs::File, path::Path};

use crate::{image, machine::Machine};

const BUSY_WAIT: &str = "z_impl_k_busy_wait";
const TICKLESS: &str = "CONFIG_TICKLESS_KERNEL";

#[derive(Default)]
pub struct BootOptions {
    pub busy_wait: Option<(String, Vec<String>)>,
    pub vectors: Option<(String, VectorAddress)>,
}

pub enum VectorAddress {
    Address(u32),
    Symbol(String),
}

pub fn configure(
    machine: &mut Machine,
    options: &BootOptions,
    image_path: Option<&Path>,
) -> Result<(), String> {
    let mut wanted = Vec::new();
    if let Some((_, exceptions)) = &options.busy_wait {
        wanted.extend([BUSY_WAIT, TICKLESS]);
        wanted.extend(exceptions.iter().map(String::as_str));
    }
    if let Some((_, VectorAddress::Symbol(symbol))) = &options.vectors {
        wanted.push(symbol);
    }
    let symbols = if wanted.is_empty() {
        Vec::new()
    } else if let Some(path) = image_path {
        let file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
        image::find_symbols(file, &wanted)
            .map_err(|error| format!("{}: {error}", path.display()))?
    } else {
        Vec::new()
    };

    if let Some((cpu, address)) = &options.vectors {
        let base = match address {
            VectorAddress::Address(base) => *base,
            VectorAddress::Symbol(symbol) => symbols
                .iter()
                .find_map(|&(name, value)| (name == symbol).then_some(value))
                .ok_or_else(|| format!("ELF symbol {symbol:?} was not found"))?,
        };
        machine.set_boot_vectors(cpu, base)?;
    }
    if let Some((cpu, exceptions)) = &options.busy_wait {
        let has_symbol = |symbol: &str| symbols.iter().any(|&(name, _)| name == symbol);
        if let Some(symbol) = exceptions.iter().find(|symbol| has_symbol(symbol)) {
            eprintln!("penv: Zephyr mode disabled because ELF symbol {symbol:?} is present");
        } else {
            if has_symbol(TICKLESS) {
                eprintln!("penv: ELF symbol {TICKLESS} can affect Zephyr busy-wait time skipping");
            }
            let entries = symbols
                .iter()
                .filter_map(|&(name, address)| (name == BUSY_WAIT).then_some(address));
            machine.set_zephyr_wait_hooks(cpu, entries)?;
        }
    }
    Ok(())
}

#[derive(Clone)]
pub struct BusyWait {
    entries: Vec<u32>,
    remaining: u64,
}

impl BusyWait {
    pub fn new(entries: impl Iterator<Item = u32>) -> Option<Self> {
        let mut entries: Vec<_> = entries.map(|entry| entry & !1).collect();
        entries.sort_unstable();
        entries.dedup();
        (!entries.is_empty()).then_some(Self {
            entries,
            remaining: 0,
        })
    }

    pub fn reset(&mut self) {
        self.remaining = 0;
    }

    pub fn waiting(&mut self) -> bool {
        if self.remaining == 0 {
            return false;
        }
        self.remaining -= 1;
        true
    }

    pub fn contains(&self, pc: u32) -> bool {
        self.entries.binary_search(&pc).is_ok()
    }

    pub fn skip(&mut self, microseconds: u32, step_hz: u32) {
        let steps = (u64::from(microseconds) * u64::from(step_hz)).div_ceil(1_000_000);
        self.remaining = steps.saturating_sub(1);
    }
}
