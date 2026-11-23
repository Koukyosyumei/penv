pub const MMISC_CTL: u32 = 0x7d0;

#[derive(Clone)]
pub struct AndesCsrs {
    misc_control: u32,
    stack_control: u32,
    stack_bound: u32,
    stack_base: u32,
    stack_warning_logged: bool,
}

impl Default for AndesCsrs {
    fn default() -> Self {
        Self {
            misc_control: 0,
            stack_control: 0,
            stack_bound: u32::MAX,
            stack_base: u32::MAX,
            stack_warning_logged: false,
        }
    }
}

impl AndesCsrs {
    pub fn unaligned_accesses(&self) -> bool {
        self.misc_control & (1 << 6) != 0
    }

    pub fn read(&self, address: u32) -> Option<u32> {
        Some(match address {
            MMISC_CTL => self.misc_control,
            0x7c6 => self.stack_control,
            0x7c7 => self.stack_bound,
            0x7c8 => self.stack_base,
            0x7ca => u32::MAX,
            0xfc2 => (1 << 16) | (1 << 5), // CCTL and HSP.
            0x7c4 | 0xfc0 | 0xfc1 | 0xfc3 | 0xfc7..=0xfcb | 0xfcf => 0,
            _ => {
                let name = stub_name(address)?;
                eprintln!(
                    "Tried to read from an unimplemented {name} CSR (0x{address:X}), returning 0x0"
                );
                0
            }
        })
    }

    pub fn write(&mut self, address: u32, value: u32) -> bool {
        match address {
            MMISC_CTL => self.misc_control = value,
            0x7c6 => {
                self.stack_control = value & 0x3f;
                if self.stack_control != 0 && !self.stack_warning_logged {
                    eprintln!("StackSafe is a stub, stack protection is not implemented");
                    self.stack_warning_logged = true;
                }
            }
            0x7c7 => self.stack_bound = value,
            0x7c8 => self.stack_base = value,
            0x7c4 => eprintln!(
                "Writing to the Machine Extended Status CSR (0x7c4) is currently not supported"
            ),
            0x7ca => eprintln!(
                "Writing to the Machine Cache Control CSR (0x7ca) is currently not supported"
            ),
            0xfc0 | 0xfc1 | 0xfc2 | 0xfc3 | 0xfc7..=0xfcb | 0xfcf => {
                eprintln!(
                    "Writing to the Machine Custom read-only CSR (0x{address:x}) is not supported"
                );
            }
            _ => {
                let Some(name) = stub_name(address) else {
                    return false;
                };
                eprintln!(
                    "Tried to write 0x{value:X} to an unimplemented {name} CSR (0x{address:X}), write ignored"
                );
            }
        }
        true
    }
}

fn stub_name(address: u32) -> Option<String> {
    Some(match address {
        0x7cb => "MCCTLBEGINADDR".into(),
        0x7cc => "MCCTLCOMMAND".into(),
        0x7cd => "MCCTLDATA".into(),
        0x800 => "UITB".into(),
        0x801 => "UCODE".into(),
        0x80b => "UCCTLBEGINADDR".into(),
        0x80c => "UCCTLCOMMAND".into(),
        0xbc0..=0xbc3 => format!("PMACFG{}", address - 0xbc0),
        0xbd0..=0xbdf => format!("PMAADDR{}", address - 0xbd0),
        _ => return None,
    })
}
