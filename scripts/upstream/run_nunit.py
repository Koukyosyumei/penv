#!/usr/bin/env python3
"""Discover mapped fixtures in a built Renode checkout and run the Rust batch."""

import argparse
from collections import Counter, defaultdict
import csv
import hashlib
import json
import os
import platform
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
import xml.etree.ElementTree as ET

sys.dont_write_bytecode = True

from nunit_mapping import PIN, case_parameters, load_mapping, runtime_host_excluded


ROOT = Path(__file__).resolve().parents[2]


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args], text=True).strip()


def run(command, cwd, log, timeout=600):
    with log.open("wb") as output:
        return subprocess.run(command, cwd=cwd, stdout=output, stderr=subprocess.STDOUT,
                              timeout=timeout, env={**os.environ, "DOTNET_CLI_UI_LANGUAGE": "en-US"}).returncode


def manifest_projects(source):
    manifest = git(source, "show", "HEAD:tests/tests.yaml")
    projects = re.findall(r"^- ([^\s]+\.csproj)$", manifest, re.MULTILINE)
    if not projects or len(projects) != len(set(projects)):
        raise ValueError("Invalid NUnit project list in the pinned manifest")
    return projects


def case_nodes(node, categories=frozenset()):
    categories = categories.union(
        prop.attrib["value"] for prop in node.findall("properties/property[@name='Category']")
    )
    if node.tag == "test-case":
        yield node.attrib, categories
    else:
        for child in node:
            if child.tag in {"test-run", "test-suite", "test-case"}:
                yield from case_nodes(child, categories)


def host_exclusions():
    system = {"Linux": "linux", "Darwin": "osx"}.get(platform.system(), "windows")
    arch = "aarch64" if platform.machine() in {"aarch64", "arm64"} else "i386"
    return {"excluded", f"exclude_{system}", f"exclude_host_{arch}"}


