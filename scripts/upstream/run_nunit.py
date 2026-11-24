#!/usr/bin/env python3
"""Discover mapped fixtures in a built Renode checkout and run the Rust batch."""

import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
import xml.etree.ElementTree as ET

sys.dont_write_bytecode = True

from nunit_mapping import PIN, case_parameters, load_mapping


ROOT = Path(__file__).resolve().parents[2]


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args], text=True).strip()


def run(command, cwd, log, timeout=600):
    with log.open("wb") as output:
        return subprocess.run(command, cwd=cwd, stdout=output, stderr=subprocess.STDOUT,
                              timeout=timeout, env={**os.environ, "DOTNET_CLI_UI_LANGUAGE": "en-US"}).returncode


def discover(project, mapping, clone, dotnet, output):
    command = [dotnet, "test", "--no-build", "--configuration", "Release", "--list-tests",
               project, "--", "NUnit.DisplayName=FullName", "NUnit.DumpXmlTestDiscovery=true"]
    log = output / "discovery.log"
    started = time.time_ns()
    if run(command, clone, log):
        raise ValueError(f"NUnit discovery failed, see {log}")
    text = log.read_text()
    assembly = re.search(r"^Test run for (.+?\.dll) \(", text, re.MULTILINE)
    _, marker, listing = text.partition("The following Tests are available:")
    if not assembly or not marker:
        raise ValueError(f"Missing NUnit listing, see {log}")
    dll = Path(assembly[1]).resolve()
    dump = dll.parent / "Dump" / f"D_{dll.name}.dump"
    if not dll.is_relative_to(clone) or dump.stat().st_mtime_ns < started:
        raise ValueError(f"Expected a fresh discovery dump inside {clone}: {dump}")
    xml = output / "discovery.xml"
    shutil.copy2(dump, xml)
    nodes = list(ET.parse(xml).iter("test-case"))
    actual = Counter(node.attrib["fullname"] for node in nodes)
    listed = Counter(line.strip() for line in listing.splitlines() if line.startswith("    "))
    if not actual or actual != listed:
        raise ValueError(f"VSTest and NUnit discovery differ, see {log} and {xml}")

    fixtures = {name.rsplit(".", 1)[0] for name in mapping}
    fixture_cases = Counter(node.attrib["fullname"] for node in nodes
                            if node.attrib["classname"] in fixtures)
    if fixture_cases != Counter(mapping.keys()):
        raise ValueError(f"Mapped fixtures differ from official discovery: {project}")

    cases = []
    for node in nodes:
        case = node.attrib
        name = case["fullname"]
        if name not in mapping:
            continue
        if case["runstate"] != "Runnable":
            raise ValueError(f"Mapped case is {case['runstate']}: {name}")
        cases.append(dict(mapping[name], id=f"{project}::{name}", nunit_id=case["id"],
                          parameters=case_parameters(case["methodname"], case["name"])))
    return cases, dict(project=project, command=command, discovered=len(nodes),
                       xml=str(xml.relative_to(output.parent)), log=str(log.relative_to(output.parent)),
                       assembly_sha256=hashlib.sha256(dll.read_bytes()).hexdigest())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renode", type=Path, default=ROOT.parent / "renode")
    parser.add_argument("--built-renode", type=Path, required=True,
                        help="separate pinned checkout with Release test assemblies already built")
    parser.add_argument("--dotnet", default="dotnet")
    parser.add_argument("--out", type=Path, required=True, help="new evidence directory")
    args = parser.parse_args()
    renode, clone, out = (path.resolve() for path in (args.renode, args.built_renode, args.out))
    if clone.is_relative_to(renode) or out.is_relative_to(renode) or out.is_relative_to(clone):
        parser.error("the built checkout and evidence directory must be separate from the pinned source")
    for path in (renode, clone):
        if git(path, "rev-parse", "HEAD") != PIN or git(path, "status", "--porcelain", "-uno"):
            parser.error(f"expected clean Renode source at {PIN}: {path}")
    infra = Path("src/Infrastructure")
    if git(clone / infra, "rev-parse", "HEAD") != git(renode, "rev-parse", f"HEAD:{infra}"):
        parser.error("Infrastructure checkout does not match the pinned Renode gitlink")
    if git(ROOT, "status", "--porcelain", "-uno"):
        parser.error("commit tracked changes before collecting evidence")

    dotnet_path = shutil.which(args.dotnet)
    if not dotnet_path:
        parser.error(f".NET executable not found: {args.dotnet}")
    dotnet = str(Path(dotnet_path).resolve())
    mapping = load_mapping()
    out.mkdir(parents=True)
    cases, projects = [], []
    for index, project in enumerate(sorted({project for project, _ in mapping}), 1):
        directory = out / f"nunit-{index}"
        directory.mkdir()
        rows, discovery = discover(project, {name: row for (p, name), row in mapping.items()
                                              if p == project}, clone, dotnet, directory)
        cases.extend(rows)
        projects.append(discovery)

    tests = sorted(case["rust_test"] for case in cases)
    command = ["cargo", "test", "--locked", "--color", "never", "--bin", "penv", "--",
               "--exact", "--test-threads=1", *tests]
    log = out / "rust.log"
    status = run(command, ROOT, log)
    rust_output = log.read_text()
    binary = re.search(r"^\s*Running unittests .* \((.+)\)$", rust_output, re.MULTILINE)
    binary_sha = hashlib.sha256((ROOT / binary[1]).read_bytes()).hexdigest() if binary else None
    results = re.findall(r"^test (\S+) \.\.\. (ok|FAILED)$", rust_output, re.MULTILINE)
    if len(dict(results)) != len(results) or not set(dict(results)).issubset(tests):
        raise ValueError(f"Unexpected Rust test results in {log}")
    outcomes = dict(results)
    totals = Counter()
    for case in cases:
        case["status"] = {"ok": "pass", "FAILED": "fail"}.get(outcomes.get(case["rust_test"]), "not_run")
        totals[case["status"]] += 1
    summary = dict(ported=len(cases), newly_ported=sum(case["origin"] == "new" for case in cases),
                   passed=totals["pass"], failed=totals["fail"], not_run=totals["not_run"])
    report = dict(renode_sha=PIN, infrastructure_sha=git(clone / infra, "rev-parse", "HEAD"),
                  penv_sha=git(ROOT, "rev-parse", "HEAD"), discovery=projects, summary=summary,
                  rust=dict(command=command, exit_code=status, log=log.name, binary_sha256=binary_sha), cases=cases)
    (out / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(summary), out)
    return int(status != 0 or totals["pass"] != len(cases))


if __name__ == "__main__":
    sys.exit(main())
