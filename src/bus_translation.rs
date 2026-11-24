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
    ContainerOccupied,
    AlreadyRegistered,
    MissingCpuRegistrationPoint,
    CpuSlotOccupied,
}

pub trait NativePeripheral {
    fn read(&mut self, region: Option<&str>, offset: u64) -> u64;
    fn write(&mut self, region: Option<&str>, offset: u64, value: u64);
    fn start(&mut self) {}
    fn pause(&mut self) {}
    fn resume(&mut self) {}

    fn read_width(&mut self, region: Option<&str>, offset: u64, _width: Width) -> u64 {
        self.read(region, offset)
    }

    fn write_width(&mut self, region: Option<&str>, offset: u64, _width: Width, value: u64) {
        self.write(region, offset, value);
    }
}

pub struct Port {
    pub base: u32,
    pub length: u32,
    pub region: Option<String>,
}

pub(crate) struct Registration {
    pub handle: PeripheralHandle,
    pub ports: Vec<Port>,
    widths: Vec<Vec<Width>>,
    peripheral: Box<dyn NativePeripheral>,
}

impl Registration {
    pub fn new(
        handle: PeripheralHandle,
        ports: Vec<(Port, Vec<Width>)>,
        peripheral: Box<dyn NativePeripheral>,
    ) -> Self {
        let (ports, widths) = ports.into_iter().unzip();
        Self {
            handle,
            ports,
            widths,
            peripheral,
        }
    }

    pub fn start(&mut self) {
        self.peripheral.start();
    }

    pub fn pause(&mut self) {
        self.peripheral.pause();
    }

    pub fn resume(&mut self) {
        self.peripheral.resume();
    }

    pub fn remove_port(&mut self, address: u32) {
        if let Some(index) = self.ports.iter().position(|port| port.base == address) {
            self.ports.remove(index);
            self.widths.remove(index);
        }
    }

    fn native_width(&self, port: usize, requested: Width, enabled: bool) -> Option<Width> {
        let widths = &self.widths[port];
        if widths.contains(&requested) {
            Some(requested)
        } else {
            enabled.then_some(widths[0])
        }
    }

    pub fn read(&mut self, address: u32, requested: Width, enabled: bool) -> Option<u64> {
        let (index, offset) = find(&self.ports, address, requested)?;
        let Some(native) = self.native_width(index, requested, enabled) else {
            return Some(0);
        };
        let region = self.ports[index].region.as_deref();
        Some(read_using(
            offset,
            requested,
            native,
            Endian::Little,
            |offset| self.peripheral.read_width(region, offset, native),
        ))
    }

    pub fn write(
        &mut self,
        address: u32,
        requested: Width,
        value: u64,
        enabled: bool,
    ) -> Option<()> {
        let (index, offset) = find(&self.ports, address, requested)?;
        if let Some(native) = self.native_width(index, requested, enabled) {
            let mut device = Region {
                peripheral: self.peripheral.as_mut(),
                region: self.ports[index].region.as_deref(),
                width: native,
            };
            write_using(
                offset,
                value,
                requested,
                native,
                Endian::Little,
                &mut device,
            );
        }
        Some(())
    }
}

fn find(ports: &[Port], address: u32, requested: Width) -> Option<(usize, u64)> {
    ports.iter().enumerate().find_map(|(index, port)| {
        let offset = u64::from(address.checked_sub(port.base)?);
        (offset + requested as u64 <= u64::from(port.length)).then_some((index, offset))
    })
}

struct Region<'a> {
    peripheral: &'a mut dyn NativePeripheral,
    region: Option<&'a str>,
    width: Width,
}

impl NativeWrite for Region<'_> {
    fn read_for_write(&mut self, offset: u64) -> u64 {
        self.peripheral.read_width(self.region, offset, self.width)
    }

    fn write_native(&mut self, offset: u64, value: u64) {
        self.peripheral
            .write_width(self.region, offset, self.width, value);
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
