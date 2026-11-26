use super::{
    Entry, RiscvConfig, keys_in, number, split_rv32_init, unsupported_rv32_isa, uses_vex_irq,
};
use crate::{
    bitmanip::BitManip,
    csr::{DEFAULT_MTVEC, InterruptMode},
};

pub(crate) fn parse_rv32_cpu<'a>(
    entry: &'a Entry,
    timers: [Option<&Entry>; 3],
    clic: Option<&Entry>,
    egis_syscon: Option<&Entry>,
) -> Result<RiscvConfig<'a>, String> {
    let [clint, litex_cpu_timer, riscv_machine_timer] = timers;
    let ibex = entry.model == "CPU.IbexRiscV32";
    let mut isa = match entry.model.as_str() {
        "CPU.IbexRiscV32" => Some((true, false, true, false)),
        "CPU.Minerva" => Some((false, false, false, false)),
        "CPU.VexRiscv" => Some((true, false, false, false)),
        _ => None,
    };
    let mut compressed_float = (true, true);
    let mut floating = (false, false, false, false, false);
    let mut zfa = false;
    let mut zfa_fli = false;
    let mut zfa_min_max = false;
    let mut zfa_quiet_compare = false;
    let mut zfa_round = false;
    let mut zicond = false;
    let mut zacas = false;
    let mut zmmul = false;
    let mut zabha = false;
    let mut zaamo = false;
    let mut zalrsc = false;
    let mut smepmp = false;
    let mut sscofpmf = false;
    let mut bitmanip = BitManip::default();
    let mut id = 0;
    let mut privilege_levels = (
        entry.model != "CPU.Minerva",
        !matches!(entry.model.as_str(), "CPU.Minerva" | "CPU.IbexRiscV32"),
    );
    let mut pmp = if matches!(entry.model.as_str(), "CPU.RiscV32" | "CPU.IbexRiscV32") {
        16
    } else {
        0
    };
    let (properties, _) = split_rv32_init(entry);
    let cpu_properties = properties
        .iter()
        .position(|(_, line)| line == "reset:")
        .map_or(properties, |index| &properties[..index]);
    if cpu_properties.len() != properties.len() {
        let reset = egis_syscon.ok_or_else(|| entry.error("unsupported CPU reset hook"))?;
        let expected = format!("PC `{} ResetVector`", reset.name);
        if !matches!(&properties[cpu_properties.len() + 1..], [(_, hook)] if hook == &expected) {
            return Err(entry.error("unsupported CPU reset hook"));
        }
    }
    for (key, value) in keys_in(
        entry,
        cpu_properties,
        &[
            "cpuType",
            "hartId",
            "timeProvider",
            "privilegedArchitecture",
            "privilegeLevels",
            "pmpEntryCount",
            "builtInIrqController",
            "allowUnalignedAccesses",
            "CyclesPerInstruction",
            "vectorBootstrap",
            "zfaFli",
            "zfaMinMax",
            "zfaQuietCompare",
            "zfaRound",
            "interruptMode",
        ],
    )? {
        match (key, value) {
            ("cpuType", value) => {
                let architecture = value.trim_matches('"').to_ascii_lowercase();
                let mut base = architecture.as_str();
                zfa = false;
                zicond = false;
                zacas = false;
                zmmul = false;
                zabha = false;
                zaamo = false;
                zalrsc = false;
                smepmp = false;
                sscofpmf = false;
                let mut zca = false;
                let mut zcf = false;
                let mut zcd = false;
                let mut zfh = false;
                let mut zfhmin = false;
                let mut zfbfmin = false;
                bitmanip = BitManip::default();
                while let Some((prefix, extension)) = base.rsplit_once('_') {
                    match extension {
                        "zicsr" | "zifencei" if !ibex => {}
                        "zihintpause" => {}
                        "zicond" if !zicond => zicond = true,
                        "zmmul" if !zmmul => zmmul = true,
                        "zaamo" if !zaamo => zaamo = true,
                        "zalrsc" if !zalrsc => zalrsc = true,
                        "zabha" if !zabha => zabha = true,
                        "zacas" if !zacas => zacas = true,
                        "sscofpmf" if !sscofpmf => sscofpmf = true,
                        "smepmp" if !smepmp => smepmp = true,
                        "zfbfmin" if !zfbfmin => zfbfmin = true,
                        "zfa" if !zfa => zfa = true,
                        "zfhmin" if !zfhmin => zfhmin = true,
                        "zfh" if !zfh => zfh = true,
                        "zba" if !bitmanip.zba => bitmanip.zba = true,
                        "zbb" if !bitmanip.zbb => bitmanip.zbb = true,
                        "zksed" if !bitmanip.zksed => bitmanip.zksed = true,
                        "zksh" if !bitmanip.zksh => bitmanip.zksh = true,
                        "zkne" if !bitmanip.zkne => bitmanip.zkne = true,
                        "zknd" if !bitmanip.zknd => bitmanip.zknd = true,
                        "zknh" if !bitmanip.zknh => bitmanip.zknh = true,
                        "zbkc" if !bitmanip.zbkc => bitmanip.zbkc = true,
                        "zbc" if !bitmanip.zbc => bitmanip.zbc = true,
                        "xandesvsintload" if !bitmanip.xandesvsintload => {
                            bitmanip.xandesvsintload = true
                        }
                        "xandesvbfhcvt" if !bitmanip.xandesvbfhcvt => bitmanip.xandesvbfhcvt = true,
                        "xandesvqmac" if !bitmanip.xandesvqmac => bitmanip.xandesvqmac = true,
                        "xandesvdot" if !bitmanip.xandesvdot => bitmanip.xandesvdot = true,
                        "xandesvpackfph" if !bitmanip.xandesvpackfph => {
                            bitmanip.xandesvpackfph = true
                        }
                        "xandesvsinth" if !bitmanip.xandesvsinth => bitmanip.xandesvsinth = true,
                        "v" if !bitmanip.vector => bitmanip.vector = true,
                        "xandesbfhcvt" if !bitmanip.xandesbfhcvt => bitmanip.xandesbfhcvt = true,
                        "xandes" | "xandesperf" if !bitmanip.xandesperf => {
                            bitmanip.xandesperf = true
                        }
                        "zca" if !zca => zca = true,
                        "zcf" if !zcf => zcf = true,
                        "zcd" if !zcd => zcd = true,
                        "zcb" if !bitmanip.zcb => bitmanip.zcb = true,
                        "zbkx" if !bitmanip.zbkx => bitmanip.zbkx = true,
                        "zbkb" if !bitmanip.zbkb => bitmanip.zbkb = true,
                        "zbs" if !bitmanip.zbs => bitmanip.zbs = true,
                        _ => break,
                    }
                    base = prefix;
                }
                let (letters, suffix) = base.split_at(base.find('z').unwrap_or(base.len()));
                let letters = letters.strip_prefix("rv32").unwrap_or("");
                let letters = letters.replace('_', "");
                bitmanip.vector |= letters.contains('v');
                let general = letters.contains('g');
                let embedded = letters.contains('e');
                let single = general || letters.contains('f') || bitmanip.xandesbfhcvt;
                let double = general || letters.contains('d');
                // Renode accepts separated C even after a named extension.
                let compressed = letters.contains('c') || suffix.split('_').any(|part| part == "c");
                let valid_suffix = suffix
                    .split('_')
                    .all(|part| matches!(part, "" | "c" | "zicsr" | "zifencei" | "zihintpause"));
                let ibex_legacy = ibex && base == "rv32imcu_zicsr_zifencei";
                if !ibex_legacy
                    && (!(general || embedded || letters.contains('i'))
                        || !letters.chars().all(|letter| "iemafdcgbv".contains(letter))
                        || (embedded && !letters.chars().all(|letter| "emac".contains(letter)))
                        || !valid_suffix
                        || (double && !single))
                {
                    return Err(entry.error(&unsupported_rv32_isa(value, &architecture)));
                }
                let legacy_compressed = compressed || bitmanip.zcb;
                if zcf && (!(legacy_compressed || zca) || !single) {
                    return Err(entry.error("Zcf requires RV32 Zca and F"));
                }
                if zcd && (!(legacy_compressed || zca) || !double) {
                    return Err(entry.error("Zcd requires RV32 Zca and D"));
                }
                compressed_float = (legacy_compressed || zcf, legacy_compressed || zcd);
                floating = (single, double, zfh, zfbfmin, zfhmin);
                if letters.contains('b') {
                    // Renode enables all four supported bit-manipulation subsets for B.
                    bitmanip = BitManip {
                        zba: true,
                        zbb: true,
                        zbc: true,
                        zbs: true,
                        ..bitmanip
                    };
                }
                isa = Some((
                    general || letters.contains('m'),
                    general || letters.contains('a'),
                    legacy_compressed || zca,
                    embedded,
                ));
            }
            ("hartId", value) => id = number(value).map_err(|e| entry.error(&e))? as usize,
            ("privilegedArchitecture", "PrivilegedArchitecture.Priv1_10") => {}
            ("privilegedArchitecture", "PrivilegedArchitecture.PrivUnratified")
            | ("timeProvider", "empty")
                if clic.is_some() => {}
            ("privilegedArchitecture", "PrivilegedArchitecture.Priv1_11")
            | ("timeProvider", "empty")
                if ibex => {}
            ("timeProvider", name)
                if clint
                    .or(litex_cpu_timer)
                    .is_some_and(|timer| timer.name == name)
                    || riscv_machine_timer.is_some_and(|timer| timer.name == name) => {}
            ("builtInIrqController", "true" | "false" | "True" | "False")
                if entry.model == "CPU.VexRiscv" => {}
            ("allowUnalignedAccesses", "true" | "false" | "True" | "False")
                if matches!(entry.model.as_str(), "CPU.RiscV32" | "CPU.IbexRiscV32") => {}
            ("CyclesPerInstruction", value) if number(value).is_ok_and(|cycles| cycles != 0) => {}
            (
                "interruptMode",
                "InterruptMode.Auto"
                | "InterruptMode.Direct"
                | "InterruptMode.Vectored"
                | "0"
                | "1"
                | "2",
            ) if entry.model == "CPU.RiscV32" => {}
            ("privilegeLevels", value) if entry.model == "CPU.RiscV32" => {
                privilege_levels = match value {
                    "PrivilegeLevels.Machine" => (false, false),
                    "PrivilegeLevels.MachineUser" => (true, false),
                    "PrivilegeLevels.MachineSupervisorUser" => (true, true),
                    _ => return Err(entry.error("unsupported RV32 privilegeLevels")),
                };
            }
            ("timeProvider", "empty")
                if matches!(entry.model.as_str(), "CPU.RiscV32" | "CPU.VexRiscv") => {}
            ("zfaFli", "true" | "false") if entry.model == "CPU.RiscV32" => {
                zfa_fli = value == "true";
            }
            ("zfaMinMax", "true" | "false") if entry.model == "CPU.RiscV32" => {
                zfa_min_max = value == "true";
            }
            ("zfaQuietCompare", "true" | "false") if entry.model == "CPU.RiscV32" => {
                zfa_quiet_compare = value == "true";
            }
            ("zfaRound", "true" | "false") if entry.model == "CPU.RiscV32" => {
                zfa_round = value == "true";
            }
            ("vectorBootstrap", "true") if entry.model == "CPU.RiscV32" => {}
            ("pmpEntryCount", "0") => pmp = 0,
            ("pmpEntryCount", "16") => pmp = 16,
            ("pmpEntryCount", "64") => pmp = 64,
            _ => return Err(entry.error(&format!("unsupported CPU property {key} = {value}"))),
        }
    }
    Ok((
        id,
        entry,
        {
            let (multiply, atomic, compressed, embedded) =
                isa.ok_or_else(|| entry.error("missing cpuType"))?;
            if zfa && !floating.0 {
                return Err(entry.error("Zfa requires the RV32 F extension"));
            }
            if zfa_fli && !floating.0 {
                return Err(entry.error("zfaFli requires the RV32 F extension"));
            }
            if zfa_min_max && !floating.0 {
                return Err(entry.error("zfaMinMax requires the RV32 F extension"));
            }
            if zfa_quiet_compare && !floating.0 {
                return Err(entry.error("zfaQuietCompare requires the RV32 F extension"));
            }
            if zfa_round && !floating.0 {
                return Err(entry.error("zfaRound requires the RV32 F extension"));
            }
            if floating.2 && !floating.0 {
                return Err(entry.error("Zfh requires the RV32 F extension"));
            }
            if floating.4 && !floating.0 {
                return Err(entry.error("Zfhmin requires the RV32 F extension"));
            }
            if floating.3 && !floating.0 {
                return Err(entry.error("Zfbfmin requires the RV32 F extension"));
            }
            if smepmp && pmp == 0 {
                return Err(entry.error("Smepmp requires internal PMP entries"));
            }
            if zacas && !(atomic || zaamo) {
                return Err(entry.error("Zacas requires RV32 Zaamo or A"));
            }
            if zabha && !(atomic || zaamo) {
                return Err(entry.error("Zabha requires RV32 Zaamo or A"));
            }
            (
                (multiply, zmmul),
                atomic,
                (compressed, compressed_float),
                embedded,
                zicond,
                (zacas, zabha, zaamo, zalrsc),
                (smepmp, sscofpmf),
                bitmanip,
                (
                    (floating, zfa),
                    (zfa_fli, zfa_min_max, zfa_quiet_compare, zfa_round),
                ),
            )
        },
        pmp,
        privilege_levels,
    ))
}

