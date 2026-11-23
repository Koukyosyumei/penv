#!/usr/bin/env python3
"""Run each ACT parameter through the fixed Robot fixture in its own process."""
import argparse
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import os
import re
import resource
import runpy
import signal
import subprocess
import xml.etree.ElementTree as ET


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("renode", type=Path, help="built checkout of the fixed Renode revision")
    parser.add_argument("work", type=Path, help="ACT work directory")
    parser.add_argument("--logs", type=Path, required=True)
    parser.add_argument("--jobs", type=int, default=6)
    args = parser.parse_args()
    renode, work, logs = args.renode.resolve(), args.work.resolve(), args.logs.resolve()
    suite = renode / "tests/unit-tests/riscv-arch-tests.robot"
    selector = runpy.run_path(str(suite.with_suffix(".py")))["get_variables"]
    paths = sorted(Path(name) for name in selector(str(work))["TESTS_ARCH_64"])
    if not paths:
        parser.error("upstream selector found zero RV64 ELF files")
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))

    def run(item):
        index, elf = item
        directory = logs / f"{index:03d}"
        inputs = directory / "input/spike-rv64-max"
        inputs.mkdir(parents=True, exist_ok=True)
        link = inputs / elf.name
        if not link.is_symlink():
            link.symlink_to(elf)
        elif link.resolve() != elf.resolve():
            raise ValueError(f"stale input link: {link}")
        with (directory / "console.log").open("wb") as output:
            with subprocess.Popen(
                [renode / "renode-test", suite, "--fixture", "Should Run Arch Tests 64",
                 "--variable", f"RISCV_ARCH_TESTS_PATH:{inputs.parent}",
                 "--results-dir", directory, "-P", "0", "--show-log", "--test-timeout", "15"],
                stdout=output, stderr=subprocess.STDOUT, start_new_session=True,
            ) as process:
                try:
                    code = process.wait(timeout=45)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
                    return str(elf.relative_to(work)), "timeout"
        console = (directory / "console.log").read_text(errors="replace")
        xml = directory / "robot_output.xml"
        try:
            tree = ET.parse(xml)
            robot_status = tree.find(".//test/status").get("status")
        except (FileNotFoundError, ET.ParseError, AttributeError):
            robot_status = None
        if code == 0 and robot_status == "PASS":
            reports = re.findall(r'RVCP-SUMMARY: Test File "([^"]+)": (\w+)', console)
            expected = elf.with_suffix(".S").name
            finish = "tohost: TEST FINISHED" in console and "tohost: TEST PASSED" in console
            status = "signature-and-robot-pass" if finish and reports == [(expected, "PASSED")] else "signature-unverified"
        elif "CPU abort" in console or "Machine aborted" in console:
            status = "cpu-abort"
        else:
            status = "robot-fail"
        return str(elf.relative_to(work)), status

    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        for name, status in pool.map(run, enumerate(paths, 1)):
            print(name, status, sep="\t", flush=True)


if __name__ == "__main__":
    main()
