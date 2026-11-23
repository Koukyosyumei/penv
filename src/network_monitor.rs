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
                || (node.boot.image.is_none() && node.boot.binaries.is_empty())
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
            [button, verb @ ("Press" | "Release")] if button.contains('.') => {
                let node = self.monitor_node()?;
                let name = button.strip_prefix("sysbus.").unwrap_or(button).to_owned();
                self.actions.push(Action::Button {
                    node,
                    name,
                    pressed: *verb == "Press",
                });
            }
            ["sysbus", "ReadDoubleWord", address] => {
                let node = self.monitor_node()?;
                let address = crate::platform::number(resolve(*address)?)?;
                self.actions.push(Action::ReadWord { node, address });
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
