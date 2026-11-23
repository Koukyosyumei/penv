#!/usr/bin/env python3
"""Emit the RV64 source-case inventory from the pinned upstream trees."""
import argparse
import csv
import runpy
from pathlib import Path
import re
import sys

BOARD = {"BeagleV-StarLight.robot", "BeagleV-Fire.robot"}
SHARED = {
    "pmp.robot", "PolarFireSoC.robot", "Icicle-Kit.robot", "SiFive-FU540.robot",
    "SDCard.robot", "STM32SDMMC.robot", "SynopsysDWCEthernetQualityOfService.robot",
    "NetworkServer.robot", "VirtIO.robot", "VirtioMMIOEntropy.robot",
    "VirtioMMIOConsole.robot", "monitor-tests.robot", "memory-access.robot",
    "llvm-asm-disasm.robot", "opcodes-counting.robot", "register-read-caching.robot",
}
FIXTURES = {
    "SDCardEmmcTest.cs", "StorageTests.cs", "DataStorageTests.cs", "PacketTests.cs",
    "SystemBusTests.cs", "MemoryTests.cs", "MultiCPUTests.cs", "DmaEngineTests.cs",
    "MachineTests.cs", "VerificationTests.cs", "MergeAndCreationTests.cs",
}
CASE_HEADER = re.compile(r"\*\*\*\s*(?:Test Cases|Tasks)\s*\*\*\*", re.I)
ATTR = re.compile(r"^\s*\[(Test(?:Case(?:Source)?)?|Theory)\b")
METHOD = re.compile(r"\b(?:public|private|protected|internal)\s+(?:(?:static|async)\s+)*[\w<>?,.]+\s+(\w+)\s*\(")
SIGNAL = re.compile(r"rv64|riscv64|beaglev|polarfire|starfive|cadencegem|riscv_virt|sdcardfromfile", re.I)


def robot_cases(source):
    lines = source.splitlines()
    active = False
    suite_template = any(re.match(r"^Test Template(?:  |\t)", line, re.I) for line in lines)
    for i, line in enumerate(lines):
        if line.startswith("***"):
            active = bool(CASE_HEADER.fullmatch(line.strip()))
        elif active and line and not line[0].isspace() and not line.startswith("#"):
            section = []
            for continuation in lines[i + 1:]:
                if continuation and not continuation[0].isspace():
                    break
                if continuation.strip():
                    section.append(continuation)
            name = re.split(r"\s{2,}|\t", line.strip(), 1)[0]
            yield i + 1, name, section, suite_template


def nunit_cases(source):
    attributes = []
    for no, line in enumerate(source.splitlines(), 1):
        if ATTR.match(line):
            attributes.append((no, line.strip()))
        if attributes and (method := METHOD.search(line)):
            for attr_line, attr in attributes:
                yield attr_line, method[1], [attr], False
            attributes.clear()


