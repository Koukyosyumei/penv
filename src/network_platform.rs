use crate::{
    ethernet::{Controller, Mapping, Region},
    litex_ethernet::{LiteXEthernet, Phy},
    machine::Machine,
    mii_phy::MiiPhy,
    platform::{Entry, number},
    stm32_ethernet::Stm32Ethernet,
    stm32h7_ethernet::{MAC_SIZE, Stm32h7Ethernet},
    stm32h7_net_control::NetworkControl,
};

const STM32F_MAC: &str = "Network.SynopsysEthernetMAC";
const STM32H7_RCC: &str = "Miscellaneous.STM32H7_RCC";
const STM32_SYSCFG: &str = "Miscellaneous.STM32_SYSCFG";
const STM32H7_MAC: &str = "Network.SynopsysDWCEthernetQualityOfService";

fn is_litex(model: &str) -> bool {
    matches!(
        model,
        "Network.LiteX_Ethernet" | "Network.LiteX_Ethernet_CSR32"
    )
}

pub fn is_network(entry: &Entry) -> bool {
    match entry.model.as_str() {
        STM32_SYSCFG => rcc_alias(entry, "RegistrationPeripheralAlias.SYSCFG").is_some(),
        STM32H7_RCC | STM32F_MAC | "Network.EthernetPhysicalLayer" => true,
        model => is_litex(model) || model == STM32H7_MAC,
    }
}

pub fn rcc_alias<'a>(entry: &'a Entry, alias: &str) -> Option<&'a str> {
    let list = entry
        .registration
        .as_deref()?
        .strip_prefix('{')?
        .strip_suffix('}')?;
    list.split(';').find_map(|registration| {
        let mut words = registration.split_whitespace();
        let name = words.next()?;
        (words.next() == Some(alias) && words.next().is_none()).then_some(name)
    })
}

