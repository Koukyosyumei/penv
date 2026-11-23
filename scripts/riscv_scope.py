"""Scope decisions for reviewed, pinned Renode fixtures."""
from itertools import product
import re

LITEX = {
    "Arty_LiteX_VexRiscv.robot": "arty_litex_vexriscv.resc; VexRiscv, LiteX UART, GPIO and PHY",
    "LiteX_Linux_VexRiscv.robot": "litex_linux_vexriscv.repl; VexRiscv, GPIO, SPI and I2C",
    "LiteX_Tock.robot": "litex_vexriscv_tock.resc; ArtyA7 VexRiscv and LiteX UART",
    "LiteX_VexRiscv_Linux_SDCard.robot": "Create Platform; VexRiscv, LiteX SD card controller",
    "LiteX_VexRiscv_SMP.robot": "litex_vexriscv_smp REPL; four VexRiscv harts",
    "Nexys4DDR_LiteX_VexRiscv.robot": "nexys4ddr_litex_vexriscv.repl; VexRiscv and SPI SD",
    "Nexys_Video_LiteX_VexRiscv.robot": "litex_nexys_video_vexriscv_linux.resc; VexRiscv and LiteX video",
}

# Lines refer to the pinned fixture or its named setup. Dependent cases
# inherit the board established by their Provides/Requires chain.
OTHER_LITEX = {
    ("LiteX_Ibex.robot", "Should Boot"): "litex_ibex.resc -> litex_ibex.repl:10 requires CPU.IbexRiscV32",
    ("LiteX_Ibex.robot", "Should Display Help"): "boot-finished fixture from Should Boot requires CPU.IbexRiscV32",
    ("LiteX_Microwatt.robot", "Should Boot"): "litex_microwatt.resc -> litex_microwatt.repl:16 requires CPU.PowerPc64",
    ("LiteX_Microwatt.robot", "Should Display Help"): "boot-finished fixture from Should Boot requires CPU.PowerPc64",
}


def reservation_variants():
    for cpu, address in product(("RV32", "RV64"), ("0x81000000", "0x00100000")):
        native = "w" if cpu == "RV32" else "d"
        writes = (f"lr.{native}+sc.{native} (2 steps)", f"amoswap.{native}", f"amoadd.{native}")
        writes += ("sb", "sh", "sw") if cpu == "RV32" else ("sb", "sh", "sw", "sd")
        for write in writes:
            yield f"{cpu} | {address} | {write}"


