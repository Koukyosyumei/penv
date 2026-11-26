#[derive(Default)]
pub struct OpcodeCounters {
    pub enabled: bool,
    counters: Vec<Counter>,
}

struct Counter {
    name: String,
    mask: u64,
    value: u64,
    count: u64,
}

impl OpcodeCounters {
    pub fn install(&mut self, name: &str, pattern: &str) -> Result<(), String> {
        if !matches!(pattern.len(), 16 | 32) {
            return Err("opcode pattern must contain 16 or 32 bits".into());
        }
        let (mask, value) = parse_pattern(pattern)?;
        self.install_mask(name, mask, value)
    }

    pub(crate) fn install_mask(&mut self, name: &str, mask: u64, value: u64) -> Result<(), String> {
        if self.counters.iter().any(|counter| counter.name == name) {
            return Err(format!("duplicate opcode counter {name:?}"));
        }
        self.counters.push(Counter {
            name: name.into(),
            mask,
            value,
            count: 0,
        });
        Ok(())
    }

    pub fn record(&mut self, opcode: impl Into<u64>) {
        let opcode = opcode.into();
        for counter in &mut self.counters {
            if opcode & counter.mask == counter.value {
                counter.count = counter.count.wrapping_add(1);
            }
        }
    }

    pub(crate) fn entries(&self) -> impl Iterator<Item = (&str, u64)> {
        self.counters
            .iter()
            .map(|counter| (counter.name.as_str(), counter.count))
    }

    pub fn get(&self, name: &str) -> Result<u64, String> {
        self.counters
            .iter()
            .find(|counter| counter.name == name)
            .map(|counter| counter.count)
            .ok_or_else(|| format!("unknown opcode counter {name:?}"))
    }
}

pub(crate) fn parse_pattern(pattern: &str) -> Result<(u64, u64), String> {
    if !matches!(pattern.len(), 16 | 32 | 64) {
        return Err("opcode pattern must contain 16, 32 or 64 bits".into());
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
            b'x' | b'X' | b'_' | b'?' => {}
            _ => return Err("invalid opcode pattern character".into()),
        }
    }
    Ok((mask, value))
}

#[cfg(test)]
mod tests {
    use super::HookPhase;
    use crate::machine::{Machine, Stop};

    fn machine_with_opcodes(opcodes: &[u16]) -> Machine {
        let mut machine = Machine::new(0x1000).unwrap();
        machine.bus.map_ram(0, 0x1000).unwrap();
        machine.bus.map_nvic(0xe0, None).unwrap();
        machine.bus.map_systick().unwrap();
        machine.primary.configure_cortex_m(Some(0), true);
        machine.primary.pc = 0;
        let mut output = std::io::sink();
        for (index, &opcode) in opcodes.iter().enumerate() {
            machine
                .bus
                .write(index as u32 * 2, 2, u32::from(opcode), &mut output)
                .unwrap();
        }
        machine
    }

    fn check(opcodes: [u16; 2], patterns: &[(&str, &str, u64)], end: u32) {
        let mut machine = machine_with_opcodes(&opcodes);
        let mut output = std::io::sink();
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
    fn check_hooks(words: &[u16], opcode: u32, end: u32) {
        let mut machine = machine_with_opcodes(words);
        for (phase, value, label) in [
            (HookPhase::Pre, opcode, "pre"),
            (HookPhase::Post, opcode, "post"),
            (HookPhase::Post, opcode + (1 << 16), "unexpected"),
        ] {
            machine
                .bus
                .thumb_opcode_hooks
                .install(phase, u32::MAX, value, label.into());
        }
        let mut logs = Vec::new();
        assert_eq!(machine.run(1, &mut logs).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.primary.pc, end);
        assert_eq!(
            String::from_utf8(logs).unwrap(),
            format!(
                "pre: PC 0x00000000 op {opcode:#010x}\npost: PC 0x00000000 op {opcode:#010x}\n"
            )
        );
    }

    #[test]
    fn upstream_hooks_receive_32_bit_thumb_opcode() {
        check_hooks(&[0xf241, 0x2034], 0xf2412034, 4);
    }

    #[test]
    fn upstream_hooks_receive_16_bit_thumb_opcode() {
        check_hooks(&[0xbf00], 0xbf00, 2);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HookPhase {
    Pre,
    Post,
}

#[derive(Default)]
pub struct OpcodeLogHooks {
    hooks: Vec<(HookPhase, u32, u32, String)>,
}

impl OpcodeLogHooks {
    pub fn install(&mut self, phase: HookPhase, mask: u32, pattern: u32, label: String) {
        self.hooks.push((phase, mask, pattern, label));
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    pub fn emit(
        &self,
        phase: HookPhase,
        pc: u32,
        opcode: u32,
        output: &mut impl std::io::Write,
    ) -> Result<(), String> {
        for (when, mask, pattern, label) in &self.hooks {
            if *when == phase && opcode & mask == *pattern {
                writeln!(output, "{label}: PC {pc:#010x} op {opcode:#010x}")
                    .map_err(|error| format!("opcode hook output: {error}"))?;
            }
        }
        Ok(())
    }
}
