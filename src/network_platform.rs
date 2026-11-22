use crate::{
    litex_ethernet::{LiteXEthernet, Mapping, Phy, Region},
    machine::Machine,
    platform::{Entry, number},
};

pub fn is_mac(model: &str) -> bool {
    matches!(
        model,
        "Network.LiteX_Ethernet" | "Network.LiteX_Ethernet_CSR32"
    )
}

pub fn is_network(model: &str) -> bool {
    is_mac(model) || model == "Network.EthernetPhysicalLayer"
}

pub fn configure(
    entries: &[Entry],
    machine: &mut Machine,
    vex_cpu: Option<&str>,
) -> Result<(), String> {
    for entry in entries.iter().filter(|entry| is_mac(&entry.model)) {
        let mut rx = None;
        let mut tx = None;
        let mut irq = None;
        for (_, text) in &entry.body {
            if let Some((pin, target)) = text.split_once("->") {
                let (target, line) = target
                    .split_once('@')
                    .ok_or_else(|| entry.error("expected Ethernet IRQ destination"))?;
                if !matches!(pin.trim(), "" | "IRQ")
                    || Some(target.trim()) != vex_cpu
                    || irq.is_some()
                {
                    return Err(
                        entry.error("expected one Ethernet IRQ wired to the primary VexRiscv CPU")
                    );
                }
                let line = number(line.trim()).map_err(|e| entry.error(&e))?;
                if line >= 32 {
                    return Err(entry.error("VexRiscv IRQ line must be in 0–31"));
                }
                irq = Some(line);
                continue;
            }
            let (key, value) = text
                .split_once(':')
                .ok_or_else(|| entry.error("expected Ethernet property"))?;
            let slot = match key.trim() {
                "numberOfWriteSlots" if rx.is_none() => &mut rx,
                "numberOfReadSlots" if tx.is_none() => &mut tx,
                _ => return Err(entry.error("unsupported or duplicate Ethernet property")),
            };
            let count = number(value.trim()).map_err(|e| entry.error(&e))?;
            if !(1..=256).contains(&count) {
                return Err(entry.error("Ethernet slot count must be in 1–256"));
            }
            *slot = Some(count as usize);
        }
        let mut device = LiteXEthernet::new(
            entry.name.clone(),
            entry.model.ends_with("_CSR32"),
            rx.unwrap_or(2),
            tx.unwrap_or(2),
        );
        device.irq_line = irq;
        device.mappings = mappings(entry).map_err(|e| entry.error(&e))?;
        machine
            .bus
            .map_ethernet(device)
            .map_err(|e| entry.error(&e))?;
    }
    for entry in entries
        .iter()
        .filter(|entry| entry.model == "Network.EthernetPhysicalLayer")
    {
        let mut registration = entry
            .registration
            .as_deref()
            .unwrap_or_default()
            .split_whitespace();
        let parent = registration
            .next()
            .ok_or_else(|| entry.error("missing PHY parent"))?;
        let phy = registration
            .next()
            .ok_or_else(|| entry.error("missing PHY address"))?;
        let address = number(phy).map_err(|e| entry.error(&e))?;
        let device = machine
            .bus
            .ethernet
            .iter_mut()
            .find(|device| device.name == parent)
            .ok_or_else(|| entry.error("unknown LiteX Ethernet parent"))?;
        if address > 31 || registration.next().is_some() || device.phy.is_some() {
            return Err(entry.error("expected one PHY address in 0–31 per Ethernet device"));
        }
        let mut phy = Phy::new(address as u8);
        for (_, text) in &entry.body {
            let (key, value) = text
                .split_once(':')
                .ok_or_else(|| entry.error("expected PHY register property"))?;
            let register = match key.trim() {
                "Id1" => 2,
                "Id2" => 3,
                "VendorSpecific1" => 16,
                "VendorSpecific2" => 17,
                _ => return Err(entry.error("unsupported PHY property")),
            };
            phy.registers[register] =
                u16::try_from(number(value.trim()).map_err(|e| entry.error(&e))?)
                    .map_err(|_| entry.error("PHY register value exceeds 16 bits"))?;
        }
        device.phy = Some(phy);
    }
    Ok(())
}

fn mappings(entry: &Entry) -> Result<Vec<Mapping>, String> {
    let text = entry
        .registration
        .as_deref()
        .ok_or("missing Ethernet registration")?;
    let Some(list) = text
        .strip_prefix('{')
        .and_then(|text| text.strip_suffix('}'))
    else {
        return Ok(vec![Mapping {
            base: entry.sysbus()?.ok_or("missing Ethernet address")?,
            size: 0x100,
            region: Region::Registers,
        }]);
    };
    let mut result = Vec::new();
    let mut rest = list.trim();
    while !rest.is_empty() {
        let registration = rest
            .strip_prefix("sysbus")
            .filter(|text| text.starts_with(char::is_whitespace))
            .ok_or("expected Ethernet registration on sysbus")?
            .trim_start();
        if let Some(body) = registration
            .strip_prefix("new Bus.BusMultiRegistration")
            .map(str::trim_start)
            .and_then(|text| text.strip_prefix('{'))
        {
            let (properties, next) = body.split_once('}').ok_or("unterminated Ethernet region")?;
            result.push(region(properties)?);
            rest = next.trim_start();
        } else {
            let (base, next) = registration.split_once(';').unwrap_or((registration, ""));
            result.push(Mapping {
                base: number(base.trim())?,
                size: 0x100,
                region: Region::Registers,
            });
            rest = if next.is_empty() {
                ""
            } else {
                &registration[base.len()..]
            };
        }
        if !rest.is_empty() {
            rest = rest
                .strip_prefix(';')
                .ok_or("expected separator between Ethernet registrations")?
                .trim_start();
        }
    }
    if !result
        .iter()
        .any(|mapping| mapping.region == Region::Registers)
    {
        return Err("missing Ethernet register mapping".into());
    }
    Ok(result)
}

fn region(properties: &str) -> Result<Mapping, String> {
    let mut address = None;
    let mut size = None;
    let mut region = None;
    for property in properties
        .split(';')
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        let (key, value) = property
            .split_once(':')
            .ok_or("expected Ethernet region property")?;
        let value = value.trim();
        match key.trim() {
            "address" if address.is_none() => address = Some(number(value)?),
            "size" if size.is_none() => size = Some(number(value)?),
            "region" if region.is_none() => {
                region = Some(match value {
                    "\"buffer\"" => Region::Buffer,
                    "\"phy\"" => Region::Phy,
                    _ => return Err(format!("unknown Ethernet connection region {value}")),
                });
            }
            _ => {
                return Err(format!(
                    "unsupported or duplicate Ethernet region property {key:?}"
                ));
            }
        }
    }
    Ok(Mapping {
        base: address.ok_or("missing Ethernet region address")?,
        size: size.ok_or("missing Ethernet region size")?,
        region: region.ok_or("missing Ethernet region name")?,
    })
}
