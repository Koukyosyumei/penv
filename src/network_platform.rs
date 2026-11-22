use crate::{
    ethernet::{Controller, Mapping, Region},
    litex_ethernet::{LiteXEthernet, Phy},
    machine::Machine,
    platform::{Entry, number},
    stm32_ethernet::Stm32Ethernet,
};

pub fn is_mac(model: &str) -> bool {
    matches!(
        model,
        "Network.LiteX_Ethernet" | "Network.LiteX_Ethernet_CSR32"
    )
}

pub fn is_network(model: &str) -> bool {
    is_mac(model)
        || matches!(
            model,
            "Network.EthernetPhysicalLayer" | "Network.SynopsysEthernetMAC"
        )
}

pub fn configure(
    entries: &[Entry],
    machine: &mut Machine,
    vex_cpu: Option<&str>,
    nvic: Option<&str>,
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
            .map_ethernet(Controller::LiteX(device))
            .map_err(|e| entry.error(&e))?;
    }
    for entry in entries
        .iter()
        .filter(|entry| entry.model == "Network.SynopsysEthernetMAC")
    {
        let mut irq = None;
        for (_, text) in &entry.body {
            let (pin, target) = text
                .split_once("->")
                .ok_or_else(|| entry.error("expected STM32 Ethernet IRQ connection"))?;
            let (target, line) = target
                .split_once('@')
                .ok_or_else(|| entry.error("expected STM32 Ethernet IRQ destination"))?;
            let line = number(line.trim()).map_err(|e| entry.error(&e))?;
            if !matches!(pin.trim(), "" | "IRQ")
                || Some(target.trim()) != nvic
                || irq.is_some()
                || line >= 64
            {
                return Err(entry.error("expected one Ethernet IRQ wired to NVIC line 0–63"));
            }
            irq = Some(line as u8);
        }
        let base = entry
            .sysbus()?
            .ok_or_else(|| entry.error("missing STM32 Ethernet address"))?;
        machine
            .bus
            .map_ethernet(Controller::Stm32(Stm32Ethernet::new(
                entry.name.clone(),
                base,
                irq,
            )))
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
            .find(|device| device.name() == parent)
            .ok_or_else(|| entry.error("unknown Ethernet parent"))?;
        if address > 31 || registration.next().is_some() {
            return Err(entry.error("expected a PHY address in 0–31"));
        }
        let mut phy = Phy::new(address as u8);
        for (_, text) in &entry.body {
            let (key, value) = text
                .split_once(':')
                .ok_or_else(|| entry.error("expected PHY register property"))?;
            let key = key.trim();
            let register = match key {
                "BasicControl" => Some(0),
                "BasicStatus" => Some(1),
                "Id1" => Some(2),
                "Id2" => Some(3),
                "AutoNegotiationAdvertisement" => Some(4),
                "AutoNegotiationLinkPartnerBasePageAbility" => Some(5),
                "AutoNegotiationExpansion" => Some(6),
                "AutoNegotiationNextPageTransmit" => Some(7),
                "AutoNegotiationLinkPartnerReceivedNextPage" => Some(8),
                "MasterSlaveControl" => Some(9),
                "MasterSlaveStatus" => Some(10),
                "ExtendedStatus" => Some(15),
                _ => key
                    .strip_prefix("VendorSpecific")
                    .and_then(|index| index.parse::<usize>().ok())
                    .filter(|&index| index < 16)
                    .map(|index| 16 + index),
            }
            .ok_or_else(|| entry.error("unsupported PHY property"))?;
            phy.registers[register] =
                u16::try_from(number(value.trim()).map_err(|e| entry.error(&e))?)
                    .map_err(|_| entry.error("PHY register value exceeds 16 bits"))?;
        }
        match device {
            Controller::LiteX(device) if device.phy.is_none() => device.phy = Some(phy),
            Controller::Stm32(device)
                if !device
                    .phys
                    .iter()
                    .any(|(address, _)| *address == phy.address) =>
            {
                device.phys.push((phy.address, phy.registers));
            }
            _ => return Err(entry.error("duplicate or unsupported additional Ethernet PHY")),
        }
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
