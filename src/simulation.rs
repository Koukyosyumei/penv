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
    monitor,
    network_monitor::{NodeBoot, Plan},
    platform, zephyr_boot,
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
    Pointer {
        node: usize,
        name: String,
        event: crate::ft5336::PointerEvent,
    },
    MagneticFeed {
        node: usize,
        sensor: String,
        file: std::path::PathBuf,
    },
    VoltageFeed {
        node: usize,
        sensor: String,
        file: std::path::PathBuf,
        input: usize,
        channel: u16,
    },
    PressureFeed {
        node: usize,
        sensor: String,
        temperature: bool,
        file: std::path::PathBuf,
    },
    PressureInput {
        node: usize,
        sensor: String,
        temperature: bool,
        value: f64,
    },
    ReadPressure {
        node: usize,
        sensor: String,
    },
    IlluminanceFeed {
        node: usize,
        sensor: String,
        file: std::path::PathBuf,
    },
    Illuminance {
        node: usize,
        sensor: String,
        value: f64,
    },
    ImuResd {
        node: usize,
        sensor: String,
        file: std::path::PathBuf,
        angular_rate: bool,
    },
    ImuFeed {
        node: usize,
        sensor: String,
        file: std::path::PathBuf,
    },
    ImuInput {
        node: usize,
        sensor: String,
        axis: usize,
        value: f64,
    },
    TraceRcc {
        node: usize,
    },
    RccWrite {
        node: usize,
        offset: u32,
        value: u32,
    },
    TimerProperty {
        node: usize,
        name: String,
        frequency: bool,
    },
    TimerInput {
        node: usize,
        name: String,
        channel: u8,
        high: bool,
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
    ReadWord {
        node: usize,
        address: u32,
    },
    ReadMemoryBytes {
        node: usize,
        address: u32,
        count: usize,
    },
    WriteMemoryBytes {
        node: usize,
        address: u32,
        bytes: Vec<u8>,
    },
    ObserveUart {
        node: usize,
        name: String,
    },
    FailOnUart {
        node: usize,
        name: String,
        pattern: String,
    },
    WriteUartLine {
        node: usize,
        name: String,
        text: String,
    },
    WaitUart {
        node: usize,
        name: String,
        timeout: u64,
        unfinished: bool,
        patterns: Vec<String>,
    },
    WaitLed {
        node: usize,
        name: String,
        timeout: u64,
        state: bool,
    },
    BlinkLed {
        node: usize,
        name: String,
        duration: u64,
        on: u64,
        off: u64,
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
    let monitor_only = plan.is_monitor_only();
    let mut pending = plan.nodes.into_iter();
    let mut output = LineWriter::new(io::stdout().lock());
    let mut ran = false;
    let result = (|| {
        for action in plan.actions {
            match action {
                Action::Activate(id) => {
                    debug_assert_eq!(id, simulation.nodes.len());
                    let mut node = Node::boot(pending.next().unwrap(), root, assets, monitor_only)?;
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
                    let bus = &mut simulation.nodes[node].machine.bus;
                    let event = if pressed {
                        crate::ft5336::PointerEvent::Press
                    } else {
                        crate::ft5336::PointerEvent::Release
                    };
                    if bus.touch_input(&format!("sysbus.{name}"), event).is_err() {
                        bus.gpio_button(&name, pressed)?;
                    }
                }
                Action::Pointer { node, name, event } => {
                    simulation.nodes[node]
                        .machine
                        .bus
                        .touch_input(&name, event)?;
                }
                Action::MagneticFeed { node, sensor, file } => {
                    simulation.nodes[node]
                        .machine
                        .bus
                        .feed_magnetic(&sensor, &file)?;
                }
                Action::VoltageFeed {
                    node,
                    sensor,
                    file,
                    input,
                    channel,
                } => {
                    simulation.nodes[node]
                        .machine
                        .bus
                        .feed_ina228(&sensor, &file, input, channel)?;
                }
                Action::PressureFeed {
                    node,
                    sensor,
                    temperature,
                    file,
                } => {
                    simulation.nodes[node].machine.bus.feed_pressure(
                        &sensor,
                        &file,
                        temperature,
                    )?;
                }
                Action::PressureInput {
                    node,
                    sensor,
                    temperature,
                    value,
                } => {
                    let device = simulation.nodes[node].machine.bus.find_pressure(&sensor)?;
                    let accepted = if temperature {
                        device.set_temperature(value)
                    } else {
                        device.set_pressure(value)
                    };
                    if !accepted {
                        eprintln!("{sensor}: out-of-range input {value} ignored");
                    }
                }
                Action::ReadPressure { node, sensor } => {
                    let pressure = simulation.nodes[node]
                        .machine
                        .bus
                        .find_pressure(&sensor)?
                        .pressure();
                    writeln!(output, "{sensor} pressure: {pressure}").map_err(|e| e.to_string())?;
                }
                Action::IlluminanceFeed { node, sensor, file } => {
                    simulation.nodes[node]
                        .machine
                        .bus
                        .feed_illuminance(&sensor, &file)?;
                }
                Action::Illuminance {
                    node,
                    sensor,
                    value,
                } => {
                    simulation.nodes[node]
                        .machine
                        .bus
                        .set_illuminance(&sensor, value)?;
                }
                Action::ImuResd {
                    node,
                    sensor,
                    file,
                    angular_rate,
                } => {
                    simulation.nodes[node]
                        .machine
                        .bus
                        .find_imu(&sensor)?
                        .feed_resd(&file, angular_rate)?;
                }
                Action::ImuFeed { node, sensor, file } => {
                    simulation.nodes[node]
                        .machine
                        .bus
                        .feed_imu_samples(&sensor, &file)?;
                }
                Action::ImuInput {
                    node,
                    sensor,
                    axis,
                    value,
                } => {
                    simulation.nodes[node]
                        .machine
                        .bus
                        .find_imu(&sensor)?
                        .set_input(axis, value);
                }
                Action::TraceRcc { node } => {
                    simulation.nodes[node].machine.bus.trace_rcc()?;
                }
                Action::RccWrite {
                    node,
                    offset,
                    value,
                } => {
                    let bus = &mut simulation.nodes[node].machine.bus;
                    let address = bus
                        .rcc_base()?
                        .checked_add(offset)
                        .ok_or("RCC offset overflow")?;
                    bus.write(address, 4, value, &mut output)
                        .map_err(|e| e.to_string())?;
                }
                Action::TimerProperty {
                    node,
                    name,
                    frequency,
                } => {
                    let (hz, enabled) =
                        simulation.nodes[node].machine.bus.timer_properties(&name)?;
                    if frequency {
                        writeln!(output, "{hz}").map_err(|e| e.to_string())?;
                    } else {
                        writeln!(output, "{enabled}").map_err(|e| e.to_string())?;
                    }
                }
                Action::TimerInput {
                    node,
                    name,
                    channel,
                    high,
                } => {
                    simulation.nodes[node]
                        .machine
                        .bus
                        .set_timer_input(&name, channel, high)?;
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
                Action::ReadMemoryBytes {
                    node,
                    address,
                    count,
                } => {
                    let node = &simulation.nodes[node];
                    let bytes = node.machine.bus.read_memory_bytes(address, count)?;
                    writeln!(
                        output,
                        "{}: sysbus[{address:#010x}] = {bytes:02x?}",
                        node.name
                    )
                    .map_err(|error| format!("sysbus read output: {error}"))?;
                }
                Action::WriteMemoryBytes {
                    node,
                    address,
                    bytes,
                } => {
                    simulation.nodes[node]
                        .machine
                        .bus
                        .write_memory_bytes(address, &bytes)?;
                }
                Action::ObserveUart { node, name } => {
                    simulation.nodes[node]
                        .machine
                        .bus
                        .stm32_usart_mut(&name)?
                        .observed = Some(Default::default());
                }
                Action::FailOnUart {
                    node,
                    name,
                    pattern,
                } => {
                    simulation.nodes[node]
                        .machine
                        .bus
                        .stm32_usart_mut(&name)?
                        .observed
                        .as_mut()
                        .ok_or("start a UART observer before registering failures")?
                        .failing
                        .push(pattern);
                }
                Action::WriteUartLine { node, name, text } => {
                    simulation.nodes[node]
                        .machine
                        .bus
                        .write_stm32_uart_line(&name, text.into_bytes())?;
                }
                Action::WaitUart {
                    node,
                    name,
                    timeout,
                    unfinished,
                    patterns,
                } => {
                    ran = true;
                    let mut scanned = None;
                    let mut matched = false;
                    simulation.advance_until(Run::Rounds(timeout), limit, &mut output, |sim| {
                        let bytes = sim.nodes[node]
                            .machine
                            .bus
                            .stm32_usart_mut(&name)?
                            .observed
                            .as_mut()
                            .ok_or("start a UART observer before waiting")?;
                        let bytes = &mut bytes.bytes;
                        if scanned == Some(bytes.len()) {
                            return Ok(false);
                        }
                        scanned = Some(bytes.len());
                        matched = consume_uart_match(bytes, &patterns, unfinished);
                        Ok(matched)
                    })?;
                    if !matched {
                        return Err(format!(
                            "UART {name} did not match {patterns:?} within {timeout} rounds"
                        ));
                    }
                    writeln!(
                        output,
                        "UART {name} matched {patterns:?} at round {}",
                        simulation.rounds
                    )
                    .map_err(|e| e.to_string())?;
                }
                Action::WaitLed {
                    node,
                    name,
                    timeout,
                    state,
                } => {
                    ran = true;
                    let mut matched = false;
                    simulation.advance_until(Run::Rounds(timeout), limit, &mut output, |sim| {
                        matched = sim.led_state(node, &name)? == state;
                        Ok(matched)
                    })?;
                    if !matched {
                        return Err(format!(
                            "LED {name} did not reach {state} within {timeout} rounds"
                        ));
                    }
                    writeln!(
                        output,
                        "LED {name} reached {state} at round {}",
                        simulation.rounds
                    )
                    .map_err(|e| e.to_string())?;
                }
                Action::BlinkLed {
                    node,
                    name,
                    duration,
                    on,
                    off,
                } => {
                    ran = true;
                    let start = simulation.rounds;
                    let mut level = simulation.led_state(node, &name)?;
                    let mut last_edge: Option<u64> = None;
                    let mut intervals = 0;
                    simulation.advance_until(Run::Rounds(duration), limit, &mut output, |sim| {
                        let current = sim.led_state(node, &name)?;
                        if current != level {
                            if let Some(previous) = last_edge {
                                let elapsed = sim.rounds - previous;
                                let expected = if level { on } else { off };
                                if u128::from(elapsed.abs_diff(expected)) * 20 > u128::from(expected) {
                                    return Err(format!("LED {name} held {level} for {elapsed} rounds, expected {expected} ±5%"));
                                }
                                intervals += 1;
                            }
                            level = current;
                            last_edge = Some(sim.rounds);
                        }
                        Ok(false)
                    })?;
                    if simulation.rounds - start != duration || intervals == 0 {
                        return Err(format!(
                            "LED {name} blinking window incomplete or no complete interval"
                        ));
                    }
                    writeln!(output, "LED {name} blinked within tolerance ({intervals} complete intervals over {duration} rounds)")
                        .map_err(|e| e.to_string())?;
                }
                Action::Run(run) => {
                    ran = true;
                    if simulation.advance(run, limit, &mut output)? {
                        return Ok(true);
                    }
                }
            }
        }
        if ran || monitor_only {
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
    fn boot(
        node: NodeBoot,
        root: Option<&Path>,
        assets: Option<&Path>,
        monitor_only: bool,
    ) -> Result<Self, String> {
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
        let mut image_path = None;
        if let Some(value) = boot.image {
            let (file, path) = open(&value)?;
            image::load(&mut machine, file, false)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            image_path = Some(path);
        }
        for (value, address) in boot.binaries {
            let (file, path) = open(&value)?;
            image::load_binary(&mut machine, file, address)
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        for (cpu, address) in boot.boot_pcs {
            machine.set_named_entry(&cpu, address)?;
        }
        zephyr_boot::configure(&mut machine, &boot.zephyr, image_path.as_deref())?;
        if !monitor_only {
            machine
                .validate_entry()
                .map_err(|e| format!("{name}: {e}"))?;
        }
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
        self.advance_until(run, limit, output, |_| Ok(false))
    }

    fn led_state(&self, node: usize, name: &str) -> Result<bool, String> {
        self.nodes[node]
            .machine
            .bus
            .gpio_led_states()
            .find_map(|(registered, state)| (registered == name).then_some(state))
            .ok_or_else(|| format!("unknown GPIO LED {name:?}"))
    }

    fn advance_until(
        &mut self,
        run: Run,
        limit: u64,
        output: &mut impl Write,
        mut done: impl FnMut(&mut Self) -> Result<bool, String>,
    ) -> Result<bool, String> {
        if done(self)? {
            return Ok(false);
        }
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
            if done(self)? || !active {
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

fn consume_uart_match(bytes: &mut Vec<u8>, patterns: &[String], unfinished: bool) -> bool {
    let mut consumed = 0;
    for line in bytes.split_inclusive(|&byte| byte == b'\n') {
        let complete = line.ends_with(b"\n");
        if !complete && !unfinished {
            break;
        }
        if patterns.iter().any(|pattern| {
            line.windows(pattern.len())
                .any(|window| window == pattern.as_bytes())
        }) {
            bytes.drain(..consumed + line.len());
            return true;
        }
        consumed += line.len();
    }
    false
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
    fn uart_matches_complete_lines_unless_unfinished_is_requested() {
        let mut bytes = b"earlier\r\nBooting".to_vec();
        let boot = ["Booting".to_owned()];
        assert!(!consume_uart_match(&mut bytes, &boot, false));
        assert!(consume_uart_match(&mut bytes, &boot, true));
        assert!(bytes.is_empty());
        bytes.extend(b"noise\nLED state: OFF\r\nPress the button\r\n");
        assert!(consume_uart_match(
            &mut bytes,
            &["LED state: ON".to_owned(), "LED state: OFF".to_owned()],
            false
        ));
        assert_eq!(bytes, b"Press the button\r\n");
    }

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
