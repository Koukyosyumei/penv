use std::{
    collections::HashMap,
    fs::File,
    io::{self, BufWriter, LineWriter, Write},
    path::Path,
    process::ExitCode,
};

use crate::{
    ethernet_switch::{Port, Switch},
    image,
    machine::{Machine, Stop},
    network_monitor::{NodeBoot, Plan},
    network_repl, network_resc as monitor, zephyr_boot,
};

pub enum Run {
    Remaining,
    Rounds(u64),
}

pub enum Connection {
    Connect,
    Disconnect,
    Promiscuous(bool),
}

pub enum Action {
    Activate(usize),
    CreateSwitch(String),
    Connect {
        operation: Connection,
        node: usize,
        nic: String,
        switch: String,
    },
    Button {
        node: usize,
        name: String,
        pressed: bool,
    },
    DisplayRate {
        node: usize,
        fps: u32,
    },
    WriteWord {
        node: usize,
        address: u32,
        value: u32,
    },
    FindBytes {
        node: usize,
        search: crate::monitor::FindBytes,
    },
    ReadWord {
        node: usize,
        address: u32,
    },
    Run(Run),
}

struct Node {
    name: String,
    machine: Machine,
    switch_by_nic: Vec<Option<usize>>,
    stop: Option<Stop>,
    retired: u64,
    uart_log: Option<BufWriter<File>>,
}

#[derive(Default)]
struct Simulation {
    nodes: Vec<Node>,
    switches: Vec<Switch>,
    switch_ids: HashMap<String, usize>,
    rounds: u64,
}

