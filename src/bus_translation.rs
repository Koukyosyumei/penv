use crate::{
    access_translation::{Endian, NativeWrite, read_using, write_using},
    dma_engine::Width,
};

use std::sync::Arc;

#[derive(Clone, Default)]
pub struct PeripheralHandle(Arc<()>);

impl PartialEq for PeripheralHandle {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for PeripheralHandle {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistrationError {
    NotRegistered,
    AlreadyGrouped,
    GroupNotFound,
}

pub trait NativePeripheral {
    fn read(&mut self, region: Option<&str>, offset: u64) -> u64;
    fn write(&mut self, region: Option<&str>, offset: u64, value: u64);
}

pub struct Port {
    pub base: u32,
    pub length: u32,
    pub region: Option<String>,
}

pub(crate) struct Registration {
    pub handle: PeripheralHandle,
    pub ports: Vec<Port>,
    width: Width,
    peripheral: Box<dyn NativePeripheral>,
}

impl Registration {
    pub fn new(
        handle: PeripheralHandle,
        ports: Vec<Port>,
        width: Width,
        peripheral: Box<dyn NativePeripheral>,
    ) -> Self {
        Self {
            handle,
            ports,
            width,
            peripheral,
        }
    }

    pub fn read(&mut self, address: u32, requested: Width, enabled: bool) -> Option<u64> {
        let (port, offset) = find(&self.ports, address, requested)?;
        if requested != self.width && !enabled {
            return Some(0);
        }
        Some(read_using(
            offset,
            requested,
            self.width,
            Endian::Little,
            |offset| self.peripheral.read(port.region.as_deref(), offset),
        ))
    }

    pub fn write(
        &mut self,
        address: u32,
        requested: Width,
        value: u64,
        enabled: bool,
    ) -> Option<()> {
        let (port, offset) = find(&self.ports, address, requested)?;
        if requested == self.width || enabled {
            let mut device = Region {
                peripheral: self.peripheral.as_mut(),
                region: port.region.as_deref(),
            };
            write_using(
                offset,
                value,
                requested,
                self.width,
                Endian::Little,
                &mut device,
            );
        }
        Some(())
    }
}

fn find(ports: &[Port], address: u32, requested: Width) -> Option<(&Port, u64)> {
    ports.iter().find_map(|port| {
        let offset = u64::from(address.checked_sub(port.base)?);
        (offset + requested as u64 <= u64::from(port.length)).then_some((port, offset))
    })
}

struct Region<'a> {
    peripheral: &'a mut dyn NativePeripheral,
    region: Option<&'a str>,
}

impl NativeWrite for Region<'_> {
    fn read_for_write(&mut self, offset: u64) -> u64 {
        self.peripheral.read(self.region, offset)
    }

    fn write_native(&mut self, offset: u64, value: u64) {
        self.peripheral.write(self.region, offset, value);
    }
}

pub(crate) fn width(bytes: usize) -> Option<Width> {
    match bytes {
        1 => Some(Width::Byte),
        2 => Some(Width::Word),
        4 => Some(Width::DoubleWord),
        8 => Some(Width::QuadWord),
        _ => None,
    }
}
