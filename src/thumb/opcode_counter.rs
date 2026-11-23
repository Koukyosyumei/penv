#[derive(Default)]
pub struct OpcodeCounters {
    pub enabled: bool,
    counters: Vec<Counter>,
}

struct Counter {
    name: String,
    mask: u32,
    value: u32,
    count: u64,
}

impl OpcodeCounters {
    pub fn install(&mut self, name: &str, pattern: &str) -> Result<(), String> {
        if pattern.len() != 32 {
            return Err("Thumb opcode pattern must contain 32 bits".into());
        }
        if self.counters.iter().any(|counter| counter.name == name) {
            return Err(format!("duplicate opcode counter {name:?}"));
        }
        let (mut mask, mut value) = (0, 0);
        for bit in pattern.bytes() {
            mask <<= 1;
            value <<= 1;
            match bit {
                b'0' => mask |= 1,
                b'1' => {
                    mask |= 1;
                    value |= 1;
                }
                b'x' => {}
                _ => return Err("expected 0, 1 or x in opcode pattern".into()),
            }
        }
        self.counters.push(Counter {
            name: name.into(),
            mask,
            value,
            count: 0,
        });
        Ok(())
    }

    pub fn record(&mut self, opcode: u32) {
        for counter in &mut self.counters {
            if opcode & counter.mask == counter.value {
                counter.count = counter.count.wrapping_add(1);
            }
        }
    }

    pub fn get(&self, name: &str) -> Result<u64, String> {
        self.counters
            .iter()
            .find(|counter| counter.name == name)
            .map(|counter| counter.count)
            .ok_or_else(|| format!("unknown opcode counter {name:?}"))
    }
}

#[cfg(test)]
mod tests {
    use crate::machine::{Machine, Stop};

    fn check(opcodes: [u16; 2], patterns: &[(&str, &str, u64)], end: u32) {
        let mut machine = Machine::new(0x1000).unwrap();
        machine.bus.map_ram(0, 0x1000).unwrap();
        machine.bus.map_nvic(0xe0, None, true).unwrap();
        machine.bus.map_systick().unwrap();
        machine.primary.configure_cortex_m(Some(0), true);
        machine.primary.pc = 0;
        let mut output = std::io::sink();
        for (index, opcode) in opcodes.into_iter().enumerate() {
            machine
                .bus
                .write(index as u32 * 2, 2, u32::from(opcode), &mut output)
                .unwrap();
        }
        for &(name, pattern, _) in patterns {
            machine.bus.thumb_opcodes.install(name, pattern).unwrap();
        }
        machine.bus.thumb_opcodes.enabled = true;
        assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.primary.pc, end);
        for &(name, _, count) in patterns {
            assert_eq!(machine.bus.thumb_opcodes.get(name).unwrap(), count);
        }
    }

    #[test]
    fn upstream_counts_32_bit_thumb_opcode() {
        check(
            [0xf241, 0x2034],
            &[
                ("movw", "11110010010000010010000000110100", 1),
                ("unexpected", "11110010010000010010000000110101", 0),
            ],
            4,
        );
    }

    #[test]
    fn upstream_counts_16_bit_thumb_opcode() {
        check(
            [0xbf00, 0xbf00],
            &[("nop", "00000000000000001011111100000000", 1)],
            2,
        );
    }
}