def robot_review(filename, case, source, parameter, scope, status, basis):
    other_board = OTHER_LITEX.get((filename, case))
    if other_board:
        return "outside", "outside", other_board
    if filename in LITEX:
        return "target", "partial", LITEX[filename] + "; firmware regression, Robot equivalence pending"
    if filename == "riscv-extensions.robot" and parameter in {"RV32I_Smepmp", "RV32I_Sscofpmf"}:
        return "target", "ported", f"fixed CPU-only {parameter} fixture constructs without RAM or firmware"
    if filename == "riscv-extensions.robot" and re.fullmatch(
        r"RV32I_(?:V|Zvfh|Zve32[xf]|Zve64[xfd])", parameter
    ):
        return "target", "blocked-cpu", f"fixed CPU-only fixture requires unsupported {parameter.split('_', 1)[1]}"
    if (filename, case) in {
        ("riscv-isa-string.robot", "Should Throw Construction Exception"),
        ("riscv-extensions.robot", "Should Fail On Unsupported ISA"),
    }:
        return "target", "ported", "fixed CPU.RiscV32 construction rejected with expected diagnostic"
    if filename == "riscv-isa-string.robot" and case == "Should Parse All Valid ISA Strings":
        if re.fullmatch(r"rv32(?:[imafdcgb_]+|_?e[mac]*)(?:_?z(?:icsr|ifencei|bs|icond))*(?:_xandes)?", parameter.lower()):
            return "target", "ported", "fixed ISA spelling constructs CPU.RiscV32 without RAM or firmware"
        return "target", "pending", "construction requires V"
    if filename in {"riscv-isa-string.robot", "riscv-extensions.robot"} and parameter.lower().startswith("rv32"):
        return "target", status, "CPU.RiscV32 construction keyword; " + (basis or "fixed ISA argument")
    if filename == "pmp.robot":
        if re.search(r"(?m)^\s+Create RV64PRIV10 Machine\s*$", source):
            return "outside", "outside", "Create RV64PRIV10 Machine uses CPU.RiscV64 rv64gcv with 16 PMP entries"
        if re.search(r"(?m)^\s+Create RV32(?:PRIV10| No PMP) Machine\s*$", source):
            return "target", "partial", "RV32 inline PMP fixture; Rust checks pinned addresses and results with reduced ISA"
    if filename == "riscv-lr-sc-strong-atomicity.robot" and parameter:
        if parameter.startswith("RV64"):
            return "outside", "outside", "riscv-lr-sc-strong-atomicity.repl selects CPU.RiscV64 cpu_0/cpu_1"
        return "target", "ported", "fixed RV32 hart IDs 2/3, both memory models and original ordered steps; shared memory and failed SC checked"
    if filename == "riscv-interrupt-mode.robot":
        mode = re.search(r"(?m)^\s+Create Machine\s+(Auto|Direct|Vectored)\s*$", source)
        if mode:
            fixture = f"CPU.RiscV32 rv32imac, Priv1_10, interruptMode={mode[1]}"
            if "Written By Software" in case:
                return "target", "ported", fixture + "; fixed guest opcodes, readbacks and warning sequence checked"
            return "target", "ported", fixture + "; ordered monitor writes, readbacks and warnings checked"
    if filename == "timer-reconfiguration.robot" and re.search(r"(?m)^\s+Create Machine\s*$", source):
        return "target", "pending", "Create Machine uses CPU.RiscV32 rv32ic, 1 MHz CLINT and block hooks"
    if re.search(r"(?m)^\s+Execute Script\s+tests/peripherals/CLIC/CLIC-test-setup\.resc\s*$", source):
        return "outside", "outside", "CLIC-test-setup.resc -> CLIC-test-platform.repl: CoreLocalInterruptController and PrivUnratified; virt uses PLIC/CLINT"
    if filename == "AndesNCEPLIC100.robot" and "plic ReadDoubleWord 0x1080" in source:
        return "outside", "outside", "Create Machine loads egis_et171.repl: AndesNCEPLIC100 trigger-type register"
    if filename == "riscv-zcb-zcmp-zcmt-extensions.robot" and re.search(r"Create Machine (?:With|Without) Extensions", source):
        return "outside", "outside", "selected inline fixture instantiates CPU.CV32E40P; separate core from CPU.RiscV32/VexRiscv"
    if filename == "riscv-stack-access-hook.robot" and re.search(r"(?m)^\s+Create Machine\s*$", source):
        return "outside", "outside", "Create Machine instantiates CPU.RiscV64 rv64gc_V"
    fixed_rv64 = {
        "riscv-amo-instructions.robot": "inline CPU.RiscV64 AMO fixture",
        "riscv-custom-instructions.robot": "inline CPU.RiscV64 rv64imac_zicsr fixture",
        "riscv-lr-sc-instructions.robot": "polarfire-soc.repl uses RV64 E51/U54 harts",
    }
    if filename in fixed_rv64 and re.search(r"(?m)^\s+Create Machine\s*$", source):
        return "outside", "outside", fixed_rv64[filename]
    if filename == "riscv-external-pmp.robot":
        selection = re.search(r"(?m)^\s+(?:Should Access All (32|64) Bit|Create RV (32|64) Machine)", source)
        if selection:
            bits = selection[1] or selection[2]
            if bits == "32":
                return "target", "ported", "fixed RV32 16-entry external PMP; monitor and guest CSR values and diagnostic ordering checked"
            return "outside", "outside", "Create RV 64 Machine selects CPU.RiscV64 and RiscVExternalPMP"
    if filename == "riscv-zbb-extension.robot":
        selection = re.search(r"Create Machine\s+\$\{(32|64)bit_cpu}", source)
        if selection:
            bits = selection[1]
            if bits == "32" and case == "Should Handle RORI Properly on 32 Bit CPU":
                return "target", "ported", "fixed RV32 fixture and four instruction encodings; a0 checked after each step"
            scope, status = ("target", "pending") if bits == "32" else ("outside", "outside")
            return scope, status, f"fixed CPU.RiscV{bits} rv{bits}imac_zicsr_zba_zbb fixture"
    if filename == "riscv-andestar-v5.robot":
        selection = re.search(r"(?m)^\s+Create (?:Vector )?Machine(?:[ \t]+([^\n]*))?$", source)
        if selection:
            arguments = selection[1] or ""
            bits = "64" if re.search(r"\bbits=64\b|^64(?:\s|$)", arguments) else "32"
            if bits == "32" and case in {
                "Should Box Scalar BF16 With FLEN Thirty Two", "Should Widen Scalar BF16",
                "Should Narrow Scalar BF16", "Should Round Scalar BF16 In Every Rounding Mode",
                "Should Reject Scalar BF16 With Floating Point Disabled",
                "Should Reject Reserved BF16 Rounding Modes",
            }:
                return "target", "ported", "fixed RV32 scalar BF16 inputs; PC, full NaN boxing, FFLAGS and illegal traps checked"
            if bits == "32" and case in {
                "Should Store Half Words With SHGP", "Should Execute LBGP", "Should Execute LBUGP",
                "Should Execute LHGP", "Should Execute LHUGP", "Should Execute LWGP",
                "Should Wrap ADDIGP At Thirty Two Bits", "Should Decode SBGP Displacements",
                "Should Decode SHGP Displacements", "Should Decode SWGP Displacements",
                "Should Decode Nineteen Bit LWGP Displacements",
            }:
                return "target", "ported", "fixed RV32 XandesPerf GP operands, memory inputs and result assertions checked on ELF"
            if bits == "32" and case in {
                "Should Execute BBC", "Should Execute BBS", "Should Execute BEQC",
                "Should Execute BNEC", "Should Execute BFOZ", "Should Execute BFOS",
            }:
                return "target", "ported", "fixed RV32 XandesPerf branch or field inputs; PC and register results checked"
            if bits == "32" and case in {
                "Should Execute LEA H", "Should Execute LEA W", "Should Execute LEA D",
                "Should Execute FFB", "Should Execute FFZMISM", "Should Execute FFMISM",
                "Should Execute FLMISM",
            }:
                return "target", "ported", "fixed RV32 XandesPerf register operands and single-step PC/results checked on ELF"
            scope, status = ("target", "pending") if bits == "32" else ("outside", "outside")
            return scope, status, f"Create Machine keyword uses CPU.RiscV{bits}; parameters: {arguments.strip() or 'fixture defaults'}"
    if filename == "riscv-amocas.robot":
        operations = set(re.findall(r"(?m)^\s+Amocas\.[wdq].*? On (RV32|RV64)\b", source))
        if operations == {"RV64"}:
            return "outside", "outside", "AMOCAS keyword selects CPU.RiscV64 from mixed CPU fixture"
        if operations == {"RV32"}:
            return "target", "ported", "fixed RV32 ISA and hartId 2; single-step registers, memory and illegal-instruction assertions checked with inactive RV64 omitted"
    if filename == "riscv-ebreak.robot" and re.search(r"(?m)^\s+Create Machine\s*$", source):
        return "target", "ported", "fixed VexRiscv rv32gc/timeProvider, RAM, LiteX UART and ELF; six ordered UART lines matched"
    if filename in {"memory-assertions.robot", "riscv-bkb.robot", "riscv-mret.robot", "riscv-priv.robot",
                    "riscv-unit-tests.robot", "riscv-vector-csr.robot", "riscv-zicond-extension.robot"}:
        choices = {a or b for a, b in re.findall(
            r"(?m)^\s+Create (?:Machine\s+(?:bitness=)?(32|64)|(32|64) Bit Machine)(?=\s|$)",
            source,
        )}
        if choices == {"64"}:
            return "outside", "outside", "case passes bitness=64 to its CPU fixture"
        if choices == {"32"}:
            if filename == "riscv-bkb.robot":
                return "target", "ported", "fixed rv32gc_zbkb fixture; original single-step operands, results and no-trap assertions checked"
            if filename == "riscv-zicond-extension.robot":
                return "target", "ported", "pinned RV32 operands or trap PC, MCAUSE and MEPC checked on ELF"
            if filename == "riscv-mret.robot" and case in {
                "User Level Exists 32Bits", "User Level Does Not Exists 32Bits",
            }:
                return "target", "ported", "fixed rv32gc fixture and instruction words; both single-step PC and MPP assertions checked"
            if filename == "riscv-unit-tests.robot" and case in {
                "Should Set MEPC on Illegal Instruction",
                "Should Set MEPC on Illegal CSR access",
                "Should Set MEPC on Non-Existing CSR access",
                "Should Clear MTVAL On EBREAK And ECALL",
                "Should Allow SRET In Machine Mode",
            }:
                return "target", "ported", "fixed rv32gc_Zfh fixture; opcode, step count, and all PC/CSR assertions checked"
            if filename == "riscv-unit-tests.robot" and case in {
                "Should Handle LB", "Should Handle LH", "Should Handle LW",
                "Should Handle SB", "Should Handle SH", "Should Handle SW",
                "Should Fail LH", "Should Fail LW", "Should Fail SH", "Should Fail SW",
            }:
                return "target", "ported", "fixed rv32gc_Zfh fixture; opcode and a1 preserved, single-step PC and trapping MCAUSE checked"
            if filename == "riscv-unit-tests.robot" and case == "Should Fail On Setting X0 register":
                return "target", "ported", "fixed RV32 fixture; monitor write of 1 rejects as read-only and same hart reads back 0"
            if filename == "riscv-unit-tests.robot" and case == "Should Properly Handle ResetVector After Creation And After Reset 32":
                return "target", "ported", "fixed RV32 fixture; ordered ResetVector, explicit PC, two CPU resets and all PC readbacks checked"
            if filename == "riscv-unit-tests.robot" and case == "Should Exit Translation Block After Invalid Instruction And Report Single Error":
                return "target", "ported", "fixed non-V program and run window; diagnostic at first opcode and absence at later opcodes checked"
            if filename == "riscv-priv.robot":
                basis = "fixed RV32 fixture requires unsupported V; separated C and Zcb are available"
                if re.fullmatch(r"Should (?:Increment|Inhibit) M(?:CYCLE|INSTRET)(?: After Write Finishes)? RV32", case):
                    basis += "; original counter input and a3 assertions passed with V omitted"
                return "target", "blocked-cpu", basis
            return "target", status, "case passes bitness=32 to its CPU fixture; " + (basis or "equivalence pending")
    if filename == "riscv-mret.robot" and "LoadPlatformDescriptionFromString ${invalidate_test_plat}" in source:
        return "target", "ported", "inline CPU.RiscV32 rv32im_zicsr; pinned ELF logs unrecoverable before fail_loop, which is observable in a negative control"
    return scope, status, basis or "-"


def nunit_source_variants(attribute, source):
    if "AccessConditionParserTestCases" in attribute:
        return re.findall(r'yield return new TestCaseData\(\s*(".*?"),\s*(".*?")\s*\);', source)
    if attribute == 'TestCaseSource("testCases")':
        return re.findall(r"new Tuple<int, Action>\((\d+), \(\) => (.*?)\),", source)
    return []
