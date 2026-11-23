#!/usr/bin/env python3
"""Join fixed Renode ACT reference outcomes and a Penv run by ELF parameter."""

import argparse
import csv
from pathlib import Path
import sys


def reason(result, log, parameter):
    if result == "diagnostic-pass":
        return "signature-and-watcher-pass"
    if "unsupported RV64 ISA" in log:
        return "cpu-configuration"
    if "unmapped ELF64 segment" in log:
        if parameter.endswith(".sig.elf"):
            return "elf-outside-ram;signature-generation-intermediate"
        return "elf-outside-ram"
    if "exception 4," in log:
        return "load-address-misaligned"
    if "instruction limit reached" in log:
        return "instruction-limit"
    if "TEST FINISHED" in log:
        return "finished-without-passing-signature-and-exit-code"
    return "missing-watcher-completion"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("run_directory", type=Path)
    args = parser.parse_args()
    with args.reference.open() as stream:
        reference = list(csv.DictReader(stream, delimiter="\t"))
    with (args.run_directory / "robot.tsv").open() as stream:
        results = list(csv.reader(stream, delimiter="\t"))
    results_by_elf = dict(results)
    parameters = {row["parameter"] for row in reference}
    if len(results) != len(results_by_elf) or parameters != results_by_elf.keys():
        parser.error("run and reference must contain the same unique ELF parameters")

    writer = csv.writer(sys.stdout, delimiter="\t", lineterminator="\n")
    writer.writerow(("upstream_file", "case_line", "parameter", "renode_fixed",
                     "penv_fixed_inputs", "penv_result", "equivalence", "penv_log"))
    for row in reference:
        parameter = row["parameter"]
        relative = Path("robot") / f"{parameter}.log"
        log = (args.run_directory / relative).read_text()
        cause = reason(results_by_elf[parameter], log, parameter)
        writer.writerow((row["upstream_file"], row["case_line"], parameter, row["renode_fixed"],
                         results_by_elf[parameter], cause, "pending", relative))


if __name__ == "__main__":
    main()