pub(crate) fn configure_rv32_cpu(
    hart: &mut crate::machine::Hart,
    config: &RiscvConfig<'_>,
    default_vector_mapped: bool,
    external_pmp: Option<(&Entry, usize)>,
) -> Result<(), String> {
    let &(
        id,
        entry,
        (
            (multiply, zmmul),
            atomic,
            (compressed, compressed_float),
            embedded,
            zicond,
            (zacas, zabha, zaamo, zalrsc),
            (smepmp, sscofpmf),
            bitmanip,
            ((floating, zfa), (zfa_fli, zfa_min_max, zfa_quiet_compare, zfa_round)),
        ),
        pmp,
        (user, supervisor),
    ) = config;
    hart.set_hart_id(id);
    hart.name = Some(entry.name.clone());
    if let Some(cycles) = entry
        .body
        .iter()
        .find_map(|(_, text)| {
            let (name, value) = text.split_once(':')?;
            (name.trim() == "CyclesPerInstruction").then(|| number(value.trim()))
        })
        .transpose()
        .map_err(|error| entry.error(&error))?
    {
        hart.set_rv32_cycles_per_instruction(cycles);
    }
    let unaligned = entry
        .body
        .iter()
        .find_map(|(_, text)| {
            let (key, value) = text.split_once(':')?;
            (key.trim() == "allowUnalignedAccesses")
                .then(|| value.trim().eq_ignore_ascii_case("true"))
        })
        .unwrap_or(entry.model == "CPU.IbexRiscV32");
    hart.set_unaligned_accesses(unaligned);
    hart.set_integer_extensions(multiply, atomic || (zaamo && zalrsc), compressed);
    hart.set_compressed_float(compressed_float.0, compressed_float.1);
    hart.set_atomic_subextensions(zaamo, zalrsc);
    hart.set_zmmul(zmmul);
    hart.set_embedded_base(embedded);
    hart.set_zicond(zicond);
    hart.set_zacas(zacas);
    hart.set_zabha(zabha);
    hart.set_sscofpmf(sscofpmf);
    hart.set_bitmanip(bitmanip);
    if floating.0 {
        hart.enable_floating(floating.1, floating.2);
        hart.set_zfbfmin(floating.3);
        hart.set_zfhmin(floating.4);
        hart.set_zfa(zfa);
        hart.set_zfa_diagnostics(zfa_fli, zfa_min_max, zfa_quiet_compare, zfa_round);
    }
    match entry.model.as_str() {
        "CPU.VexRiscv" => hart.enable_vex_core(uses_vex_irq(entry)),
        "CPU.IbexRiscV32" => hart.enable_ibex(),
        "CPU.Minerva" => hart.enable_minerva(),
        _ => {}
    }
    if user {
        hart.enable_user_mode();
    }
    if supervisor {
        hart.enable_supervisor_mode();
    }
    if entry.model == "CPU.RiscV32" {
        hart.enable_address_triggers();
        let reset = if entry.body.iter().any(|(_, line)| line == "reset:") {
            0x7000_0000
        } else {
            0x1000
        };
        hart.configure_reset_vector(reset);
        if default_vector_mapped {
            hart.set_mtvec(DEFAULT_MTVEC);
        }
    }
    if let Some((external, count)) = external_pmp {
        if smepmp {
            return Err(external.error("Smepmp requires internal PMP"));
        }
        hart.attach_external_pmp(format!("{}.{}", entry.name, external.name), count);
        let message = format!("{}: Enabling External PMP", entry.name);
        crate::monitor_log::record(1, &message);
        eprintln!("penv: {message}");
    } else if pmp != 0 {
        hart.enable_pmp(pmp);
        if smepmp {
            hart.enable_smepmp();
        }
    }
    if bitmanip.has_vector_execution()
        || split_rv32_init(entry).0.iter().any(|(_, text)| {
            text.split_once(':').is_some_and(|(key, value)| {
                key.trim() == "vectorBootstrap" && value.trim() == "true"
            })
        })
    {
        hart.enable_vector_bootstrap();
    }
    if let Some(mode) = split_rv32_init(entry).0.iter().find_map(|(_, text)| {
        let (key, value) = text.split_once(':')?;
        if key.trim() != "interruptMode" {
            return None;
        }
        match value.trim() {
            "InterruptMode.Auto" | "0" => Some(InterruptMode::Auto),
            "InterruptMode.Direct" | "1" => Some(InterruptMode::Direct),
            "InterruptMode.Vectored" | "2" => Some(InterruptMode::Vectored),
            _ => None,
        }
    }) {
        hart.configure_interrupt_mode(mode);
    }
    if let Some(commands) = split_rv32_init(entry).1 {
        if commands.is_empty() {
            return Err(entry.error("expected an RV32 init command"));
        }
        for (line, command) in commands {
            let error = |detail: &str| format!("{}:{line}: {detail}", entry.source.display());
            let (register, value) = command
                .split_once(char::is_whitespace)
                .ok_or_else(|| error("expected RV32 init command REGISTER VALUE"))?;
            let value = number(value.trim()).map_err(|detail| error(&detail))?;
            match register {
                "MSTATUS" => hart.set_mstatus(value),
                "MTVEC" => hart.set_mtvec(value),
                _ => return Err(error("expected RV32 init register MSTATUS or MTVEC")),
            }
        }
    }
    Ok(())
}
