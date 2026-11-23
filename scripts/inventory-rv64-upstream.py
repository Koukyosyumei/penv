#!/usr/bin/env python3
"""Emit the RV64 source-case inventory from the pinned upstream trees."""
import argparse
import csv
import json
import runpy
from pathlib import Path
import re
import sys

from rv64_scope import CELLS, RobotFixtures

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
CASE_HEADER = re.compile(r"\*{3,}\s*(?:Test Cases|Tasks)\s*\*{3,}", re.I)
ATTR = re.compile(r"^\s*\[(Test(?:Case(?:Source)?)?|Theory)\b")
METHOD = re.compile(r"\b(?:public|private|protected|internal)\s+(?:(?:static|async|virtual|override|sealed|new|unsafe)\s+)*[\w<>?,.]+\s+(\w+)\s*\(")
SIGNAL = re.compile(r"rv64|riscv64|beaglev|polarfire|starfive|cadencegem|riscv_virt|sdcardfromfile", re.I)


def robot_cases(source):
    lines = source.splitlines()
    active = False
    suite_template = any(re.match(r"^Test Template(?:  |\t)", line, re.I) for line in lines)
    for i, line in enumerate(lines):
        if line.startswith("***"):
            active = bool(CASE_HEADER.fullmatch(line.strip()))
        elif active and line and not line[0].isspace() and not line.startswith(("#", "...")):
            row = re.split(r"\s{2,}|\t", line.strip(), 1)
            name = row[0]
            section = row[1:]
            for continuation in lines[i + 1:]:
                if continuation and not continuation[0].isspace() and not continuation.startswith("..."):
                    break
                if continuation.strip():
                    section.append(continuation)
            yield i + 1, name, section, suite_template


def template_parameters(body):
    rows = []
    collecting = False
    for line in body:
        line = line.strip()
        if line.startswith(("[", "#")):
            collecting = False
        elif line.startswith("..."):
            if collecting:
                rows[-1] += "  " + line[3:].strip()
        else:
            rows.append(line)
            collecting = True
    return rows


def robot_dependencies(cases):
    providers = {}
    for _, _, body, _ in cases:
        for line in body:
            columns = CELLS.split(line.strip())
            if len(columns) == 2 and columns[0] == "Provides":
                providers[columns[1]] = body
    return providers


def dependency_body(body, providers, seen=frozenset()):
    for line in body:
        columns = CELLS.split(line.strip())
        if len(columns) != 2 or columns[0] != "Requires":
            continue
        state = columns[1]
        if state in providers and state not in seen:
            setup = providers[state]
            yield from setup
            yield from dependency_body(setup, providers, seen | {state})


def nunit_cases(source):
    attributes = []
    for no, line in enumerate(source.splitlines(), 1):
        if ATTR.match(line):
            attributes.append((no, line.strip()))
        if attributes and (method := METHOD.search(line)):
            # NUnit uses Test as a marker on a parameterized method, not an
            # additional invocation without arguments.
            parameterized = any(not re.fullmatch(r"\[Test\]", attr) for _, attr in attributes)
            for attr_line, attr in attributes:
                if parameterized and attr == "[Test]":
                    continue
                yield attr_line, method[1], [attr], False
            attributes.clear()


def nunit_parameters(source, attribute, path):
    if 'nameof(AccessConditionParserTestCases)' in attribute:
        return re.findall(r'yield return new TestCaseData\(\s*("[^"]*",\s*"[^"]*")\s*\);', source)
    if 'TestCaseSource("testCases")' in attribute and 'class TypeExtensionsCacheTests' in source:
        return re.findall(r'^\s*new Tuple<int, Action>\((.*)\),', source, re.M)
    if "nameof(testCases)" in attribute and "class SystemRDLMem1PeripheralTest" in source:
        return re.findall(r"new TestCase\s*\{([^}]+)\}", source)
    if "nameof(GetTestCases)" in attribute and "class SystemRDLGenTest" in source:
        metadata = json.loads(path.with_name("systemrdl.json").read_text())
        return [f"{entry['class']}; {entry['file']}; {entry['peakrdl'][-1]}" for entry in metadata]
    return []