pub fn run(
    plan: Plan,
    root: Option<&Path>,
    assets: Option<&Path>,
    limit: u64,
    stats: bool,
    uart_log_dir: Option<&Path>,
) -> Result<ExitCode, String> {
    let mut simulation = Simulation::default();
    let mut pending = plan.nodes.into_iter();
    let mut output = LineWriter::new(io::stdout().lock());
    let mut ran = false;
    let result = (|| {
        for action in plan.actions {
            match action {
                Action::Activate(id) => {
                    debug_assert_eq!(id, simulation.nodes.len());
                    let mut node = Node::boot(pending.next().unwrap(), root, assets)?;
                    if let Some(directory) = uart_log_dir {
                        let path = directory.join(format!("{id}.uart.log"));
                        node.uart_log = Some(BufWriter::new(
                            std::fs::OpenOptions::new()
                                .write(true)
                                .create_new(true)
                                .open(&path)
                                .map_err(|e| format!("{}: {e}", path.display()))?,
                        ));
                    }
                    simulation.nodes.push(node);
                }
                Action::CreateSwitch(name) => simulation.create_switch(name)?,
                Action::Connect {
                    operation,
                    node,
                    nic,
                    switch,
                } => {
                    simulation.connect(operation, node, &nic, &switch)?;
                }
                Action::Button {
                    node,
                    name,
                    pressed,
                } => {
                    simulation.nodes[node]
                        .machine
                        .bus
                        .gpio_button(&name, pressed)?;
                }
                Action::DisplayRate { node, fps } => {
                    simulation.nodes[node]
                        .machine
                        .bus
                        .ltdc_frames_per_second(fps)?;
                }
                Action::WriteWord {
                    node,
                    address,
                    value,
                } => {
                    let node = &mut simulation.nodes[node];
                    if let Some(log) = &mut node.uart_log {
                        node.machine.bus.write(address, 4, value, log)
                    } else {
                        node.machine.bus.write(address, 4, value, &mut output)
                    }
                    .map_err(|e| format!("{}: {e}", node.name))?;
                }
                Action::FindBytes { node, search } => {
                    let node = &simulation.nodes[node];
                    search.write_matches(
                        &node.machine.bus,
                        &format!("{}: sysbus matches = ", node.name),
                        &mut output,
                    )?;
                }
                Action::ReadWord { node, address } => {
                    let node = &mut simulation.nodes[node];
                    let value = node
                        .machine
                        .bus
                        .read::<4>(address)
                        .map_err(|error| format!("{}: {error}", node.name))?;
                    writeln!(
                        output,
                        "{}: sysbus[{address:#010x}] = {value:#010x}",
                        node.name
                    )
                    .map_err(|error| format!("sysbus read output: {error}"))?;
                }
                Action::Run(run) => {
                    ran = true;
                    if simulation.advance(run, limit, &mut output)? {
                        return Ok(true);
                    }
                }
            }
        }
        if ran {
            Ok(false)
        } else {
            simulation.advance(Run::Remaining, limit, &mut output)
        }
    })();
    output.flush().map_err(|e| format!("UART: {e}"))?;
    for node in &mut simulation.nodes {
        if let Some(log) = &mut node.uart_log {
            log.flush()
                .map_err(|e| format!("{} UART: {e}", node.name))?;
        }
    }
    let hit_limit = result?;
    if stats {
        eprintln!("{} simulation rounds", simulation.rounds);
        for node in &simulation.nodes {
            let reason = match node.stop {
                Some(Stop::Ecall) => "Ecall",
                Some(Stop::Ebreak) => "Ebreak",
                _ if hit_limit => "Limit",
                _ => "Paused",
            };
            eprintln!(
                "{}: {reason}, {} instructions, pc={:#010x}",
                node.name, node.retired, node.machine.primary.pc
            );
        }
    }
    if hit_limit {
        eprintln!("penv: simulation round limit reached");
        Ok(ExitCode::from(2))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

impl Node {
    fn boot(node: NodeBoot, root: Option<&Path>, assets: Option<&Path>) -> Result<Self, String> {
        let NodeBoot { name, boot, source } = node;
        let mut machine = network_repl::load(&boot.platform.unwrap(), root)?;
        if let Some(mips) = boot.mips {
            machine
                .bus
                .set_step_hz(mips.checked_mul(1_000_000).ok_or("virtual MIPS overflow")?);
        }
        if let Some(uart) = &boot.uart_input {
            machine.bus.select_uart_input(uart);
        }
        let open = |value: &str| {
            let path = monitor::asset_path(value, &source, root.unwrap_or(Path::new(".")), assets)?;
            let file = File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            Ok::<_, String>((file, path))
        };
        if let Some(value) = boot.mtvec {
            machine.primary.set_mtvec(value);
        }
        if let Some(value) = boot.mask {
            machine.primary.set_vex_irq_mask(value)?;
        }
        let mut image_path = None;
        if let Some(value) = boot.image {
            let (file, path) = open(&value)?;
            image::load(&mut machine, file, false, None)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            image_path = Some(path);
        }
        for (value, address) in boot.binaries {
            let (file, path) = open(&value)?;
            image::load_binary(&mut machine, file, address.into())
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        for (cpu, address) in boot.boot_pcs {
            machine.set_named_entry(&cpu, address)?;
        }
        for (address, value) in boot.writes {
            machine
                .bus
                .write(address, 4, value, &mut io::sink())
                .map_err(|error| error.to_string())?;
        }
        zephyr_boot::configure(&mut machine, &boot.zephyr, image_path.as_deref())?;
        machine
            .validate_entry()
            .map_err(|e| format!("{name}: {e}"))?;
        Ok(Self {
            name,
            switch_by_nic: vec![None; machine.bus.ethernet.len()],
            machine,
            stop: None,
            retired: 0,
            uart_log: None,
        })
    }
}

impl Simulation {
    fn create_switch(&mut self, name: String) -> Result<(), String> {
        if name.is_empty() || self.switch_ids.contains_key(&name) {
            return Err(format!("invalid or duplicate switch name {name:?}"));
        }
        self.switch_ids.insert(name, self.switches.len());
        self.switches.push(Switch::new());
        Ok(())
    }

    fn connect(
        &mut self,
        operation: Connection,
        node: usize,
        nic: &str,
        switch: &str,
    ) -> Result<(), String> {
        let switch_id = *self
            .switch_ids
            .get(switch)
            .ok_or_else(|| format!("unknown switch {switch:?}"))?;
        let device = &mut self.nodes[node];
        let nic = device
            .machine
            .bus
            .ethernet
            .iter()
            .position(|device| device.name() == nic)
            .ok_or_else(|| format!("{}: unknown Ethernet peripheral {nic:?}", device.name))?;
        let port = Port { node, nic };
        let attached = &mut device.switch_by_nic[nic];
        let switch = &mut self.switches[switch_id];
        match operation {
            Connection::Connect => {
                if attached.is_some() {
                    return Err(format!(
                        "{}: Ethernet port is already connected",
                        device.name
                    ));
                }
                switch.connect(port)?;
                *attached = Some(switch_id);
            }
            Connection::Disconnect => {
                switch.disconnect(port)?;
                *attached = None;
            }
            Connection::Promiscuous(enabled) => switch.promiscuous(port, enabled)?,
        }
        Ok(())
    }

    fn advance(&mut self, run: Run, limit: u64, output: &mut impl Write) -> Result<bool, String> {
        let available = limit - self.rounds;
        let requested = match run {
            Run::Remaining => u64::MAX,
            Run::Rounds(rounds) => rounds,
        };
        for _ in 0..available.min(requested) {
            let mut active = false;
            for index in 0..self.nodes.len() {
                let node = &mut self.nodes[index];
                if node.stop.is_some() {
                    continue;
                }
                let result = if let Some(log) = &mut node.uart_log {
                    node.machine.run(1, log)
                } else {
                    node.machine.run(1, output)
                };
                let (stop, retired) = result.map_err(|e| format!("{}: {e}", node.name))?;
                node.retired += retired;
                if stop == Stop::Limit {
                    active = true;
                } else {
                    node.stop = Some(stop);
                }
                self.forward(index);
            }
            self.rounds += 1;
            if !active {
                return Ok(false);
            }
        }
        Ok(requested > available && self.nodes.iter().any(|node| node.stop.is_none()))
    }

    fn forward(&mut self, node: usize) {
        for nic in 0..self.nodes[node].switch_by_nic.len() {
            while let Some(frame) = self.nodes[node].machine.bus.ethernet[nic].take_frame() {
                let Some(switch) = self.nodes[node].switch_by_nic[nic] else {
                    continue;
                };
                for port in self.switches[switch].recipients(Port { node, nic }, &frame) {
                    self.nodes[port.node]
                        .machine
                        .bus
                        .receive_ethernet(port.nic, &frame);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bus::{Bus, RAM_BASE},
        ethernet::{Controller, Mapping, Region},
        litex_ethernet::{LiteXEthernet, SLOT_SIZE},
        stm32h7_ethernet::{MAC_SIZE, Stm32h7Ethernet},
    };

    #[test]
    fn switches_frames_between_stm32h7_dma_and_litex_slots_in_both_directions() {
        const MAC: u32 = 0x4002_8000;
        const DMA: u32 = MAC + 0x1000;
        const CSR: u32 = 0x6000_9800;
        const SLOTS: u32 = 0x3000_0000;
        const TX_DESC: u32 = RAM_BASE;
        const RX_DESC: u32 = RAM_BASE + 0x10;
        const TX_BUFFER: u32 = RAM_BASE + 0x100;
        const RX_BUFFER: u32 = RAM_BASE + 0x200;
        const OWN: u32 = 1 << 31;
        const FIRST_LAST: u32 = (1 << 29) | (1 << 28);

        let mut simulation = Simulation::default();
        let mut litex = LiteXEthernet::new("mac".into(), true, 1, 1);
        litex.mappings = vec![
            Mapping {
                base: CSR,
                size: 0x100,
                region: Region::Registers,
            },
            Mapping {
                base: SLOTS,
                size: 2 * SLOT_SIZE as u32,
                region: Region::Buffer,
            },
        ];
        let h7 = Stm32h7Ethernet::new(
            "mac".into(),
            vec![
                Mapping {
                    base: MAC,
                    size: MAC_SIZE,
                    region: Region::Registers,
                },
                Mapping {
                    base: DMA,
                    size: 0x400,
                    region: Region::Dma,
                },
            ],
            None,
        );
        simulation.create_switch("lan".into()).unwrap();
        for (index, device) in [Controller::Stm32h7(h7), Controller::LiteX(litex)]
            .into_iter()
            .enumerate()
        {
            let mut machine = Machine::new(0x1000).unwrap();
            machine.bus.map_ethernet(device).unwrap();
            simulation.nodes.push(Node {
                name: index.to_string(),
                machine,
                switch_by_nic: vec![None],
                stop: None,
                retired: 0,
                uart_log: None,
            });
            simulation
                .connect(Connection::Connect, index, "mac", "lan")
                .unwrap();
        }

        let write = |bus: &mut Bus, address, value| {
            bus.write(address, 4, value, &mut io::sink()).unwrap();
        };
        let mut frame = [0u8; 60];
        frame[..14].copy_from_slice(&[2, 0, 0, 0, 0, 2, 2, 0, 0, 0, 0, 1, 0x88, 0xb5]);
        frame[14..].fill(0x41);
        let bus = &mut simulation.nodes[0].machine.bus;
        bus.memory(TX_BUFFER, frame.len())
            .unwrap()
            .copy_from_slice(&frame);
        for (address, value) in [
            (TX_DESC, TX_BUFFER),
            (TX_DESC + 8, frame.len() as u32 | OWN),
            (TX_DESC + 12, OWN | FIRST_LAST),
            (RX_DESC, RX_BUFFER),
            (RX_DESC + 12, OWN | (1 << 30) | (1 << 24)),
            (MAC, 3),
            (DMA + 0x114, TX_DESC),
            (DMA + 0x11c, RX_DESC),
            (DMA + 0x108, (128 << 1) | 1),
            (DMA + 0x128, RX_DESC),
            (DMA + 0x104, 1),
            (DMA + 0x120, TX_DESC),
        ] {
            write(bus, address, value);
        }
        assert_eq!(bus.read::<4>(TX_DESC + 12).unwrap(), FIRST_LAST);
        simulation.forward(0);

        let bus = &mut simulation.nodes[1].machine.bus;
        assert_eq!(bus.read::<4>(CSR + 4).unwrap(), 64);
        assert_eq!(bus.read::<4>(CSR + 0x10).unwrap(), 1);
        for (offset, byte) in frame.iter().copied().enumerate() {
            assert_eq!(
                bus.read::<1>(SLOTS + offset as u32).unwrap(),
                u32::from(byte)
            );
        }
        write(bus, CSR + 0x10, 1);

        frame.swap(5, 11);
        frame[14..].fill(0x42);
        for (offset, byte) in frame.iter().copied().enumerate() {
            bus.write(
                SLOTS + SLOT_SIZE as u32 + offset as u32,
                1,
                u32::from(byte),
                &mut io::sink(),
            )
            .unwrap();
        }
        write(bus, CSR + 0x28, frame.len() as u32);
        write(bus, CSR + 0x18, 1);
        simulation.forward(1);

        let bus = &mut simulation.nodes[0].machine.bus;
        assert_eq!(bus.read::<4>(RX_DESC + 12).unwrap(), FIRST_LAST | 64);
        assert_eq!(bus.memory(RX_BUFFER, frame.len()).unwrap(), frame);
    }
}
