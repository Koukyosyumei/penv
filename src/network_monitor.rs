use std::path::{Path, PathBuf};

use crate::{
    monitor::{self, Boot, Variables},
    simulation::{Action, Connection, Run},
};

pub struct NodeBoot {
    pub name: String,
    pub boot: Boot,
    pub source: PathBuf,
}

#[derive(Default)]
pub struct Plan {
    pub nodes: Vec<NodeBoot>,
    pub actions: Vec<Action>,
    selected: Option<usize>,
    started: bool,
    configured: usize,
}

fn byte_array(bytes: &[&str]) -> Result<Vec<u8>, String> {
    let bytes = bytes.join(" ");
    let list = bytes
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .ok_or("expected bracketed SystemBus byte array")?;
    Ok(if list.trim().is_empty() {
        Vec::new()
    } else {
        list.split(',')
            .map(|value| {
                let value = crate::platform::number(value.trim())?;
                u8::try_from(value).map_err(|_| "SystemBus byte value exceeds 255".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?
    })
}

impl Plan {
    pub fn load(path: &Path, root: &Path) -> Result<Self, String> {
        let mut plan = Self::default();
        monitor::walk(path, root, &mut |text, variables, source| {
            plan.command(text, variables, source, root)
        })?;
        if plan.nodes.is_empty() {
            return Err(format!("{}: expected machine creation", path.display()));
        }
        for node in &plan.nodes {
            if node.boot.platform.is_none()
                || (!plan.is_monitor_only()
                    && node.boot.image.is_none()
                    && node.boot.binaries.is_empty())
            {
                return Err(format!(
                    "{}: machine {:?} requires a platform and firmware",
                    node.source.display(),
                    node.name
                ));
            }
        }
        Ok(plan)
    }

    pub fn is_single_boot(&self) -> bool {
        self.nodes.len() == 1 && self.actions.len() == 1
    }

    pub fn is_monitor_only(&self) -> bool {
        self.actions.len() > self.nodes.len()
            && self
                .nodes
                .iter()
                .any(|node| node.boot.image.is_none() && node.boot.binaries.is_empty())
            && !self.actions.iter().any(|action| {
                matches!(
                    action,
                    Action::Run(_)
                        | Action::WaitUart { .. }
                        | Action::WaitLed { .. }
                        | Action::BlinkLed { .. }
                )
            })
    }

    fn command(
        &mut self,
        text: &str,
        vars: &Variables,
        source: &Path,
        root: &Path,
    ) -> Result<(), String> {
        let words = monitor::words(text)?;
        let resolve = |value| monitor::resolve(vars, value).map(|name| name.trim_matches('"'));
        match words.as_slice() {
            [] | ["using", "sysbus"] => {}
            ["mach" | "machine", "create", name @ ..] if name.len() <= 1 => {
                let name = match name.first() {
                    Some(value) => resolve(*value)?.to_owned(),
                    None => format!("machine-{}", self.nodes.len()),
                };
                if name.is_empty() || self.nodes.iter().any(|node| node.name == name) {
                    return Err(format!("invalid or duplicate machine name {name:?}"));
                }
                let id = self.nodes.len();
                self.nodes.push(NodeBoot {
                    name,
                    boot: Boot::default(),
                    source: source.into(),
                });
                self.selected = Some(id);
                self.actions.push(Action::Activate(id));
            }
            ["mach" | "machine", "set", selector] => {
                let value = resolve(*selector)?;
                let id = self
                    .nodes
                    .iter()
                    .position(|node| node.name == value)
                    .or_else(|| {
                        value
                            .parse::<usize>()
                            .ok()
                            .filter(|&id| id < self.nodes.len())
                    });
                self.selected = Some(id.ok_or_else(|| format!("unknown machine {value:?}"))?);
            }
            ["mach" | "machine", "clear"] => self.selected = None,
            ["emulation", "CreateSwitch", switch] => {
                self.actions
                    .push(Action::CreateSwitch(resolve(*switch)?.to_owned()));
            }
            ["connector", verb @ ("Connect" | "Disconnect"), nic, switch] => {
                self.connection(
                    if *verb == "Connect" {
                        Connection::Connect
                    } else {
                        Connection::Disconnect
                    },
                    resolve(*nic)?,
                    resolve(*switch)?,
                )?;
            }
            [
                switch,
                verb @ ("EnablePromiscuousMode" | "DisablePromiscuousMode"),
                nic,
            ] => {
                self.connection(
                    Connection::Promiscuous(*verb == "EnablePromiscuousMode"),
                    resolve(*nic)?,
                    resolve(*switch)?,
                )?;
            }
            [
                sensor,
                command @ ("FeedShuntVoltageSamplesFromRESD" | "FeedBusVoltageSamplesFromRESD"),
                file,
                channel,
            ] if sensor.starts_with("sysbus.") => {
                let node = self.monitor_node()?;
                let path = resolve(*file)?
                    .strip_prefix('@')
                    .ok_or("expected @RESD path")?;
                let file = monitor::asset_path(path.trim_matches('"'), source, root, None)?;
                let channel = resolve(*channel)?
                    .parse()
                    .map_err(|_| "invalid RESD channel")?;
                self.actions.push(Action::VoltageFeed {
                    node,
                    sensor: (*sensor).into(),
                    file,
                    input: usize::from(*command == "FeedBusVoltageSamplesFromRESD"),
                    channel,
                });
            }
            [
                sensor,
                command @ ("FeedTemperatureSamplesFromRESD" | "FeedPressureSamplesFromRESD"),
                file,
            ] if sensor.starts_with("sysbus.") => {
                let node = self.monitor_node()?;
                let path = resolve(*file)?
                    .strip_prefix('@')
                    .ok_or("expected @RESD path")?;
                let file = monitor::asset_path(path.trim_matches('"'), source, root, None)?;
                self.actions.push(Action::PressureFeed {
                    node,
                    sensor: (*sensor).into(),
                    temperature: *command == "FeedTemperatureSamplesFromRESD",
                    file,
                });
            }
            [sensor, property @ ("Pressure" | "Temperature"), value]
                if sensor.starts_with("sysbus.") =>
            {
                let node = self.monitor_node()?;
                let value = resolve(*value)?
                    .parse()
                    .map_err(|_| "invalid pressure or temperature")?;
                self.actions.push(Action::PressureInput {
                    node,
                    sensor: (*sensor).into(),
                    temperature: *property == "Temperature",
                    value,
                });
            }
            [sensor, "Pressure"] if sensor.starts_with("sysbus.") => {
                let node = self.monitor_node()?;
                self.actions.push(Action::ReadPressure {
                    node,
                    sensor: (*sensor).into(),
                });
            }
            [sensor, "FeedMagneticSamplesFromRESD", file] if sensor.starts_with("sysbus.") => {
                let node = self.monitor_node()?;
                let path = resolve(*file)?
                    .strip_prefix('@')
                    .ok_or("expected @RESD path")?;
                let file = monitor::asset_path(path.trim_matches('"'), source, root, None)?;
                self.actions.push(Action::MagneticFeed {
                    node,
                    sensor: (*sensor).into(),
                    file,
                });
            }
            [sensor, "FeedIlluminanceSamplesFromRESD", file] if sensor.starts_with("sysbus.") => {
                let node = self.monitor_node()?;
                let value = resolve(*file)?;
                let value = value.strip_prefix('@').ok_or("expected @RESD path")?;
                let file = monitor::asset_path(value.trim_matches('"'), source, root, None)?;
                self.actions.push(Action::IlluminanceFeed {
                    node,
                    sensor: (*sensor).into(),
                    file,
                });
            }
            [sensor, "Illuminance", value] if sensor.starts_with("sysbus.") => {
                let node = self.monitor_node()?;
                let value = resolve(*value)?
                    .parse()
                    .map_err(|_| "invalid illuminance")?;
                self.actions.push(Action::Illuminance {
                    node,
                    sensor: (*sensor).into(),
                    value,
                });
            }
            [
                sensor,
                command @ ("FeedAccelerationSamplesFromRESD" | "FeedAngularRateSamplesFromRESD"),
                file,
            ] if sensor.starts_with("sysbus.") => {
                let node = self.monitor_node()?;
                let path = resolve(*file)?
                    .strip_prefix('@')
                    .ok_or("expected @RESD path")?;
                let file = monitor::asset_path(path.trim_matches('"'), source, root, None)?;
                self.actions.push(Action::ImuResd {
                    node,
                    sensor: (*sensor).into(),
                    file,
                    angular_rate: *command == "FeedAngularRateSamplesFromRESD",
                });
            }
            [sensor, "FeedSamplesFromCsv", file] if sensor.starts_with("sysbus.") => {
                let node = self.monitor_node()?;
                let value = resolve(*file)?;
                let value = value.strip_prefix('@').ok_or("expected @CSV path")?;
                let file = monitor::asset_path(value.trim_matches('"'), source, root, None)?;
                self.actions.push(Action::ImuFeed {
                    node,
                    sensor: (*sensor).into(),
                    file,
                });
            }
            [sensor, property, value]
                if sensor.starts_with("sysbus.")
                    && let Some(axis) = crate::lsm6dso::input_axis(property) =>
            {
                let node = self.monitor_node()?;
                let value = resolve(*value)?
                    .parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite())
                    .ok_or("expected finite LSM6DSO input value")?;
                self.actions.push(Action::ImuInput {
                    node,
                    sensor: (*sensor).into(),
                    axis,
                    value,
                });
            }
            ["sysbus", "LogPeripheralAccess", "rcc"] => {
                let node = self.monitor_node()?;
                self.actions.push(Action::TraceRcc { node });
            }
            ["rcc", "WriteDoubleWord", offset, value] => {
                let node = self.monitor_node()?;
                self.actions.push(Action::RccWrite {
                    node,
                    offset: crate::platform::number(resolve(*offset)?)?,
                    value: crate::platform::number(resolve(*value)?)?,
                });
            }
            [name, "Frequency"] | ["sysbus", "IsPeripheralEnabled", name] => {
                let node = self.monitor_node()?;
                self.actions.push(Action::TimerProperty {
                    node,
                    name: resolve(*name)?.into(),
                    frequency: words.len() == 2,
                });
            }
            [timer, "OnGPIO", channel, level] => {
                let node = self.monitor_node()?;
                let channel = crate::platform::irq_index(resolve(*channel)?, 4)?;
                let high = resolve(*level)?
                    .parse::<bool>()
                    .map_err(|_| "expected GPIO level")?;
                self.actions.push(Action::TimerInput {
                    node,
                    name: timer.strip_prefix("sysbus.").unwrap_or(timer).to_owned(),
                    channel,
                    high,
                });
            }
            ["sysbus", "ReadBytes", address, count, "true" | "True"] => {
                let node = self.monitor_node()?;
                let address = crate::platform::number(resolve(*address)?)?;
                let count = crate::platform::number(resolve(*count)?)? as usize;
                self.actions.push(Action::ReadMemoryBytes {
                    node,
                    address,
                    count,
                });
            }
            ["sysbus", "FindBytes", args @ ..] => {
                let node = self.monitor_node()?;
                let end_of_array = args
                    .iter()
                    .position(|word| word.ends_with(']'))
                    .ok_or("expected bracketed SystemBus byte array")?;
                let (pattern, bounds) = args.split_at(end_of_array + 1);
                let (start, end) = match bounds {
                    [] => (0, u32::MAX),
                    [start] => (crate::platform::number(resolve(*start)?)?, u32::MAX),
                    [start, end] => (
                        crate::platform::number(resolve(*start)?)?,
                        crate::platform::number(resolve(*end)?)?,
                    ),
                    _ => return Err("expected at most two SystemBus search bounds".into()),
                };
                let pattern = byte_array(pattern)?;
                self.actions.push(Action::FindMemoryBytes {
                    node,
                    pattern,
                    start,
                    end,
                });
            }
            ["sysbus", "WriteBytes", bytes @ .., address, "true" | "True"] => {
                let node = self.monitor_node()?;
                let address = crate::platform::number(resolve(*address)?)?;
                let bytes = byte_array(bytes)?;
                self.actions.push(Action::WriteMemoryBytes {
                    node,
                    address,
                    bytes,
                });
            }
            ["sysbus", "WriteDoubleWord", address, value] => {
                let node = self.monitor_node()?;
                let address = crate::platform::number(resolve(*address)?)?;
                let value = crate::platform::number(resolve(*value)?)?;
                self.actions.push(Action::WriteWord {
                    node,
                    address,
                    value,
                });
            }
            ["ltdc", "FramesPerVirtualSecond", value] => {
                let node = self.monitor_node()?;
                let fps = crate::platform::number(resolve(*value)?)?;
                self.actions.push(Action::DisplayRate { node, fps });
            }
            [touch, "MoveTo", x, y] if touch.starts_with("sysbus.") => {
                let coordinate = |value| -> Result<i32, String> {
                    resolve(value)?
                        .parse()
                        .map_err(|_| "invalid pointer coordinate".into())
                };
                self.actions.push(Action::Pointer {
                    node: self.monitor_node()?,
                    name: (*touch).into(),
                    event: crate::ft5336::PointerEvent::Move(coordinate(*x)?, coordinate(*y)?),
                });
            }
            [button, verb @ ("Press" | "Release")] if button.contains('.') => {
                let node = self.monitor_node()?;
                let name = button.strip_prefix("sysbus.").unwrap_or(button).to_owned();
                self.actions.push(Action::Button {
                    node,
                    name,
                    pressed: *verb == "Press",
                });
            }
            [
                "sysbus",
                operation @ ("ReadByte" | "ReadDoubleWord"),
                address,
            ] => {
                let node = self.monitor_node()?;
                let address = crate::platform::number(resolve(*address)?)?;
                let byte = *operation == "ReadByte";
                self.actions.push(Action::ReadScalar {
                    node,
                    address,
                    byte,
                });
            }
            ["tester", "ObserveUart", name] => {
                let node = self.monitor_node()?;
                let name = name.strip_prefix("sysbus.").unwrap_or(name).to_owned();
                self.actions.push(Action::ObserveUart { node, name });
            }
            ["tester", "FailOnUart" | "WriteUartLine", name, text] => {
                let node = self.monitor_node()?;
                let name = name.strip_prefix("sysbus.").unwrap_or(name).to_owned();
                let text = resolve(*text)?.to_owned();
                if words[1] == "FailOnUart" {
                    if text.is_empty() || text.contains(['\r', '\n']) {
                        return Err("expected nonempty single-line UART failure text".into());
                    }
                    self.actions.push(Action::FailOnUart {
                        node,
                        name,
                        pattern: text,
                    });
                } else {
                    self.actions
                        .push(Action::WriteUartLine { node, name, text });
                }
            }
            ["tester", "WaitUart", name, timeout, mode, patterns @ ..] if !patterns.is_empty() => {
                let node = self.monitor_node()?;
                let unfinished = match *mode {
                    "line" => false,
                    "partial" => true,
                    _ => return Err("expected UART match mode line or partial".into()),
                };
                let patterns = patterns
                    .iter()
                    .map(|p| resolve(*p).map(str::to_owned))
                    .collect::<Result<Vec<_>, _>>()?;
                if patterns.iter().any(String::is_empty) {
                    return Err("expected nonempty UART patterns".into());
                }
                self.configured = self.nodes.len();
                self.actions.push(Action::WaitUart {
                    node,
                    unfinished,
                    patterns,
                    name: name.strip_prefix("sysbus.").unwrap_or(name).to_owned(),
                    timeout: rounds(resolve(*timeout)?)?,
                });
            }
            ["tester", "WaitLed", name, timeout, state] => {
                let node = self.monitor_node()?;
                self.configured = self.nodes.len();
                self.actions.push(Action::WaitLed {
                    node,
                    name: (*name).into(),
                    timeout: rounds(resolve(*timeout)?)?,
                    state: state
                        .parse()
                        .map_err(|_| "expected LED state true or false")?,
                });
            }
            ["tester", "BlinkLed", name, duration, on, off] => {
                let node = self.monitor_node()?;
                let duration = rounds(resolve(*duration)?)?;
                let on = rounds(resolve(*on)?)?;
                let off = rounds(resolve(*off)?)?;
                if [duration, on, off].contains(&0) {
                    return Err("expected positive LED durations".into());
                }
                self.configured = self.nodes.len();
                self.actions.push(Action::BlinkLed {
                    node,
                    name: (*name).into(),
                    duration,
                    on,
                    off,
                });
            }
            ["start" | "s"] => self.run(Run::Remaining)?,
            ["emulation", "RunFor", duration] => {
                self.run(Run::Rounds(rounds(resolve(*duration)?)?))?
            }
            _ => {
                let id = self
                    .selected
                    .ok_or_else(|| format!("no machine selected for {text:?}"))?;
                if id < self.configured {
                    return Err("boot commands must precede execution of their machine".into());
                }
                monitor::execute(
                    text,
                    vars,
                    &mut true,
                    &mut self.nodes[id].boot,
                    source,
                    root,
                )?;
            }
        }
        Ok(())
    }

    fn monitor_node(&self) -> Result<usize, String> {
        if self.started {
            return Err("monitor operations must precede unbounded start".into());
        }
        self.selected
            .ok_or_else(|| "select a machine before accessing peripherals".into())
    }

    fn connection(&mut self, operation: Connection, nic: &str, switch: &str) -> Result<(), String> {
        let node = self
            .selected
            .ok_or("select a machine before addressing a NIC")?;
        let nic = nic.strip_prefix("sysbus.").unwrap_or(nic).to_owned();
        self.actions.push(Action::Connect {
            operation,
            node,
            nic,
            switch: switch.to_owned(),
        });
        Ok(())
    }

    fn run(&mut self, run: Run) -> Result<(), String> {
        if self.nodes.is_empty() {
            return Err("create a machine before starting execution".into());
        }
        if self.started {
            return Err("unbounded start must be the final execution command".into());
        }
        self.started = matches!(run, Run::Remaining);
        self.configured = self.nodes.len();
        self.actions.push(Action::Run(run));
        Ok(())
    }
}

// The scheduler uses one virtual microsecond per round.
fn rounds(duration: &str) -> Result<u64, String> {
    let (seconds, fraction) = duration.split_once('.').unwrap_or((duration, ""));
    if seconds.is_empty()
        || !seconds.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(format!("expected a duration in seconds, got {duration:?}"));
    }
    let denominator = u64::from(crate::machine::VIRTUAL_STEPS_PER_SECOND);
    let micros = fraction
        .bytes()
        .take(6)
        .enumerate()
        .map(|(index, byte)| u64::from(byte - b'0') * 10u64.pow(5 - index as u32))
        .sum::<u64>();
    seconds
        .parse::<u64>()
        .ok()
        .and_then(|seconds| seconds.checked_mul(denominator))
        .and_then(|seconds| seconds.checked_add(micros))
        .ok_or_else(|| format!("duration is too large: {duration:?}"))
}
