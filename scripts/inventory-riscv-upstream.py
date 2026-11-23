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

REVISIONS = {
    "renode": "c38ac3babcd38a7144b9498e3ca4c2226b76aa37",
    "infrastructure": "09ac7c563095c5410258ecc1b1a6b841a013db85",
    "ui": "5bc51e6eb4b1c36421527c1fab8c0fc3f6b6947f",
}
MAX_ISA_BLOCKER = "upstream CPU creation requires F, D, V, Zfh, Zacas, Smepmp"
BOARD_REGRESSION = {
    "Arty_LiteX_VexRiscv.robot",
    "LiteX_Linux_VexRiscv.robot",
    "LiteX_Tock.robot",
    "LiteX_VexRiscv_Linux_SDCard.robot",
    "LiteX_VexRiscv_SMP.robot",
    "Nexys4DDR_LiteX_VexRiscv.robot",
    "Nexys_Video_LiteX_VexRiscv.robot",
}
BOARD_OUTSIDE = {
    "LiteX_Ibex.robot": "CPU.IbexRiscV32 board outside the requested CPU list",
    "LiteX_Microwatt.robot": "Power ISA board",
}
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
        reason = ""
        if path.name in BOARD_OUTSIDE:
            scope, reason = "outside", BOARD_OUTSIDE[path.name]
        for section in get_model(source).sections:
            for case in getattr(section, "body", ()):
                if not isinstance(case, TestCase):
                    continue
                case_scope, case_reason = scope, reason
                if re.search(r"\bRV64\b|\b64Bits\b|^Should Run (?:Arch )?Tests 64$", case.name, re.I):
                    if not dedicated:
                        case_scope, case_reason = "outside", "case explicitly requires RV64"
                parameters = [
                    (row.lineno, " | ".join(row.args))
                    for row in case.body if isinstance(row, TemplateArguments)
                ]
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
                        yield ("Renode", name, case.lineno, case.name, portable, "target",
                               "blocked-cpu", basis)
                    continue
                status = "outside" if case_scope == "outside" else "pending"
                if path.name == "LiteX_VexRiscv_I2S.robot" and case.name == "Should Echo Audio":
                    status, case_reason = "ported", "validate-riscv-upstream.sh: same input, 3.2s, Dispose and cmp"
                elif path.name in BOARD_REGRESSION:
                    case_reason = "board regression available; compare expectations case by case"
                elif any(type(node).__name__ == "For" for node in case.body):
                    case_reason = case_reason or "review nested parameter loop"
                for line, parameter in parameters or [(case.lineno, "")]:
                    if parameter == "RV32IMC_Zicsr_Zifencei_Zbs" and path.name == "riscv-isa-string.robot":
                        yield ("Renode", name, line, case.name, parameter, "target", "partial",
                               "penv loads exact CPU string; Renode log behavior still to compare")
                    else:
                        yield ("Renode", name, line, case.name, parameter, case_scope, status, case_reason or "-")


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
            for parameter in parameters or argument_rows or [""]:
                basis = ""
                if parameter.startswith("TestCaseSource"):
                    basis = "expand NUnit data source"
                elif "[Values" in declaration or "[Range" in declaration:
                    basis = "NUnit argument tuple" if argument_rows else "expand NUnit argument data"
                yield (component, name, 1 + source.count("\n", 0, match.start()), method,
                       parameter, "review-shared", "pending", basis or "-")


def main():
    sys.dont_write_bytecode = True
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
