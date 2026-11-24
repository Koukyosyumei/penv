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