pub fn configure(
    entries: &[Entry],
    machine: &mut Machine,
    vex_cpu: Option<&str>,
    nvic: Option<&str>,
) -> Result<(), String> {
    for entry in entries.iter().filter(|entry| is_litex(&entry.model)) {
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
        .filter(|entry| matches!(entry.model.as_str(), STM32F_MAC | STM32H7_MAC))
    {
        let qos = entry.model == STM32H7_MAC;
        let mut irq = None;
        let mut frequency = None;
        let mut width = None;
        for (_, text) in &entry.body {
            if qos && let Some((key, value)) = text.split_once(':') {
                match key.trim() {
                    "systemClockFrequency" if frequency.is_none() => {
                        let value = number(value.trim()).map_err(|e| entry.error(&e))?;
                        if value == 0 {
                            return Err(entry.error("Ethernet clock frequency must be positive"));
                        }
                        frequency = Some(value);
                    }
                    "dmaBusWidth" if width.is_none() && value.trim() == "BusWidth.Bits32" => {
                        width = Some(32);
                    }
                    _ => return Err(entry.error("unsupported or duplicate Ethernet QoS property")),
                }
                continue;
            }
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
        let device = if qos {
            let mappings = mappings(entry).map_err(|e| entry.error(&e))?;
            Controller::Stm32h7(Stm32h7Ethernet::new(entry.name.clone(), mappings, irq))
        } else {
            let base = entry
                .sysbus()?
                .ok_or_else(|| entry.error("missing STM32 Ethernet address"))?;
            Controller::Stm32(Stm32Ethernet::new(entry.name.clone(), base, irq))
        };
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
            Controller::Stm32h7(device)
                if !device.phys.iter().any(|other| other.address == phy.address) =>
            {
                device.phys.push(MiiPhy::new(phy.address, phy.registers));
            }
            _ => return Err(entry.error("duplicate or unsupported additional Ethernet PHY")),
        }
    }
    configure_board_control(entries, machine)?;

    Ok(())
}

fn configure_board_control(entries: &[Entry], machine: &mut Machine) -> Result<(), String> {
    let rccs: Vec<_> = entries
        .iter()
        .filter(|entry| entry.model == STM32H7_RCC)
        .collect();
    let Some(&rcc) = rccs.first() else {
        return Ok(());
    };
    if rccs.len() != 1 {
        return Err(rcc.error("only one STM32H7 clock controller is supported"));
    }
    let mut peripherals = entries.iter().filter(|entry| {
        entry.model == STM32H7_MAC
            && rcc_alias(entry, "RegistrationPeripheralAlias.ETH1MAC") == Some(&rcc.name)
    });
    let ethernet = peripherals.next();
    let dma_or_adc = entries.iter().any(|entry| {
        crate::stm32h7_adc_platform::has_regions(&entry.model)
            && [
                "RegistrationPeripheralAlias.ADC12",
                "RegistrationPeripheralAlias.ADC3",
                "RegistrationPeripheralAlias.DMA1",
                "RegistrationPeripheralAlias.DMA2",
                "RegistrationPeripheralAlias.BDMA",
            ]
            .into_iter()
            .any(|alias| rcc_alias(entry, alias) == Some(&rcc.name))
    });
    let gpio = entries.iter().any(|entry| {
        entry.model == crate::stm32h7_gpio_platform::GPIO
            && (b'A'..=b'K').any(|port| {
                let alias = format!("RegistrationPeripheralAlias.GPIO{}", char::from(port));
                rcc_alias(entry, &alias) == Some(&rcc.name)
            })
    });
    let rtc = entries.iter().any(|entry| {
        entry.model == crate::stm32h7_rtc_platform::RTC
            && rcc_alias(entry, "RegistrationPeripheralAlias.RTC") == Some(&rcc.name)
    });
    let spi = entries.iter().any(|entry| {
        entry.model == crate::stm32h7_spi::SPI
            && rcc_alias(entry, "RegistrationPeripheralAlias.SPI4") == Some(&rcc.name)
    });
    let board_peripheral = entries
        .iter()
        .any(|entry| crate::stm32h7_board_platform::has_rcc_binding(entry, rcc));
    if ethernet.is_none() && !dma_or_adc && !gpio && !rtc && !spi && !board_peripheral {
        return Err(rcc.error("STM32H7 RCC requires a supported peripheral binding"));
    }
    if peripherals.next().is_some() {
        return Err(rcc.error("multiple ETH1MAC clock bindings"));
    }
    let mut hse_frequency = 8_000_000;
    for (_, property) in &rcc.body {
        let Some((key, value)) = property.split_once(':') else {
            return Err(rcc.error("expected STM32H7 RCC property"));
        };
        match key.trim() {
            "nvic" | "dwt" if !value.trim().is_empty() => {}
            "hseFrequency" => {
                hse_frequency = number(value.trim()).map_err(|e| rcc.error(&e))?;
                if hse_frequency == 0 {
                    return Err(rcc.error("expected a nonzero HSE frequency"));
                }
            }
            _ => return Err(rcc.error("unsupported STM32H7 RCC property")),
        }
    }
    let rcc_base = rcc
        .sysbus()?
        .ok_or_else(|| rcc.error("missing STM32H7 RCC address"))?;
    let mut syscfgs = entries.iter().filter(|entry| {
        entry.model == STM32_SYSCFG
            && rcc_alias(entry, "RegistrationPeripheralAlias.SYSCFG") == Some(&rcc.name)
    });
    let syscfg_base = if let Some(syscfg) = syscfgs.next() {
        if syscfgs.next().is_some() {
            return Err(syscfg.error("multiple Ethernet SYSCFG bindings"));
        }
        let registrations = syscfg.registration.as_deref().unwrap();
        let mut bases = registrations[1..registrations.len() - 1]
            .split(';')
            .filter_map(|registration| {
                let mut words = registration.split_whitespace();
                (words.next() == Some("sysbus")).then(|| {
                    words
                        .next()
                        .ok_or_else(|| "missing SYSCFG address".to_owned())
                })
            });
        let base = bases
            .next()
            .ok_or_else(|| syscfg.error("missing SYSCFG sysbus registration"))?
            .and_then(number)
            .map_err(|error| syscfg.error(&error))?;
        if bases.next().is_some() {
            return Err(syscfg.error("expected one SYSCFG sysbus registration"));
        }
        Some(base)
    } else {
        None
    };
    let nic = ethernet
        .map(|ethernet| {
            machine
                .bus
                .ethernet
                .iter()
                .position(|device| device.name() == ethernet.name)
                .ok_or_else(|| ethernet.error("missing STM32H7 Ethernet controller"))
        })
        .transpose()?;
    // Renode returns zero for unhandled MMIO. Zephyr accesses DBGMCU
    // during STM32H7 startup even though the upstream platform has no model.
    machine.bus.tag(0x5c001000..=0x5c0013ff, 0);
    let mut control = NetworkControl::new(rcc_base, syscfg_base, nic);
    control.hse_frequency = hse_frequency;
    machine
        .bus
        .map_h7_network_control(control)
        .map_err(|error| rcc.error(&error))
}

fn mappings(entry: &Entry) -> Result<Vec<Mapping>, String> {
    let qos = entry.model == STM32H7_MAC;
    let csr_size = if qos { MAC_SIZE } else { 0x100 };
    let text = entry
        .registration
        .as_deref()
        .ok_or("missing Ethernet registration")?;
    let Some(list) = text
        .strip_prefix('{')
        .and_then(|text| text.strip_suffix('}'))
    else {
        let base = entry.sysbus()?.ok_or("missing Ethernet address")?;
        let mut result = vec![Mapping {
            base,
            size: csr_size,
            region: Region::Registers,
        }];
        if qos {
            for (offset, region) in [(0xc00, Region::Mtl), (0x1000, Region::Dma)] {
                result.push(Mapping {
                    base: base
                        .checked_add(offset)
                        .ok_or("Ethernet region address overflow")?,
                    size: 0x200,
                    region,
                });
            }
        }
        return Ok(result);
    };
    let mut result = Vec::new();
    let mut rest = list.trim();
    while !rest.is_empty() {
        let (registration, next) = rest.split_once(';').unwrap_or((rest, ""));
        if qos
            && registration.split_whitespace().nth(1) == Some("RegistrationPeripheralAlias.ETH1MAC")
        {
            rest = next.trim_start();
            continue;
        }
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
            result.push(region(properties, qos)?);
            rest = next.trim_start();
        } else {
            let (base, next) = registration.split_once(';').unwrap_or((registration, ""));
            result.push(Mapping {
                base: number(base.trim())?,
                size: csr_size,
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

fn region(properties: &str, qos: bool) -> Result<Mapping, String> {
    let mut address = None;
    let mut size = None;
    let mut region = None;
    for (key, value) in crate::platform::object_fields(properties)? {
        match key {
            "address" if address.is_none() => address = Some(number(value)?),
            "size" if size.is_none() => size = Some(number(value)?),
            "region" if region.is_none() => {
                region = Some(match value {
                    "\"buffer\"" if !qos => Region::Buffer,
                    "\"phy\"" if !qos => Region::Phy,
                    "\"mtl\"" if qos => Region::Mtl,
                    "\"dma\"" if qos => Region::Dma,
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
