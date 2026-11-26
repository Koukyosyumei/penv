use std::io::{self, Read, Write};

use super::{Cpu, Sleep};
use crate::snapshot::{invalid, read_bool, read_words, write_words};

impl Cpu {
    pub(crate) fn save_snapshot(&self, output: &mut impl Write) -> io::Result<()> {
        let sleep = match self.sleep {
            Sleep::Awake => 0,
            Sleep::Interrupt => 1,
            Sleep::Event => 2,
        };
        write_words(output, &[u64::from(self.affinity), sleep])?;
        let control = [
            self.cpsr, self.sctlr, self.vbar, self.hsctlr, self.hactlr, self.hvbar,
            self.hcr, self.cpacr, self.fpexc, self.elr_hyp, self.debug_status,
        ];
        write_words(output, &control.map(u64::from))?;
        write_words(output, &[self.hdcr, self.hcptr, self.hstr, self.actlr].map(u64::from))?;
        for bank in self.banks {
            write_words(output, &[bank.sp, bank.lr, bank.spsr].map(u64::from))?;
        }
        write_words(output, &self.shared_high.map(u64::from))?;
        write_words(output, &self.fiq_high.map(u64::from))?;
        write_words(output, &self.vfp_high_words.map(u64::from))?;
        write_words(output, &self.thread_ids.map(u64::from))?;
        write_words(output, &self.interrupt_inputs.map(u64::from))?;
        write_words(output, &self.fault_status.map(u64::from))?;
        write_words(output, &self.fault_address.map(u64::from))?;
        write_words(output, &[u64::from(self.hpfar)])?;
        self.thumb.save_snapshot(output)?;
        if let Some(pmsa) = &self.pmsa {
            for bank in pmsa {
                bank.save_snapshot(output)?;
            }
        }
        if let Some(timer) = &self.timer {
            write_words(output, &[u64::from(timer.counter_frequency_register), timer.virtual_offset])?;
            write_words(output, &[u64::from(timer.hyp_counter_control)])?;
            for channel in timer.channels {
                write_words(output, &[channel.compare, u64::from(channel.control)])?;
            }
        }
        Ok(())
    }

    pub(crate) fn load_snapshot(&mut self, input: &mut impl Read, version: u8) -> io::Result<()> {
        let [affinity, sleep] = read_words(input)?;
        if affinity != u64::from(self.affinity) {
            return Err(invalid("snapshot AArch32 CPU affinity differs from the platform"));
        }
        self.sleep = match sleep {
            0 => Sleep::Awake,
            1 => Sleep::Interrupt,
            2 => Sleep::Event,
            _ => return Err(invalid("invalid AArch32 sleep state")),
        };
        [self.cpsr, self.sctlr, self.vbar, self.hsctlr, self.hactlr, self.hvbar,
            self.hcr, self.cpacr, self.fpexc, self.elr_hyp, self.debug_status] = read_u32s(input)?;
        if version >= 4 {
            [self.hdcr, self.hcptr, self.hstr] = read_u32s(input)?;
        }
        if version >= 9 {
            [self.actlr] = read_u32s(input)?;
        }
        if Self::bank_for(self.cpsr).is_none() {
            return Err(invalid("invalid snapshot AArch32 CPU mode"));
        }
        for bank in &mut self.banks {
            [bank.sp, bank.lr, bank.spsr] = read_u32s(input)?;
        }
        self.shared_high = read_u32s(input)?;
        self.fiq_high = read_u32s(input)?;
        self.vfp_high_words = read_u32s(input)?;
        if version >= 3 {
            self.thread_ids = read_u32s(input)?;
        } else {
            let [first, second, third] = read_u32s(input)?;
            self.thread_ids = [first, second, third, 0];
        }
        for (flag, value) in self.interrupt_inputs.iter_mut().zip(read_words::<2>(input)?) {
            *flag = read_bool(value)?;
        }
        self.fault_status = read_u32s(input)?;
        self.fault_address = read_u32s(input)?;
        if version >= 5 {
            [self.hpfar] = read_u32s(input)?;
        }
        self.thumb.load_snapshot(input)?;
        if let Some(pmsa) = &mut self.pmsa {
            for bank in pmsa {
                bank.load_snapshot(input)?;
            }
        }
        if let Some(timer) = &mut self.timer {
            let [frequency, offset] = read_words(input)?;
            timer.counter_frequency_register = u32::try_from(frequency).map_err(io::Error::other)?;
            timer.virtual_offset = offset;
            if version >= 8 {
                [timer.hyp_counter_control] = read_u32s(input)?;
            }
            for channel in &mut timer.channels {
                let [compare, control] = read_words(input)?;
                if control > 3 {
                    return Err(invalid("invalid snapshot AArch32 timer control"));
                }
                channel.compare = compare;
                channel.control = control as u32;
            }
        }
        Ok(())
    }
}

fn read_u32s<const N: usize>(input: &mut impl Read) -> io::Result<[u32; N]> {
    let mut words = [0; N];
    for (word, value) in words.iter_mut().zip(read_words::<N>(input)?) {
        *word = u32::try_from(value).map_err(io::Error::other)?;
    }
    Ok(words)
}