def fixture_status(status, reason, fixtures, corpus, context):
    if status not in {"review", "pending"}:
        return status, reason
    models = {model for model, _, _ in fixtures}
    if "CPU.RiscV64" in models:
        return "pending", "RV64 CPU fixture: " + fixture_description(fixtures)
    if any(model == "Network.SynopsysDWCEthernetQualityOfService" and re.search(r"\.robot:\d+$", origin)
           for model, _, origin in fixtures):
        return "out", ("Requires Synopsys DWC QoS MTL/DMA register regions and Cortex-A9/Zynq "
                       "Linux fixture; StarLight uses the distinct SynopsysEthernetMAC BeagleV model")
    if models == {"CPU.ARMv8R"} and re.search(
        r"\b(?:PMSA|MPU|TCM|CPSR|SPSR|VBAR|ELR_hyp|HYP)\b", context, re.I
    ):
        return "out", "Requires Cortex-R52 ARMv8-R architecture state: " + fixture_description(fixtures)
    if models and models <= {"CPU.RiscV32", "CPU.VexRiscv", "CPU.CV32E40P"}:
        description = fixture_description(fixtures)
        if (models == {"CPU.CV32E40P"} or "CLIC-test-platform.repl" in description
                or (corpus == "riscv" and re.search(r"\b(?:Machine|CPU|rv32|RV 32)\b", context, re.I))):
            return "out", "Requires 32-bit CPU configuration: " + description
        return "review", ("Fixture uses " + description
                          + "; verify whether its assertions require that configuration")
    return "review", reason


def fixture_description(fixtures):
    return "; ".join(f"{model}{' ' + isa if isa else ''} ({origin})"
                     for model, isa, origin in fixtures)


def packet_fixture(source, line, tree):
    fragment = []
    for text in source.splitlines()[line:]:
        if fragment and ATTR.match(text):
            break
        fragment.append(text)
    method = "\n".join(fragment)
    if re.search(r"^\s*(?:namespace Ieee80211|using PacketDotNet\.Ieee80211)\b", source, re.M):
        radio_input = re.search(
            r'\b(?:new|typeof\s*\()\s*(\w*(?:Frame|RadioTap\w*|Ppi\w*|Field|InformationElement)\w*)'
            r'|"(?:[^"]*/)?([^"/]*80211[^"/]*\.pcap)"',
            method,
            re.I,
        )
        if radio_input:
            fixture = radio_input[1] or radio_input[2]
            return "out", (f"Requires IEEE 802.11 link framing/radio metadata ({fixture}); "
                           "BeagleV uses wired EthernetFrame and RV64 virt has no Wi-Fi device")
    captures = re.findall(r'"[^"]*/([^"/]+\.pcap)"', method)
    if not captures and "ResetPacketPosition()" in method:
        captures = re.findall(r'prefix \+ "([^"]+\.pcap)"', source)
    link_types = set()
    for capture in captures:
        path = tree / "Test" / "CaptureFiles" / capture
        if not path.is_file():
            continue
        with path.open("rb") as stream:
            header = stream.read(24)
        byte_order = {b"\xd4\xc3\xb2\xa1": "little", b"\xa1\xb2\xc3\xd4": "big"}.get(header[:4])
        if byte_order and len(header) == 24:
            link_types.add(int.from_bytes(header[20:24], byte_order) & 0xffff)
    if link_types == {113}:
        return "out", ("Requires Linux cooked capture link type 113 from " + ", ".join(captures)
                       + "; the supported BeagleV switch supplies Ethernet link type 1")
    if 1 in link_types:
        return "pending", "Wired Ethernet capture fixture: " + ", ".join(captures)
    if helper := re.search(r'\b(ChecksumUtils|EndianBitConverter|EndianBinaryReader|NetworkToHostOrder)\b', method):
        return "pending", f"Shared wired packet checksum or network byte-order behavior ({helper[1]})"
    protocol = re.search(
        r'\b(EthernetPacket|ARPPacket|IPv[46]Packet|IPPacket|ICMPv[46]Packet|'
        r'IGMPv2Packet|UdpPacket|TcpPacket|VLanTaggedFrame|LldpPacket|WakeOnLanPacket)\b',
        method, re.I,
    )
    if protocol:
        return "pending", (f"Shared Packet.Net wired-network codec ({protocol[1]}); "
                           "BeagleV EthernetFrame uses Packet.ParsePacket with LinkLayers.Ethernet")
    return "review", "Verify the asserted link type, packet format and use from the wired Ethernet path"


