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
    if filename == "riscv-extensions.robot" and parameter == "RV32I_Smepmp":
        return "target", "partial", "CPU.RiscV32 Smepmp creation and CSR execution tested; Robot equivalence pending"
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
        return "target", "pending", "mixed-width REPL selects CPU.RiscV32 cpu32_0/cpu32_1; ordered steps and write width required"
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
            scope, status = ("target", "pending") if bits == "32" else ("outside", "outside")
            return scope, status, f"Create RV {bits} Machine; CPU.RiscV{bits} and RiscVExternalPMP with 16 entries"
    if filename == "riscv-zbb-extension.robot":
        selection = re.search(r"Create Machine\s+\$\{(32|64)bit_cpu}", source)
        if selection:
            bits = selection[1]
            scope, status = ("target", "pending") if bits == "32" else ("outside", "outside")
            return scope, status, f"fixed CPU.RiscV{bits} rv{bits}imac_zicsr_zba_zbb fixture"
    if filename == "riscv-andestar-v5.robot":
        selection = re.search(r"(?m)^\s+Create (?:Vector )?Machine(?:[ \t]+([^\n]*))?$", source)
        if selection:
            arguments = selection[1] or ""
            bits = "64" if re.search(r"\bbits=64\b|^64(?:\s|$)", arguments) else "32"
            scope, status = ("target", "pending") if bits == "32" else ("outside", "outside")
            return scope, status, f"Create Machine keyword uses CPU.RiscV{bits}; parameters: {arguments.strip() or 'fixture defaults'}"
    if filename == "riscv-amocas.robot":
        operations = set(re.findall(r"(?m)^\s+Amocas\.[wdq].*? On (RV32|RV64)\b", source))
        if operations == {"RV64"}:
            return "outside", "outside", "AMOCAS keyword selects CPU.RiscV64 from mixed CPU fixture"
        if operations == {"RV32"}:
            return "target", status, "AMOCAS keyword selects CPU.RiscV32 rv32gc_zicsr_zifencei_zacas; " + (basis or "equivalence pending")
    if filename == "riscv-ebreak.robot" and re.search(r"(?m)^\s+Create Machine\s*$", source):
        return "target", "partial", "CPU.VexRiscv rv32gc and LiteX UART; upstream ELF UART sequence tested with reduced ISA"
    if filename in {"memory-assertions.robot", "riscv-bkb.robot", "riscv-mret.robot", "riscv-priv.robot",
                    "riscv-unit-tests.robot", "riscv-vector-csr.robot", "riscv-zicond-extension.robot"}:
        choices = {a or b for a, b in re.findall(
            r"(?m)^\s+Create (?:Machine\s+(?:bitness=)?(32|64)|(32|64) Bit Machine)(?=\s|$)",
            source,
        )}
        if choices == {"64"}:
            return "outside", "outside", "case passes bitness=64 to its CPU fixture"
        if choices == {"32"}:
            return "target", status, "case passes bitness=32 to its CPU fixture; " + (basis or "equivalence pending")
    if filename == "riscv-mret.robot" and "LoadPlatformDescriptionFromString ${invalidate_test_plat}" in source:
        return "target", "partial", "inline CPU.RiscV32 rv32im_zicsr; upstream ELF tested, monitor/log equivalence pending"
    return scope, status, basis or "-"


def nunit_source_variants(attribute, source):
    if "AccessConditionParserTestCases" in attribute:
        return re.findall(r'yield return new TestCaseData\(\s*(".*?"),\s*(".*?")\s*\);', source)
    if attribute == 'TestCaseSource("testCases")':
        return re.findall(r"new Tuple<int, Action>\((\d+), \(\) => (.*?)\),", source)
    return []
