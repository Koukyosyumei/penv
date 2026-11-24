use super::Bus;
use crate::bus_translation::{PeripheralHandle, RegistrationError};

pub struct NullRegistrationPoint {
    _private: (),
}

pub static NULL_REGISTRATION_POINT: NullRegistrationPoint = NullRegistrationPoint { _private: () };

pub struct RegisteredChild<'a> {
    pub peripheral: &'a PeripheralHandle,
    pub registration_point: &'static NullRegistrationPoint,
}

impl Bus {
    pub fn is_peripheral_registered(&self, peripheral: &PeripheralHandle) -> bool {
        self.is_translated_peripheral_registered(peripheral)
            || self
                .translated_children
                .iter()
                .any(|(_, child)| child == peripheral)
    }

    pub fn register_null_child(
        &mut self,
        owner: &PeripheralHandle,
        child: &PeripheralHandle,
        _point: &'static NullRegistrationPoint,
    ) -> Result<(), RegistrationError> {
        if !self.is_translated_peripheral_registered(owner) {
            return Err(RegistrationError::NotRegistered);
        }
        if self.null_child(owner).is_some() {
            return Err(RegistrationError::ContainerOccupied);
        }
        if self.is_peripheral_registered(child) {
            return Err(RegistrationError::AlreadyRegistered);
        }
        self.translated_children
            .push((owner.clone(), child.clone()));
        Ok(())
    }

    pub fn unregister_null_child(
        &mut self,
        owner: &PeripheralHandle,
        child: &PeripheralHandle,
    ) -> Result<(), RegistrationError> {
        let index = self
            .translated_children
            .iter()
            .position(|(parent, entry)| parent == owner && entry == child)
            .ok_or(RegistrationError::NotRegistered)?;
        self.translated_children.remove(index);
        Ok(())
    }

    pub fn null_registration_points(
        &self,
        owner: &PeripheralHandle,
        _child: &PeripheralHandle,
    ) -> impl Iterator<Item = &'static NullRegistrationPoint> {
        // The pinned container returns its slot whenever occupied.
        self.null_child(owner)
            .map(|entry| entry.registration_point)
            .into_iter()
    }

    pub fn null_child(&self, owner: &PeripheralHandle) -> Option<RegisteredChild<'_>> {
        self.translated_children.iter().find_map(|(parent, child)| {
            (parent == owner).then_some(RegisteredChild {
                peripheral: child,
                registration_point: &NULL_REGISTRATION_POINT,
            })
        })
    }

    pub(super) fn remove_translated_children(&mut self, owner: &PeripheralHandle) {
        self.translated_children
            .retain(|(parent, _)| parent != owner);
    }
}
