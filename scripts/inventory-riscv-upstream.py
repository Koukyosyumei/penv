"""Inventory pinned Renode cases. Runtime results and applicability require review."""
import argparse
import csv
import importlib.util
from itertools import product
from pathlib import Path
import re
import subprocess
import sys

from robot.api import get_model
from robot.parsing.model.blocks import TestCase
from robot.parsing.model.statements import TemplateArguments

sys.dont_write_bytecode = True

from riscv_scope import nunit_source_variants, reservation_variants, robot_review

REVISIONS = {
    "renode": "c38ac3babcd38a7144b9498e3ca4c2226b76aa37",
    "infrastructure": "09ac7c563095c5410258ecc1b1a6b841a013db85",
    "ui": "5bc51e6eb4b1c36421527c1fab8c0fc3f6b6947f",
}
MAX_ISA_BLOCKER = "upstream CPU creation requires V"
DIRECT = re.compile(r"(?i)litex|vexriscv|minerva|nexys|riscv_virt|CPU\.RiscV32|bitness=32|"
                    r"ns16550|corelevelinterruptor|platformlevelinterruptcontroller")
NUNIT = re.compile(
    r"(\[Test(?:CaseSource|Case)?(?:\([^\n]*\))?\]"
    r"(?:\s*\[[^\n]+\])*)\s*(?:public|private|protected|internal)\s+"
    r"(?:static\s+)?(?:async\s+)?[\w<>.]+\s+(\w+)\s*\(", re.M
)


def upstream_variables(source, directory):
    spec = importlib.util.spec_from_file_location("renode_case_discovery", source)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.get_variables(str(directory))