# Constructors in NUnit fixture setup. These device models are absent from
# the target REPLs and their imported CPU descriptions.
OTHER_DEVICES = {
    "OpenTitan_I2C", "EFR32_I2CController", "CAST_CANCTRL", "OpenTitan_AES",
    "OpenTitan_KeyManager", "EFM32GGI2CController", "LPC43xx_GPIO",
    "Cadence_TTC", "Cadence_UART", "FT5336", "SHT21", "STMCAN", "BMA180", "BMC050",
}


def infrastructure_fixture(source, line):
    # Local Setup may instantiate more than one device. The first recognized
    # model is the controller under test for multi-device setups.
    devices = [device for device in re.findall(r"\bnew\s+(\w+)\s*\(", source)
               if device in OTHER_DEVICES]
    if devices and re.search(r"\[SetUp\]|\[TestFixture\]", source):
        return "out", (f"Requires NUnit {devices[0]} model and fixture register/protocol behavior; "
                       "absent from Fire, StarLight and RV64 virt REPL composition")
    program = "\n".join(source.splitlines()[line:])
    if re.search(r"\bnew (?:BusRangeRegistration|BusPointRegistration|CPURegistrationPoint|MappedMemory)\(", source):
        if re.search(r"\b(?:class SystemBusTests|class MultiCPUTests|class MemoryTests)\b", source):
            return "pending", ("Shared machine-bus registration, memory or multi-hart arbitration fixture; "
                               "RV64 BeagleV and virt use the same bus contracts")
    if "AccessConditionParser.Parse" in program and "class AccessConditionParserTests" in source:
        return "pending", "Shared conditional sysbus registration expression grammar"
    return "review", "Verify the selected NUnit fixture and its use by the three target configurations"


