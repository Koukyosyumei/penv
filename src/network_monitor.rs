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
