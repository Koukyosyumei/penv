//! A local Ethernet switch for MACs using the loopback datagram transport.
use std::{
    collections::HashMap,
    io::{self, Write},
    net::{Ipv4Addr, UdpSocket},
};

const MAX_FRAME: usize = 16_379;
const MAX_LEARNED: usize = 1024;

#[derive(Default)]
struct ForwardingTable {
    ports: HashMap<[u8; 6], u16>,
}

impl ForwardingTable {
    fn destination(&mut self, ingress: u16, frame: &[u8]) -> Option<u16> {
        let destination: [u8; 6] = frame[..6].try_into().unwrap();
        let source: [u8; 6] = frame[6..12].try_into().unwrap();
        let target = self.ports.get(&destination).copied();
        if source[0] & 1 == 0
            && (self.ports.len() < MAX_LEARNED || self.ports.contains_key(&source))
        {
            self.ports.insert(source, ingress);
        }
        target
    }
}

pub fn run(local: u16, guests: &[u16]) -> io::Result<()> {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, local))?;
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "Listening on {}", socket.local_addr()?)?;
    stdout.flush()?;
    drop(stdout);

    let mut table = ForwardingTable::default();
    let mut buffer = [0; MAX_FRAME + 1];
    loop {
        let (length, sender) = match socket.recv_from(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        let ingress = sender.port();
        if !sender.ip().is_loopback()
            || !guests.contains(&ingress)
            || !(14..=MAX_FRAME).contains(&length)
        {
            continue;
        }
        let frame = &buffer[..length];
        let destination = table.destination(ingress, frame);
        for &port in guests {
            if destination.map_or(port != ingress, |target| target == port) {
                socket.send_to(frame, (Ipv4Addr::LOCALHOST, port))?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learns_unicast_ports_and_floods_unknown_and_group_destinations() {
        let mut table = ForwardingTable::default();
        let first = [2, 0, 0, 0, 0, 1];
        let second = [2, 0, 0, 0, 0, 2];
        let broadcast = [0xff; 6];
        let frame = |destination: [u8; 6], source: [u8; 6]| {
            let mut bytes = [0; 14];
            bytes[..6].copy_from_slice(&destination);
            bytes[6..12].copy_from_slice(&source);
            bytes
        };
        assert_eq!(table.destination(1, &frame(second, first)), None);
        assert_eq!(table.destination(2, &frame(first, second)), Some(1));
        assert_eq!(table.destination(1, &frame(second, first)), Some(2));
        assert_eq!(table.destination(3, &frame(broadcast, first)), None);
        assert_eq!(table.destination(2, &frame(first, second)), Some(3));
        assert_eq!(table.destination(2, &frame(second, second)), Some(2));
        assert_eq!(table.destination(1, &frame(first, first)), Some(3));
        assert_eq!(table.destination(1, &frame(first, first)), Some(1));
    }
}

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
