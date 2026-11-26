use super::Bus;
use crate::bus_translation::{PeripheralHandle, RegistrationError};

#[derive(Clone, Copy, Default)]
pub struct CpuRegistrationPoint {
    pub slot: Option<usize>,
}

pub(crate) struct RegisteredCpu {
    pub handle: PeripheralHandle,
    pub architecture: String,
    pub slot: usize,
    pub local_name: Option<String>,
}

impl Bus {
    /// Register the bus identity of a non-clustered CPU at an explicit point.
    pub fn register_cpu_identity(
        &mut self,
        handle: &PeripheralHandle,
        architecture: &str,
        point: Option<CpuRegistrationPoint>,
    ) -> Result<usize, RegistrationError> {
        let point = point.ok_or(RegistrationError::MissingCpuRegistrationPoint)?;
        if self.is_peripheral_registered(handle) {
            return Err(RegistrationError::AlreadyRegistered);
        }

        let occupied = |slot| {
            self.registered_cpu_identities
                .iter()
                .any(|cpu| cpu.slot == slot)
        };
        let slot = match point.slot {
            Some(slot) if !occupied(slot) => slot,
            Some(_) => return Err(RegistrationError::CpuSlotOccupied),
            None => (0..).find(|&slot| !occupied(slot)).unwrap(),
        };
        self.registered_cpu_identities.push(RegisteredCpu {
            handle: handle.clone(),
            architecture: architecture.into(),
            slot,
            local_name: None,
        });
        Ok(slot)
    }

    pub fn registered_cpu_architecture(&self, slot: usize) -> Option<&str> {
        self.registered_cpu_identities
            .iter()
            .find(|cpu| cpu.slot == slot)
            .map(|cpu| cpu.architecture.as_str())
    }
}

impl Bus {
    pub(crate) fn cpu_registrations(&self) -> &[RegisteredCpu] {
        &self.registered_cpu_identities
    }

    pub(crate) fn set_cpu_local_name(
        &mut self,
        handle: &PeripheralHandle,
        name: &str,
    ) -> Result<(), RegistrationError> {
        let cpu = self
            .registered_cpu_identities
            .iter_mut()
            .find(|cpu| &cpu.handle == handle)
            .ok_or(RegistrationError::NotRegistered)?;
        cpu.local_name = Some(name.to_owned());
        Ok(())
    }

    /// Save CPU bus identities and machine-local names. The caller supplies the
    /// device-specific payload, independently of the bus registration metadata.
    pub(crate) fn save_cpu_registrations(
        &self,
        output: impl std::io::Write,
        mut save_cpu: impl FnMut(&PeripheralHandle) -> std::io::Result<Option<String>>,
    ) -> std::io::Result<()> {
        let mut writer = crate::migrant::primitive::Writer::new(output, false);
        writer.unsigned(self.registered_cpu_identities.len() as u64)?;
        for cpu in &self.registered_cpu_identities {
            writer.unsigned(cpu.slot as u64)?;
            writer.string(&cpu.architecture)?;
            let payload = save_cpu(&cpu.handle)?;
            for field in [cpu.local_name.as_deref(), payload.as_deref()] {
                writer.bytes(&[u8::from(field.is_some())])?;
                if let Some(value) = field {
                    writer.string(value)?;
                }
            }
        }
        writer.finish()
    }

    /// Restore the registrations onto a fresh bus, instantiating each CPU with a
    /// new handle and returning the saved device-specific payload to the caller.
    pub(crate) fn restore_cpu_registrations(
        &mut self,
        input: impl std::io::Read,
    ) -> std::io::Result<Vec<(PeripheralHandle, Option<String>)>> {
        use std::io::{Error, ErrorKind};
        if !self.registered_cpu_identities.is_empty() {
            return Err(Error::new(
                ErrorKind::AlreadyExists,
                "CPU registrations already exist",
            ));
        }
        let mut reader = crate::migrant::primitive::Reader::new(input, false);
        let count = reader.unsigned()?;
        let mut restored = Vec::new();
        let mut bus = Bus::default();
        for _ in 0..count {
            let slot = usize::try_from(reader.unsigned()?)
                .map_err(|_| Error::new(ErrorKind::InvalidData, "invalid CPU slot"))?;
            let architecture = reader.string()?;
            let mut read_optional = || match reader.byte()? {
                0 => Ok(None),
                1 => reader.string().map(Some),
                _ => Err(Error::new(ErrorKind::InvalidData, "invalid CPU field flag")),
            };
            let name = read_optional()?;
            let payload = read_optional()?;
            let handle = PeripheralHandle::default();
            bus.register_cpu_identity(
                &handle,
                &architecture,
                Some(CpuRegistrationPoint { slot: Some(slot) }),
            )
            .map_err(|_| Error::new(ErrorKind::InvalidData, "duplicate CPU registration"))?;
            if let Some(name) = name {
                bus.set_cpu_local_name(&handle, &name).unwrap();
            }
            restored.push((handle, payload));
        }
        reader.finish()?;
        self.registered_cpu_identities = bus.registered_cpu_identities;
        Ok(restored)
    }
}
