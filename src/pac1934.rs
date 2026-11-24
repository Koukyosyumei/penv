use crate::efr32_i2c::Slave;

/// Identification registers of the PAC1934 power monitor.
#[derive(Default)]
pub struct Pac1934 {
    register: Option<u8>,
}

impl Slave for Pac1934 {
    fn write(&mut self, bytes: &[u8]) {
        if let Some(&register) = bytes.first() {
            self.register = Some(register);
        }
    }

    fn read(&mut self, _: usize) -> Vec<u8> {
        let Some(register) = self.register.take() else {
            return vec![0];
        };
        let value = match register {
            0xfd => 0x5b,
            0xfe => 0x5d,
            0xff => 0x03,
            _ => 0,
        };
        u16::to_le_bytes(value).to_vec()
    }
}