def robot_cases(root, isa, act):
    for path in sorted((root / "tests").rglob("*.robot")):
        name = path.relative_to(root).as_posix()
        source = path.read_text(encoding="utf-8-sig")
        dedicated = "/LiteX/" in name or path.name == "litex_vexriscv_verilated_cfu.robot"
        explicit = DIRECT.search(source) or path.name.startswith("riscv-") or path.name in {
            "pmp.robot", "AndesNCEPLIC100.robot", "CLIC-riscv-arch-tests.robot",
        }
        scope = "target" if dedicated else ("review-variants" if explicit else "review-shared")
        for section in get_model(source).sections:
            for case in getattr(section, "body", ()):
                if not isinstance(case, TestCase):
                    continue
                case_scope, case_reason = scope, ""
                parameters = [
                    (row.lineno, " | ".join(row.args))
                    for row in case.body if isinstance(row, TemplateArguments)
                ]
                if case.name in {"Should Run Tests 64", "Should Run Arch Tests 64"}:
                    yield ("Renode", name, case.lineno, case.name, "", "outside", "outside",
                           "Run Test Case width=64 selects CPU.RiscV64 and the RV64 ELF discovery list")
                    continue
                if case.name in {"Should Run Tests 32", "Should Run Arch Tests 32"}:
                    directory, key = ((act, "TESTS_ARCH_32") if "Arch" in case.name
                                      else (isa, "TESTS_32"))
                    values = upstream_variables(path.with_suffix(".py"), directory)[key]
                    if not values:
                        yield ("Renode", name, case.lineno, case.name, "", "target",
                               "missing-input", "upstream discovery found zero instances")
                    for value in sorted(values):
                        portable = str(Path(value).relative_to(directory)) if "Arch" in case.name else value
                        bitmanip_checked = (
                            re.match(r"rv32uzb[abcs]-[pv]-", portable) if key == "TESTS_32"
                            else re.fullmatch(r"spike-rv32-max/elfs/rv32i/Zb[abcs]/[^/]+\.elf", portable)
                        )
                        basis = MAX_ISA_BLOCKER
                        if bitmanip_checked:
                            basis += "; reduced-ISA bitmanip diagnostic passed"
                        if re.fullmatch(r"spike-rv32-max/elfs/rv32i/Zcb(?:M|Zbb)?/[^/]+\.elf", portable):
                            basis += "; fixed ISA omits Zcb; reduced-ISA Zcb diagnostic passed"
                        if re.fullmatch(r"rv32uzbkx-[pv]-xperm[48]|spike-rv32-max/elfs/rv32i/Zbkx/[^/]+\.elf", portable):
                            basis += "; fixed ISA omits Zbkx; reduced-ISA Zbkx diagnostic passed"
                        if re.fullmatch(r"spike-rv32-max/elfs/rv32i/(?:Zabha|ZacasZabha)/[^/]+\.elf", portable):
                            basis += "; fixed ISA omits Zabha; reduced-ISA Zabha/Zacas ELF passed"
                        if re.fullmatch(r"spike-rv32-max/elfs/rv32i/Zmmul/[^/]+\.elf", portable):
                            basis += "; ELF passed both standalone Zmmul and fixed ISA with V omitted"
                        if re.fullmatch(r"spike-rv32-max/elfs/rv32i/Zfbfmin/[^/]+\.elf", portable):
                            basis += "; fixed ISA omits Zfbfmin"
                            if Path(portable).name == "Zfbfmin-fcvt.s.bf16-00.elf":
                                basis += "; reduced ISA fails pinned bad NaN-boxing signature (expected 0x7e000000)"
                            else:
                                basis += "; reduced-ISA Zfbfmin ELF passed"
                        if re.fullmatch(r"spike-rv32-max/elfs/rv32i/Zbkc/[^/]+\.elf", portable):
                            basis += "; ELF passed both standalone Zbkc and fixed ISA with V omitted"
                        if re.fullmatch(r"spike-rv32-max/elfs/rv32i/Zknh/[^/]+\.elf", portable):
                            basis += "; fixed ISA omits Zknh; reduced-ISA Zknh ELF passed"
                        if re.fullmatch(r"spike-rv32-max/elfs/rv32i/Zksh/[^/]+\.elf", portable):
                            basis += "; fixed ISA omits Zksh; reduced-ISA Zksh ELF passed"
                        if portable == "spike-rv32-max/elfs/rv32i/Zacas/Zacas-amocas.w-00.elf":
                            basis += "; reduced-ISA Zacas diagnostic passed"
                        if re.fullmatch(r"spike-rv32-max/elfs/rv32i/(?:F|D|Zcd|Zcf)/[^/]+\.elf", portable):
                            if re.fullmatch(r"[FD]-f(?:add|div|mul|sub|n?madd|n?msub)\.[sd]-0[12]\.elf",
                                            Path(portable).name):
                                basis += "; F/D diagnostic: exceeds 1 MiB RAM, passes with 2 MiB"
                            else:
                                basis += "; F/D diagnostic passed with 1 MiB RAM"
                        if key == "TESTS_32" and re.match(r"rv32u[fd]-[pv]-", portable):
                            basis += "; F/D ISA diagnostic passed with 2 MiB RAM"
                        if re.fullmatch(r"spike-rv32-max/elfs/rv32i/Zfh(?:min)?D?/[^/]+\.elf", portable):
                            if re.fullmatch(r".*/Zfh/Zfh-f(?:(?:add|div|mul|sub)\.h-01|n?m(?:add|sub)\.h-0[12])\.elf", portable):
                                basis += "; Zfh diagnostic: exceeds 1 MiB RAM, passes with 2 MiB"
                            else:
                                basis += "; Zfh diagnostic passed with 1 MiB RAM"
                        if key == "TESTS_32" and re.match(r"rv32uzfh-[pv]-", portable):
                            basis += "; Zfh ISA diagnostic passed with 2 MiB RAM"
                        if portable == "spike-rv32-max/elfs/priv/Sm/Sm-00.elf":
                            basis += "; vector-bootstrap diagnostic with Sail MSTATUS init passes full signature"
                        if portable == "spike-rv32-max/elfs/priv/UV/UV.elf":
                            basis += "; vector-bootstrap diagnostic with CLINT exits 0; upstream vector test body is a stub"
                        if key == "TESTS_32" and re.match(r"rv32[ms]i-", portable):
                            basis += "; privileged ISA diagnostic passed with Smepmp enabled"
                        yield ("Renode", name, case.lineno, case.name, portable, "target",
                               "blocked-cpu", basis)
                    continue
                status = "outside" if case_scope == "outside" else "pending"
                if path.name == "LiteX_VexRiscv_I2S.robot" and case.name == "Should Echo Audio":
                    status, case_reason = "ported", "validate-riscv-upstream.sh: same input, 3.2s, Dispose and cmp"
                elif path.name == "riscv-amocas.robot" and case.name.startswith("Amocas.d On RV32"):
                    case_scope, status = "target", "partial"
                    case_reason = "Rust register-pair and trap checks; Robot equivalence pending"
                elif any(type(node).__name__ == "For" for node in case.body):
                    case_reason = case_reason or "review nested parameter loop"
                if path.name == "riscv-lr-sc-strong-atomicity.robot" and case.name == "Writes To Reservation Should Cause Invalidation":
                    parameters = [(case.lineno, variant) for variant in reservation_variants()]
                if path.name == "riscv-isa-string.robot" and case.name == "Should Handle Both 32 And 64 Bit Versions":
                    # Both constructs are required by this single test instance.
                    case_scope = status = "outside"
                    case_reason = "requires mixed CPU.RiscV32 and CPU.RiscV64 construction"
                if path.name == "riscv-extensions.robot" and case.name == "Should Fail On Unsupported ISA":
                    parameters = [(case.lineno, "RV32GC_invalid_ISA")]
                lines = source.splitlines()
                body = "\n".join(lines[case.lineno - 1:case.end_lineno])
                if "tests/peripherals/CLIC/CLIC-test-setup.resc" in body:
                    artifacts = re.findall(r"https://[^\s]+/(clic[^/\s]+)", body)
                    if artifacts:
                        parameters = [(case.lineno, " | ".join(artifacts))]
                if path.name == "riscv-mret.robot":
                    privilege = re.search(r"(?m)^\s+Create Machine\s+bitness=(32|64)\s+privilege=PrivilegeLevels\.(MachineUser|Machine)\s*$", body)
                    if privilege:
                        parameters = [(case.lineno, f"bitness={privilege[1]} | privilegeLevels={privilege[2]}")]
                if path.name == "riscv-interrupt-mode.robot":
                    mode = re.search(r"(?m)^\s+Create Machine\s+(Auto|Direct|Vectored)\s*$", body)
                    if mode:
                        parameters = [(case.lineno, f"interruptMode={mode[1]}")]
                seen = set()
                for line, parameter in parameters or [(case.lineno, "")]:
                    if parameter in seen:
                        continue
                    seen.add(parameter)
                    row_status, row_reason = status, case_reason
                    if parameter == "RV32IMC_Zicsr_Zifencei_Zbs" and path.name == "riscv-isa-string.robot":
                        row_status = "partial"
                        row_reason = "penv loads exact CPU string; Renode log behavior still to compare"
                    scope_out, status_out, basis = robot_review(
                        path.name, case.name, body, parameter, case_scope, row_status, row_reason,
                    )
                    yield ("Renode", name, line, case.name, parameter, scope_out, status_out, basis)


