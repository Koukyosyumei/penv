use std::{
    collections::HashMap,
    fs::File,
    io::{self, LineWriter, Write},
    path::Path,
    process::ExitCode,
};

use crate::{
    ethernet_switch::{Port, Switch},
    image,
    machine::{Machine, Stop},
    monitor,
    network_monitor::{NodeBoot, Plan},
    platform,
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
    Run(Run),
}

struct Node {
    name: String,
    machine: Machine,
    switch_by_nic: Vec<Option<usize>>,
    stop: Option<Stop>,
    retired: u64,
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
                    simulation
                        .nodes
                        .push(Node::boot(pending.next().unwrap(), root, assets)?);
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
        let mut machine = platform::load(&boot.platform.unwrap(), root)?;
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
        if let Some(value) = boot.image {
            let (file, path) = open(&value)?;
            image::load(&mut machine, file, false)
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        for (value, address) in boot.binaries {
            let (file, path) = open(&value)?;
            image::load_binary(&mut machine, file, address)
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        for (cpu, address) in boot.boot_pcs {
            machine.set_named_entry(&cpu, address)?;
        }
        machine
            .validate_entry()
            .map_err(|e| format!("{name}: {e}"))?;
        Ok(Self {
            name,
            switch_by_nic: vec![None; machine.bus.ethernet.len()],
            machine,
            stop: None,
            retired: 0,
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
                let (stop, retired) = node
                    .machine
                    .run(1, output)
                    .map_err(|e| format!("{}: {e}", node.name))?;
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