def disposition(path, name, context):
    if path.name in BOARD:
        return "partial", "Existing CLI run; exact Robot semantics require audit"
    if path.name == "per-core-registration.robot":
        if name in {
            "Get Same Read From Common Memory",
            "Values Written By One Core Should Not Be Visible By The Other One",
        }:
            return "pass", "Fixed REPL, public ELF and ordered UART assertions verified by verify-rv64-per-core.py"
        if name in {
            "Write Values Using Sysbus",
            "Write Values Using Sysbus With Context",
            "Handle Being Simultaneously Registered On The Main Bus",
            "Read Values Using Sysbus Context",
        }:
            return "pass", "Fixed REPL and ELF with matching contextual or global memory and UART assertions"
        if name in {
            "Cluster Memory Should Only Be Visible To Cluster",
            "Memory Should Only Be Movable Within Single CPU",
            "Memory Move Should Only Affect Relevant CPU",
        }:
            return "pass", "Fixed REPL, stopped setup, move rejections and memory assertions verified by verify-rv64-ram-move.py"
    if path.name == "offset-bus-registrations.robot":
        if name in {
            "Cpu Should Read From Correct Offset From Mapped Memory",
            "Cpu Should Read From Correct Offset From Array Memory",
        }:
            return "pass", "Fixed REPL, PC, initialization, load and register result verified by rv64-offset-bus.S"
        if name.startswith("Sysbus Should "):
            return "pass", "Fixed REPL and same stopped bus read/write assertions verified by verify-rv64-offset-bus.py"
    if path.name == "riscv-amo-instructions.robot":
        if "MMIO" in name:
            return "pass", "Original CPU and ArrayMemory REPL with two ordered register and memory snapshots"
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
        if "MMIO" in name and name.startswith(("Amocas.d ", "Amocas.q ")):
            return "partial", "Renode passes; Rust checks the same values at the MMIO address using RAM backing"
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
    if path.name == "riscv-zbb-extension.robot" and name in {
        "Should Handle RORI Properly on 64 Bit CPU",
        "Should Handle RORIW Properly",
        "Should Handle SLLI.UW Properly",
    }:
        return "pass", "Fixed CPU, PC, instruction bytes and all stepwise a0 assertions checked"
    if path.name == "riscv-vector-csr.robot" and name in {
        "Should Trap On VXRM Write When MSTATUS.VS Is Zero",
        "Should Trap On VXSAT Write When MSTATUS.VS Is Zero",
        "Should Trap On VCSR Write When MSTATUS.VS Is Zero",
        "Should Keep VXRM Upper Bits As Zero",
        "Should Keep VXSAT Upper Bits As Zero",
        "Should Keep VCSR Upper Bits As Zero",
        "Should Reflect VXSAT In VCSR",
        "Should Reflect VXRM In VCSR",
        "Should Reflect VCSR In VXSAT And VXRM",
        "Should Reflect VSADD.VV Saturation In VCSR",
        "Should Mark MSTATUS.VS As Dirty",
        "Should Leave Enough VSTART Bits For Largest Possible Value",
    }:
        return "pass", "Fixed CPU ISA including Zcb; CSR, trap and ELF assertions verified (rv64-zcb.md)"
    if path.name == "riscv-unit-tests.robot" and name in {
        "Should Keep MEPC First Bit As Zero",
        "Should Keep SEPC First Bit As Zero",
    }:
        return "pass", "Fixed RV64 ISA, two steps and monitor EPC value 0xfe verified by ELF"
    if path.name == "riscv-unit-tests.robot" and name in {
        "Should Not Read From Csr When CSRRW RD Is Zero",
        "Should Not Read From Csr When CSRRWI RD Is Zero",
    }:
        return "pass", "Fixed ISA and single-step ELF produced only the expected custom CSR write notification"
    if path.name == "riscv-unit-tests.robot":
        if name == "Should Ignore Writes To MENVCFG Bits Of Disabled Or Unsupported Extensions":
            return "pass", "Fixed Renode mask and three-step ELF register result verified (rv64-menvcfg.md)"
        if name in {
            "Should Set Flag On Single Precision Floating Point Division By Zero",
            "Should Set Flag On Half Precision Floating Point Division By Zero",
        }:
            return "pass", "Fixed Renode reset profile, NaN-boxed operands, one step, PC, f2 and FFLAGS verified"
    if path.name == "riscv-unit-tests.robot" and name == "Should Properly Handle ResetVector After Creation And After Reset 64":
        return "pass", "Fixed CPU, reset and all 12 PC observations verified by verify-rv64-reset-vector.py"
    if path.name == "riscv-unit-tests.robot" and name in {
        "Should Handle LD", "Should Fail LD", "Should Handle SD", "Should Fail SD",
    }:
        return "pass", "Fixed instruction words, operands, PC and trap cause verified (rv64-ldsd.md)"
    if path.name == "riscv-extensions.robot" and re.search(r"\bRV32(?:I_|GC_invalid_ISA)", context):
        return "out", "Create Machine selects CPU.RiscV32 from the fixed ISA argument's rv32 prefix"
    if path.name == "memory-access.robot":
        if name == "Should Report Illegal Address For Instruction Fetch":
            return "pass", "Fixed RV64 illegal physical fetch diagnostic and abort verified (rv64-physical-address.md)"
        if name == "Should Report Illegal Address For Store":
            return "pass", "Fixed fixture executes LW; physical address diagnostic and trap cause 5 verified (rv64-physical-address.md)"
    if path.name == "riscv-external-pmp.robot" and name in {
        "Should Access All 64-Bit Address Registers From Monitor",
        "Should Access All 64-Bit Address Registers From CPU",
    }:
        return "pass", "Fixed 16-entry external PMP, all addresses and no invalid accesses verified (rv64-external-pmp.md)"
    if path.name == "riscv-external-pmp.robot" and name in {
        "Should Access All 64-Bit Config Registers From Monitor",
        "Should Access All 64-Bit Config Registers From CPU",
    }:
        return "partial", "Fixed runner skips the config loop; Penv pmpcfg0 round-trip separately verified (rv64-external-pmp.md)"
    if path.name == "riscv-external-pmp.robot" and re.search(
        r"Should Access All 32 Bit (?:Config|Address) Registers From Monitor", context
    ):
        return "out", "Fixture creates CPU.RiscV32 with external PMP and checks the RV32 monitor CSR layout"
    if path.name == "riscv-isa-string.robot" and name == "Should Handle Both 32 And 64 Bit Versions":
        return "partial", ("Fixed RV64 ISA string checked by Rust and CLI; "
                           "source case also constructs a separate RV32 CPU")
    if path.name == "riscv-isa-string.robot" and (
        re.search(r"\[Template\]\s+Create RISC-V With ISA String", context)
        or re.search(r"\[Template\]\s+Should Throw Construction Exception", context)
    ):
        return "out", "Template constructs CPU.RiscV32 and asserts its ISA string acceptance or rejection"
    if path.name == "pmp.robot" and name in {
        "Should Trap On Odd pmpcfg Access In 64-bit Mode",
        "Should Trap On Out-of-Bound pmpaddr",
    }:
        return "partial", "Instruction and trap assertions ported; upstream warning text not verified"
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

    def emit(row, configuration=None):
        key = tuple(row[:2]) + (row[3], row[4] if configuration is None else configuration)
        if key not in emitted:
            writer.writerow(row)
            emitted.add(key)

    emitted = set()
    for label, tree in trees:
        if tree is None:
            continue
        if not tree.is_dir():
            parser.error(f"missing source directory: {tree}")
        for path in sorted(p for p in tree.rglob("*") if p.suffix in {".robot", ".cs"}):
            relative = path.relative_to(tree)
            source = path.read_text(encoding="utf-8-sig", errors="replace")
            extractor = robot_cases if path.suffix == ".robot" else nunit_cases
            fixtures = RobotFixtures(source, path, tree.resolve()) if path.suffix == ".robot" else None
            cases = list(extractor(source))
            providers = robot_dependencies(cases) if fixtures else {}
            for no, name, body, suite_template in cases:
                context = "\n".join(body)
                status, reason = disposition(path, name, context)
                corpus = "riscv" if path.stem.startswith("riscv-") else ""
                if path.suffix == ".cs" and re.search(r"\bTestCaseSource\(", body[0]):
                    parameters = nunit_parameters(source, body[0], path)
                    if not parameters:
                        emit((label, relative, no, name, body[0], "unexpanded",
                              "NUnit data source must be evaluated to enumerate arguments"))
                        continue
                    if name == "TypeExtensionsCacheTest":
                        status, reason = "out", ("Requires .NET MethodInfo, PropertyInfo and FieldInfo "
                                                 "and private TypeExtensions cache miss counters")
                    elif name == "TestStaticMeta":
                        status, reason = "out", ("Requires PeakRDL C# generator and reflection over "
                                                 "generated CLR nested types and fields")
                    elif name == "TestDynamicMeta":
                        status, reason = "pending", ("Generated mock register behavior and field modes "
                                                     "share MMIO contracts; reflection fixture requires a port")
                    elif "class SystemRDLMem1PeripheralTest" in source:
                        status, reason = "pending", "Shared MMIO word access and register field packing"
                    else:
                        status, reason = "pending", ("Shared conditional sysbus registration expression "
                                                     "grammar; fixture asserts exact DNF output")
                    for parameter in parameters:
                        emit((label, relative, no, name, " ".join(parameter.split()), status, reason))
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
                                if arch and re.fullmatch(r"spike-rv64-max/elfs/rv64i/Zalrsc/Zalrsc-sc\.[dw]-00\.elf", str(parameter)):
                                    emit((label, relative, no, name, parameter, "pass",
                                          "Fixed Robot and Penv signature and watcher verified; rv64-act-unmapped.tsv"))
                                else:
                                    emit((label, relative, no, name, parameter, "pending",
                                          ("Fixed ACT result; see rv64-act-unmapped.tsv"
                                           if arch else "Upstream executable parameter; equivalence unverified")))
                            continue
                    parameter_source = selector
                elif (suite_template or any(line.lstrip().startswith("[Template]") for line in body)) and not any(
                    re.match(r"^\s*(?:FOR|WHILE|IF)(?:  |\t)", line) for line in body
                ):
                    parameters = template_parameters(body)
                    for parameter in parameters:
                        variant_status, variant_reason = fixture_status(
                            status, reason, fixtures.evidence(body, parameter), corpus,
                            f"{name}\n{context}")
                        if path.name == "memory-access.robot" and name == "Should Throw Monitor Exception":
                            if parameter.startswith("${PLAT_RISCV64}"):
                                variant_status = "pass"
                                variant_reason = "Fixed RV64 read translation rejects the illegal physical address (rv64-physical-address.md)"
                            elif parameter.startswith("${PLAT_RISCV32}"):
                                variant_status = "out"
                                variant_reason = "Fixed template uses CPU.RiscV32 rv32i and tests its 32-bit address limit"
                        emit((label, relative, no, name, parameter, variant_status, variant_reason))
                    if parameters:
                        continue
                    parameter_source = "Template has no static argument rows"
                    status, reason = "review", "Verify no-argument or dynamically supplied template execution"
                elif path.suffix == ".cs" or status != "review" or path.name in BOARD | SHARED:
                    parameter_source = " | ".join(line.strip() for line in body)
                else:
                    parameter_source = " | ".join(line.strip() for line in body
                                                    if "[Template]" in line or " IN " in line)
                if fixtures and status != "unexpanded":
                    scope_body = body + list(dependency_body(body, providers))
                    status, reason = fixture_status(status, reason, fixtures.evidence(scope_body), corpus, f"{name}\n{context}")
                    if status != "review":
                        parameter_source = " | ".join(line.strip() for line in body)
                    if status == "review" and any("SdCardFromFile" in line for line in body):
                        status, reason = "pending", ("Shared SdCardFromFile command and image handling; "
                                                     "Fire uses this command with MPFS SD/eMMC")
                elif label == "packet-net":
                    status, reason = packet_fixture(source, no, tree)
                elif label == "fdtsharp" and 'GetBinaryLocation("xilinx_zynq_iic.dtb")' in source:
                    status, reason = "pending", ("Shared FDT binary format used by RV64 virt and BeagleV; "
                                                 "xilinx_zynq_iic.dtb is parsed as data, without a CPU fixture")
                elif path.suffix == ".cs" and "class SDCardEmmcTest" in source:
                    status, reason = "pending", ("Shared SDCard eMMC mode, constructor and partition "
                                                 "semantics; Fire uses MPFS SD/eMMC")
                elif label == "infrastructure":
                    status, reason = infrastructure_fixture(source, no)
                elif status == "pending":
                    status = "review"
                emit((label, relative, no, name, parameter_source, status, reason),
                     configuration=tuple(line.strip() for line in body))


if __name__ == "__main__":
    main()