def argument_variants(declaration):
    dimensions = []
    for attribute, arguments in re.findall(r"\[(Values|Range)\(([^\]]*)\)\]", declaration):
        if attribute == "Values":
            dimensions.append(next(csv.reader([arguments], skipinitialspace=True)))
            continue
        try:
            start, stop, *steps = (int(value.strip()) for value in arguments.split(","))
            step = steps[0] if steps else 1
        except ValueError:
            return []
        dimensions.append([str(value) for value in range(start, stop + 1, step)])
    return [" | ".join(values) for values in product(*dimensions)] if dimensions else []


def nunit_cases(component, root):
    for path in sorted(root.rglob("*.cs")):
        name = path.relative_to(root).as_posix()
        if not re.search(r"(?:^|/)\w*Tests?(?:/|\.cs$)", name):
            continue
        source = path.read_text(encoding="utf-8-sig")
        for match in NUNIT.finditer(source):
            attributes, method = match.groups()
            parameters = re.findall(r"\[(TestCase(?:Source)?\([^\n]*?\))\]", attributes)
            declaration = source[match.end():].partition("{")[0]
            argument_rows = argument_variants(declaration)
            source_rows = []
            if len(parameters) == 1 and parameters[0].startswith("TestCaseSource"):
                source_rows = [" | ".join(row) for row in nunit_source_variants(parameters[0], source)]
            for parameter in dict.fromkeys(source_rows or parameters or argument_rows or [""]):
                basis = ""
                if source_rows:
                    basis = "expanded fixed NUnit TestCaseSource; applicability pending"
                elif parameter.startswith("TestCaseSource"):
                    basis = "expand NUnit data source"
                elif "[Values" in declaration or "[Range" in declaration:
                    basis = "NUnit argument tuple" if argument_rows else "expand NUnit argument data"
                yield (component, name, 1 + source.count("\n", 0, match.start()), method,
                       parameter, "review-shared", "pending", basis or "-")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("renode", "infrastructure", "ui", "isa", "act"):
        parser.add_argument(name, type=Path)
    args = parser.parse_args()
    for name, directory in vars(args).items():
        if not directory.is_dir():
            parser.error(f"{name} directory does not exist: {directory}")
    for name, revision in REVISIONS.items():
        directory = getattr(args, name)
        head = subprocess.check_output(["git", "-C", str(directory), "rev-parse", "HEAD"], text=True)
        if head.strip() != revision:
            parser.error(f"{name} must be at {revision}")
    writer = csv.writer(sys.stdout, dialect="excel-tab", lineterminator="\n")
    writer.writerow(("component", "file", "line", "case", "parameter", "scope", "status", "basis"))
    writer.writerows(robot_cases(args.renode, args.isa, args.act))
    for component, root in (
        ("Renode", args.renode), ("Infrastructure", args.infrastructure), ("UI", args.ui),
    ):
        writer.writerows(nunit_cases(component, root))


if __name__ == "__main__":
    main()
