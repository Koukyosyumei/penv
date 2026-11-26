use std::net::Ipv4Addr;

const SERVER_MAC: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
const BLOCK: usize = 512;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Peer {
    mac: [u8; 6],
    ip: [u8; 4],
    port: u16,
}

#[derive(Clone)]
struct Transfer {
    peer: Peer,
    block: u16,
    offset: usize,
}

#[derive(Clone)]
pub struct Server {
    ip: [u8; 4],
    port: u16,
    name: String,
    contents: Vec<u8>,
    transfer: Option<Transfer>,
}

impl Server {
    pub fn new(ip: Ipv4Addr, port: u16, name: String, contents: Vec<u8>) -> Self {
        Self {
            ip: ip.octets(),
            port,
            name,
            contents,
            transfer: None,
        }
    }

    pub fn reply(&mut self, frame: &[u8]) -> Option<Vec<u8>> {
        let preamble = frame.starts_with(&[0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0xd5]);
        let frame = if preamble { &frame[8..] } else { frame };
        let mac: [u8; 6] = frame.get(6..12)?.try_into().ok()?;
        let response = match read_u16(frame, 12)? {
            0x0806 => self.arp(frame.get(14..)?, mac)?,
            0x0800 => self.ipv4(frame.get(14..)?, mac)?,
            _ => return None,
        };
        let mut wire = Vec::with_capacity(response.len().max(60) + 12);
        if preamble {
            wire.extend_from_slice(&[0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0xd5]);
        }
        let start = wire.len();
        wire.extend_from_slice(&response);
        wire.resize(wire.len().max(start + 60), 0);
        let crc = crc32(&wire[start..]);
        wire.extend_from_slice(&crc.to_le_bytes());
        Some(wire)
    }

    fn arp(&self, arp: &[u8], mac: [u8; 6]) -> Option<Vec<u8>> {
        if arp.get(..8)? != [0, 1, 8, 0, 6, 4, 0, 1] || arp.get(24..28)? != self.ip {
            return None;
        }
        let mut reply = ethernet(mac, 0x0806);
        reply.extend_from_slice(&[0, 1, 8, 0, 6, 4, 0, 2]);
        reply.extend_from_slice(&SERVER_MAC);
        reply.extend_from_slice(&self.ip);
        reply.extend_from_slice(arp.get(8..18)?);
        Some(reply)
    }

    fn ipv4(&mut self, ip: &[u8], mac: [u8; 6]) -> Option<Vec<u8>> {
        let first = *ip.first()?;
        let header = usize::from(first & 15) * 4;
        let end = usize::from(read_u16(ip, 2)?);
        if first >> 4 != 4
            || header < 20
            || *ip.get(9)? != 17
            || ip.get(16..20)? != self.ip
            || read_u16(ip, 6)? & 0x3fff != 0
        {
            return None;
        }
        let udp = ip.get(header..end)?;
        if read_u16(udp, 2)? != self.port {
            return None;
        }
        let peer = Peer {
            mac,
            ip: ip.get(12..16)?.try_into().ok()?,
            port: read_u16(udp, 0)?,
        };
        let data = udp.get(8..usize::from(read_u16(udp, 4)?))?;
        let payload = self.tftp(peer, data)?;
        Some(self.wrap_udp(peer, &payload))
    }

    fn tftp(&mut self, peer: Peer, request: &[u8]) -> Option<Vec<u8>> {
        match read_u16(request, 0)? {
            1 => {
                let mut fields = request.get(2..)?.split(|&byte| byte == 0);
                if fields.next()? != self.name.as_bytes() {
                    return Some(b"\0\x05\0\x01File not found\0".to_vec());
                }
                if !fields.next()?.eq_ignore_ascii_case(b"octet") {
                    return Some(b"\0\x05\0\x04Only octet mode is supported\0".to_vec());
                }
                self.transfer = Some(Transfer {
                    peer,
                    block: 1,
                    offset: 0,
                });
            }
            4 => {
                let ack = read_u16(request, 2)?;
                let transfer = self
                    .transfer
                    .as_mut()
                    .filter(|transfer| transfer.peer == peer)?;
                if ack == transfer.block {
                    if self.contents.len().saturating_sub(transfer.offset) < BLOCK {
                        self.transfer = None;
                        return None;
                    }
                    transfer.offset += BLOCK;
                    transfer.block = transfer.block.wrapping_add(1);
                } else if ack != transfer.block.wrapping_sub(1) {
                    return None;
                }
            }
            _ => return None,
        }
        let transfer = self.transfer.as_ref()?;
        let end = (transfer.offset + BLOCK).min(self.contents.len());
        let mut response = Vec::with_capacity(4 + end - transfer.offset);
        response.extend_from_slice(&[0, 3]);
        response.extend_from_slice(&transfer.block.to_be_bytes());
        response.extend_from_slice(&self.contents[transfer.offset..end]);
        Some(response)
    }

    fn wrap_udp(&self, peer: Peer, payload: &[u8]) -> Vec<u8> {
        let mut response = ethernet(peer.mac, 0x0800);
        let length = (payload.len() + 28) as u16;
        let mut ip = [0u8; 20];
        ip[0] = 0x45;
        ip[2..4].copy_from_slice(&length.to_be_bytes());
        ip[8] = 64;
        ip[9] = 17;
        ip[12..16].copy_from_slice(&self.ip);
        ip[16..20].copy_from_slice(&peer.ip);
        let mut sum = ip
            .as_chunks::<2>()
            .0
            .iter()
            .map(|word| u32::from(u16::from_be_bytes(*word)))
            .sum::<u32>();
        while sum > 0xffff {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        ip[10..12].copy_from_slice(&(!(sum as u16)).to_be_bytes());
        response.extend_from_slice(&ip);
        response.extend_from_slice(&self.port.to_be_bytes());
        response.extend_from_slice(&peer.port.to_be_bytes());
        response.extend_from_slice(&(length - 20).to_be_bytes());
        response.extend_from_slice(&[0, 0]);
        response.extend_from_slice(payload);
        response
    }
}

fn ethernet(destination: [u8; 6], protocol: u16) -> Vec<u8> {
    let mut frame = Vec::new();
    frame.extend_from_slice(&destination);
    frame.extend_from_slice(&SERVER_MAC);
    frame.extend_from_slice(&protocol.to_be_bytes());
    frame
}

fn read_u16(bytes: &[u8], index: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        bytes.get(index..index + 2)?.try_into().ok()?,
    ))
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 * (crc & 1));
        }
    }
    !crc
}
