#!/usr/bin/env python3
"""Join fixed Renode ACT reference outcomes and a Penv run by ELF parameter."""

import argparse
import csv
from pathlib import Path
import re
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


def summarize_load_ranges(writer, ranges_path, upstream_logs, reference, run_directory):
    writer.writerow(("upstream_file", "case_line", "parameter", "skipped_start",
                     "skipped_end_exclusive", "load_boundary", "renode_fixed",
                     "penv_execution", "equivalence", "penv_log", "renode_log"))
    by_parameter = {row["parameter"]: row for row in reference}
    with ranges_path.open() as stream:
        ranges = list(csv.DictReader(stream, delimiter="\t"))
    for row in ranges:
        parameter = row["parameter"]
        ref = by_parameter[parameter]
        end = int(row["load_end_exclusive"], 16)
        start = end - int(row["bytes_after_robot_ram_end"])
        relative = Path("robot") / f"{parameter}.log"
        log = (run_directory / relative).read_text()
        upstream = (upstream_logs / ref["direct_log"]).read_text()
        renode_ranges = {
            (int(begin, 16), int(last, 16) + 1)
            for begin, last in re.findall(
                r"Tried to access bytes at non-existing peripheral in range <(0x[0-9a-fA-F]+), (0x[0-9a-fA-F]+)>",
                upstream,
            )
        }
        penv_ranges = {
            (int(begin, 16), int(limit, 16))
            for begin, limit in re.findall(r"skipped unmapped ELF64 range (0x[0-9a-f]+)\.\.(0x[0-9a-f]+)", log)
        }
        boundary = "matched" if (start, end) in renode_ranges & penv_ranges else "different"
        if fault := re.search(r"exception (\d+),", log):
            execution = f"exception-{fault[1]}"
        elif "instruction limit reached" in log:
            execution = "instruction-limit"
        else:
            execution = "see-log"
        writer.writerow((ref["upstream_file"], ref["case_line"], parameter, hex(start), hex(end),
                         boundary, ref["renode_fixed"], execution, "pending", relative, ref["direct_log"]))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("run_directory", type=Path)
    parser.add_argument("--load-ranges", type=Path)
    parser.add_argument("--upstream-logs", type=Path)
    args = parser.parse_args()
    if (args.load_ranges is None) != (args.upstream_logs is None):
        parser.error("--load-ranges requires --upstream-logs and vice versa")
    with args.reference.open() as stream:
        reference = list(csv.DictReader(stream, delimiter="\t"))
    with (args.run_directory / "robot.tsv").open() as stream:
        results = list(csv.reader(stream, delimiter="\t"))
    results_by_elf = dict(results)
    parameters = {row["parameter"] for row in reference}
    if len(results) != len(results_by_elf) or parameters != results_by_elf.keys():
        parser.error("run and reference must contain the same unique ELF parameters")

    writer = csv.writer(sys.stdout, delimiter="\t", lineterminator="\n")
    if args.load_ranges is not None:
        summarize_load_ranges(writer, args.load_ranges, args.upstream_logs, reference, args.run_directory)
        return
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