def disposition(path, name, context):
    if path.name in BOARD:
        return "partial", "Existing CLI run; exact Robot semantics require audit"
    if path.name == "riscv-amo-instructions.robot":
        if "MMIO" in name:
            return "pending", "ArrayMemory MMIO semantics"
        return "pass", "amoadd_robot_single_and_page_spanning_ram"
    if path.name == "riscv-amocas.robot":
        if "On RV32" in name:
            return "out", "Uses the RV32 CPU and RV32 register semantics"
        if "Page-Spanning" in name:
            if name.startswith("Amocas.w "):
                return "partial", "Robot expects zero-extended old word; Penv sign-extends"
            return "pass", "amocas_page_spanning_ram_matches_and_mismatches"
        if "Single-Page" in name:
            if name.startswith("Amocas.w "):
                return "partial", "Robot expects zero-extended old word; Penv sign-extends"
            return "pass", "amocas_{d,q}_{changes_single_page_on_match,preserves_single_page_on_mismatch}"
        register_cases = {
            "Zero Source": "amocas_q_zero_source_ignores_x1",
            "Zero Destination": "amocas_q_zero_destination_does_not_write_x1",
            "Odd Registers": "amocas_q_traps_on_odd_register_pairs",
        }
        for text, test in register_cases.items():
            if text in name:
                return "pass", test
        return "pending", "ArrayMemory MMIO semantics"
    if path.name == "riscv-ebreak.robot":
        return "out", "Fixture loads an RV32 VexRiscv ELF and LiteX UART"
    if path.name == "riscv-mret.robot":
        if name.endswith(" 32Bits") or name == "Should Invalidate TLB After Mode Change":
            return "out", "RV32 CPU and firmware fixture"
        return "pass", "mret_robot_rv64_privilege_levels (MachineUser / Machine)"
    if path.name == "riscv-bkb.robot":
        if name.endswith(" RV32") or re.search(r" (?:Zip|Unzip) ", name):
            return "out", "Explicit Create 32 Bit Machine fixture"
        if "Packw" in name:
            return "partial", "Pinned Robot expects zero extension; Penv sign-extends"
        return "pass", "zbkb_robot_rv64_pack_and_brev8"
    if path.name == "riscv-zicond-extension.robot":
        if name.endswith(" RV32"):
            return "out", "Explicit Create 32 Bit Machine fixture"
        return "pass", "zicond_robot_rv64_selectors_and_destination_aliases / zicond_robot_traps_when_extension_disabled"
    if path.name == "riscv-priv.robot":
        if name.endswith(" RV32"):
            return "out", "Explicit Create Machine 32 fixture"
        return "partial", "Prior Rust port; assertion equivalence remains under review"
    if path.name in {"riscv-isa-tests.robot", "riscv-arch-tests.robot"}:
        if name.endswith(" 32"):
            return "out", "Harness selects only RV32 executable parameters"
        return "unexpanded", "One row per executable required from the upstream selector"
    if path.name.startswith("riscv-") or path.name == "SDCardEmmcTest.cs":
        return "pending", "Check RV64 fixture or shared card behavior"
    if path.name in SHARED or path.name in FIXTURES or SIGNAL.search(context):
        return "review", "Determine applicability from individual fixture and model"
    return "review", "No explicit RV64 marker; check shared fixtures before excluding"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("renode", type=Path)
    parser.add_argument("infrastructure", type=Path)
    parser.add_argument("--ui", type=Path)
    parser.add_argument("--tlib", type=Path)
    parser.add_argument("--dependency", nargs=2, action="append", default=[], metavar=("NAME", "SOURCE"))
    parser.add_argument("--isa", type=Path)
    parser.add_argument("--arch", type=Path)
    args = parser.parse_args()
    writer = csv.writer(sys.stdout, delimiter="\t", lineterminator="\n")
    writer.writerow(("tree", "file", "line", "case", "parameter_source", "status", "evidence_or_reason"))
    trees = [("renode", args.renode), ("infrastructure", args.infrastructure),
             ("ui", args.ui), ("tlib", args.tlib)]
    trees.extend((name, Path(source)) for name, source in args.dependency)
    for label, tree in trees:
        if tree is None:
            continue
        if not tree.is_dir():
            parser.error(f"missing source directory: {tree}")
        for path in sorted(p for p in tree.rglob("*") if p.suffix in {".robot", ".cs"}):
            relative = path.relative_to(tree)
            source = path.read_text(encoding="utf-8-sig", errors="replace")
            extractor = robot_cases if path.suffix == ".robot" else nunit_cases
            for no, name, body, suite_template in extractor(source):
                status, reason = disposition(path, name, "\n".join(body))
                if path.suffix == ".cs" and body[0].startswith("[TestCaseSource"):
                    writer.writerow((label, relative, no, name, body[0], "unexpanded",
                                     "NUnit data source must be evaluated to enumerate arguments"))
                    continue
                if status == "unexpanded":
                    arch = path.name == "riscv-arch-tests.robot"
                    directory = args.arch if arch else args.isa
                    selector = "TESTS_ARCH_64" if arch else "TESTS_64"
                    if directory is not None:
                        get_variables = runpy.run_path(str(path.with_suffix(".py")))["get_variables"]
                        values = get_variables(str(directory))[selector]
                        if values:
                            for value in sorted(values):
                                parameter = Path(value).relative_to(directory) if arch else value
                                writer.writerow((label, relative, no, name, parameter, "pending",
                                                 "Upstream executable parameter; equivalence unverified"))
                            continue
                    parameter_source = selector
                elif (suite_template or any(line.lstrip().startswith("[Template]") for line in body)) and not any(
                    re.match(r"^\s*(?:FOR|WHILE|IF)(?:  |\t)", line) for line in body
                ):
                    for line in body:
                        parameter = line.strip()
                        if not parameter.startswith(("[", "#", "...")):
                            variant_status, variant_reason = status, reason
                            if path.name in {"riscv-isa-string.robot", "riscv-extensions.robot"} and parameter.lower().startswith("rv32"):
                                variant_status, variant_reason = "out", "Template constructs CPU.RiscV32"
                            writer.writerow((label, relative, no, name, parameter, variant_status, variant_reason))
                    continue
                elif path.suffix == ".cs" or status != "review" or path.name in BOARD | SHARED:
                    parameter_source = " | ".join(line.strip() for line in body)
                else:
                    parameter_source = " | ".join(line.strip() for line in body
                                                    if "[Template]" in line or " IN " in line)
                writer.writerow((label, relative, no, name, parameter_source, status, reason))


if __name__ == "__main__":
    main()
