use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Port {
    pub node: usize,
    pub nic: usize,
}

pub struct Switch {
    ports: Vec<(Port, bool)>,
    learned: HashMap<[u8; 6], Port>,
}

impl Switch {
    pub fn new() -> Self {
        Self {
            ports: Vec::new(),
            learned: HashMap::new(),
        }
    }

    pub fn connect(&mut self, port: Port) -> Result<(), String> {
        if self.ports.iter().any(|&(existing, _)| existing == port) {
            return Err("Ethernet port is already connected to this switch".into());
        }
        self.ports.push((port, false));
        Ok(())
    }

    pub fn disconnect(&mut self, port: Port) -> Result<(), String> {
        let index = self.index(port)?;
        self.ports.remove(index);
        self.learned.retain(|_, owner| *owner != port);
        Ok(())
    }

    pub fn promiscuous(&mut self, port: Port, enabled: bool) -> Result<(), String> {
        let index = self.index(port)?;
        self.ports[index].1 = enabled;
        Ok(())
    }

    fn index(&self, port: Port) -> Result<usize, String> {
        self.ports
            .iter()
            .position(|&(existing, _)| existing == port)
            .ok_or_else(|| "Ethernet port is disconnected from this switch".into())
    }

    pub fn recipients(&mut self, sender: Port, frame: &[u8]) -> impl Iterator<Item = Port> + '_ {
        let destination = <[u8; 6]>::try_from(&frame[..6]).unwrap();
        let source = <[u8; 6]>::try_from(&frame[6..12]).unwrap();
        if source[0] & 1 == 0 && (self.learned.len() < 4096 || self.learned.contains_key(&source)) {
            self.learned.insert(source, sender);
        }
        let target = self.learned.get(&destination).copied();
        self.ports.iter().filter_map(move |&(port, promiscuous)| {
            (port != sender && (promiscuous || target.is_none_or(|target| port == target)))
                .then_some(port)
        })
    }
}