def fixture_notes():
    with Path(__file__).with_name("nunit_gaps.tsv").open(newline="") as source:
        rows = list(csv.DictReader(source, delimiter="\t"))
    notes = {row["fixture"]: row["missing_api"] for row in rows}
    if len(notes) != len(rows):
        raise ValueError("Duplicate fixture in NUnit gap notes")
    return notes


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
    nodes = list(case_nodes(ET.parse(xml).getroot()))
    actual = Counter(case["fullname"] for case, _ in nodes)
    listed = Counter(line.strip() for line in listing.splitlines() if line.startswith("    "))
    if not actual or actual != listed:
        raise ValueError(f"VSTest and NUnit discovery differ, see {log} and {xml}")

    exclusions = host_exclusions()
    cases = []
    for case, categories in nodes:
        name = case["fullname"]
        engine_id = case["id"]
        unique_name = name if actual[name] == 1 else f"{name} [NUnit {engine_id}]"
        if name in mapping and actual[name] > 1:
            raise ValueError(f"Ambiguous mapped fixture case, use discovery ID suffix: {name}")
        row = mapping.get(unique_name)
        runstate = case["runstate"]
        if not exclusions.isdisjoint(categories):
            status = "excluded"
        elif runtime_host_excluded(case["classname"], case["methodname"], platform.system() == "Windows"):
            status = "excluded"
        elif runstate != "Runnable":
            status = {"Ignored": "ignored", "Skipped": "skipped",
                      "Explicit": "explicit"}.get(runstate, "not_runnable")
        else:
            status = "not_run" if row else "unmapped"
        cases.append(dict(project=project, official_fullname=name,
                          id=f"{project}::{unique_name}", nunit_id=engine_id,
                          fixture=case["classname"], runstate=runstate,
                          categories=sorted(categories), status=status,
                          rust_test=row["rust_test"] if row else "",
                          origin=row["origin"] if row else "",
                          parameters=case_parameters(case["methodname"], case["name"], name)))
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
    projects = manifest_projects(renode)
    submodules = {Path(*Path(project).parts[:2]) for project in projects
                  if Path(project).parts[:2] != ("tests", "unit-tests")}
    revisions = {}
    for path in sorted(submodules):
        revision = git(clone / path, "rev-parse", "HEAD")
        if revision != git(renode, "rev-parse", f"HEAD:{path}"):
            parser.error(f"{path} checkout does not match the pinned Renode gitlink")
        revisions[str(path)] = revision
    if git(ROOT, "status", "--porcelain", "-uno"):
        parser.error("commit tracked changes before collecting evidence")

    dotnet_path = shutil.which(args.dotnet)
    if not dotnet_path:
        parser.error(f".NET executable not found: {args.dotnet}")
    dotnet = str(Path(dotnet_path).resolve())
    mapping = load_mapping()
    out.mkdir(parents=True)
    cases, discovery_runs = [], []
    for index, project in enumerate(projects, 1):
        directory = out / f"nunit-{index}"
        directory.mkdir()
        rows, discovery = discover(project, {name: row for (p, name), row in mapping.items()
                                              if p == project}, clone, dotnet, directory)
        cases.extend(rows)
        discovery_runs.append(discovery)

    if len({case["id"] for case in cases}) != len(cases):
        raise ValueError("Discovery contains duplicate case IDs")
    mapped = [case for case in cases if case["rust_test"]]
    if {case["rust_test"] for case in mapped} != {row["rust_test"] for row in mapping.values()}:
        raise ValueError("Some mapped Rust tests have no official discovery ID")
    tests = sorted(case["rust_test"] for case in mapped if case["status"] == "not_run")
    if not tests:
        raise ValueError("No runnable mappings for this host")
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
    notes = fixture_notes()
    fixtures = defaultdict(Counter)
    for case in cases:
        if case["status"] == "not_run":
            case["status"] = {"ok": "pass", "FAILED": "fail"}.get(
                outcomes.get(case["rust_test"]), "not_run"
            )
        elif case["status"] == "unmapped" and case["fixture"] in notes:
            case["status"] = "api_missing"
        fixtures[(case["project"], case["fixture"])][case["status"]] += 1

    totals = Counter(case["status"] for case in cases)
    inactive = ("excluded", "ignored", "skipped", "explicit", "not_runnable")
    summary = dict(ported=len(mapped), newly_ported=sum(case["origin"] == "new" for case in mapped),
                   passed=totals["pass"], failed=totals["fail"], not_run=totals["not_run"],
                   discovered=len(cases), runnable=len(cases) - sum(totals[state] for state in inactive),
                   api_missing=totals["api_missing"], unmapped=totals["unmapped"], selected=len(tests),
                   mapped_excluded=sum(case["status"] in inactive for case in mapped),
                   **{state: totals[state] for state in inactive})
    report = dict(renode_sha=PIN, submodule_shas=revisions,
                  penv_sha=git(ROOT, "rev-parse", "HEAD"), discovery=discovery_runs, summary=summary,
                  rust=dict(command=command, exit_code=status, log=log.name, binary_sha256=binary_sha),
                  cases=mapped)
    (out / "results.json").write_text(json.dumps(report, indent=2) + "\n")

    fields = ("id", "nunit_id", "fixture", "runstate", "categories", "parameters", "status", "rust_test")
    with (out / "inventory.jsonl").open("w") as inventory:
        for case in cases:
            inventory.write(json.dumps({field: case[field] for field in fields}, ensure_ascii=False) + "\n")
    states = ("pass", "fail", "not_run", "api_missing", "unmapped", "excluded",
              "ignored", "skipped", "explicit", "not_runnable")
    with (out / "fixtures.tsv").open("w", newline="") as summary_file:
        writer = csv.writer(summary_file, delimiter="\t", lineterminator="\n")
        writer.writerow(("project", "fixture", "discovered", *states, "missing_api"))
        for (project, fixture), counts in sorted(fixtures.items()):
            writer.writerow((project, fixture, sum(counts.values()), *(counts[state] for state in states),
                             notes.get(fixture, "-") if counts["api_missing"] else "-"))

    print(json.dumps(summary), out)
    return int(status != 0 or totals["pass"] != len(tests))


if __name__ == "__main__":
    sys.exit(main())
