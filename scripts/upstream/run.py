#!/usr/bin/env python3
"""Inventory Renode's public manifest and run supported cases against penv."""

import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
from threading import Thread
from types import SimpleNamespace
import time
import xml.etree.ElementTree as ET

sys.dont_write_bytecode = True

from nunit_mapping import case_parameters, load_mapping, runtime_host_excluded
from penv_robot import Endpoint


PIN = "c38ac3babcd38a7144b9498e3ca4c2226b76aa37"
ROOT = Path(__file__).resolve().parents[2]
DEFAULT_ROBOT = ["tests/unit-tests/hex.robot", "tests/platforms/STM32F072b.robot"]
MAPPING = load_mapping()


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args], text=True).strip()


def check_upstream(root, submodules=False):
    if git(root, "rev-parse", "HEAD") != PIN or git(root, "status", "--porcelain", "-uno"):
        raise ValueError(f"{root} must contain the clean tracked Renode pin {PIN}")
    if submodules and any(row[0] != " " for row in subprocess.check_output(
            ["git", "-C", str(root), "submodule", "status", "--recursive"],
            text=True).splitlines()):
        raise ValueError(f"initialize pinned submodules in {root}")


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")


def log_command(directory, name, cmd, cwd, timeout=600):
    logfile = directory / (name + ".log")
    info = dict(command=[str(arg) for arg in cmd], cwd=str(cwd), log=str(logfile))
    with logfile.open("wb") as output:
        try:
            proc = subprocess.run(info["command"], cwd=cwd, stdout=output,
                                  stderr=subprocess.STDOUT, timeout=timeout)
            info["exit_code"] = proc.returncode
        except (OSError, subprocess.TimeoutExpired) as exc:
            info.update(exit_code=None, error=str(exc))
    write_json(directory / (name + ".json"), info)
    return info


def inventory(renode, host):
    sys.path.insert(0, str(renode / "tests"))
    from robot_tests_provider import RobotTestSuite
    from tests_engine import parse_tests_file

    options = SimpleNamespace(include=None, exclude=host["exclude"], skip=host["skip"],
                              fixture=None, hotspot=None)
    rows, projects, providers = [], [], {}
    for entry in parse_tests_file(renode / "tests/tests.yaml"):
        group, paths = next(iter(entry.items())) if isinstance(entry, dict) else (None, [entry])
        for path in paths:
            if path.endswith(".csproj"):
                projects.append(dict(project=path, group=group))
            if not path.endswith((".robot", ".robot.rst")):
                continue
            provider = RobotTestSuite(str(renode / path))
            providers[path] = provider
            if not provider.select_tests(options):
                continue
            source_sha = hashlib.sha256((renode / path).read_bytes()).hexdigest()
            variants = provider.selected_tests
            for hotspot, names in variants:
                suite = provider._get_test_suite(options, names, None, hotspot)
                for test in suite.tests:
                    rows.append(dict(id=f"{path}::{test.name}", path=path, name=test.name,
                                     parameters={"hotspot_action": hotspot}, group=group,
                                     source_sha256=source_sha, source_line=test.lineno,
                                     tags=list(test.tags), skipped=any(tag in test.tags for tag in host["skip"])))
    return rows, projects, providers, options


def run_robot(case, provider, options, renode, binary, out, assets, negative):
    from state_dependency_listener import keyword_states
    from tests_engine import TestTag

    hotspot = case["parameters"]["hotspot_action"]
    # Use the provider's resource and default suite lifecycle. Restrict to the exact
    # case after upstream selection, since Robot's name filters accept wildcards.
    names = next(names for action, names in provider.selected_tests if action == hotspot)
    suite = provider._get_test_suite(options, names, None, hotspot)
    suite.tests = [test for test in suite.tests if test.name == case["name"]]
    test, = suite.tests
    if case["skipped"]:
        return dict(status="skipped")
    if (hotspot is not None or keyword_states(test, "Requires")
            or keyword_states(test, "Provides") or "profiling" in test.tags
            or provider.timeout_expected_tag in test.tags):
        return dict(status="unsupported", reason="requires Renode-specific lifecycle")
    if not test.setup:
        test.setup.config(name="Renode Test Setup")
    if not test.teardown:
        test.teardown.config(name="Renode Test Teardown")

    endpoint = Endpoint(renode, binary, out, assets, negative)
    with endpoint.server() as server:
        worker = Thread(target=server.serve_forever, daemon=True)
        worker.start()
        try:
            result = suite.run(output=str(out / "robot.xml"), log=None, report=None,
                               console="none", loglevel="TRACE",
                               skiponfailure=[TestTag.NON_CRITICAL, TestTag.SKIPPED,
                                              TestTag.UNSTABLE],
                               variable=["SKIP_RUNNING_SERVER:True", "SAVE_LOGS:False",
                                         "CREATE_SNAPSHOT_ON_FAIL:False",
                                         f"PORT_NUMBER:{server.server_address[1]}",
                                         f"RESULTS_DIRECTORY:{out}",
                                         f"RENODEKEYWORDS:{renode / 'tests/renode-keywords.robot'}"])
        finally:
            server.shutdown()
            worker.join()
    executed, = result.suite.tests
    last = endpoint.measurements[-1]["result"] if endpoint.measurements else None
    xml = out / "robot.xml"
    failures = ET.parse(xml).getroot().iter("msg")
    missing_keyword = any(msg.get("level") == "FAIL" and re.search(
        r"No keyword with name .* found", msg.text or "") for msg in failures)
    if missing_keyword or any(call["status"] == "unsupported" for call in endpoint.calls):
        status = "unsupported"
    elif executed.status == "SKIP":
        status = "skipped"
    elif last == "pass" and result.return_code == 0 and executed.status == "PASS":
        status = "pass"
    elif last == "fail" and executed.status == "FAIL":
        status = "fail"
    elif last and last != "pass":
        status = last
    elif any(call["status"] == "error" for call in endpoint.calls):
        status = "startup_error"
    else:
        status = "harness_error"
    return dict(status="diagnostic" if negative else status, outcome=status,
                robot_status=executed.status, message=executed.message,
                remote_calls=endpoint.calls, measurements=endpoint.measurements,
                evidence=str(xml))


def nunit_nodes(node, inherited=frozenset()):
    categories = inherited.union(
        prop.attrib["value"] for prop in node.findall("properties/property[@name='Category']")
    )
    if node.tag == "test-case":
        yield node, categories
    else:
        for child in node:
            if child.tag in {"NUnitXml", "test-run", "test-suite", "test-case"}:
                yield from nunit_nodes(child, categories)


def discover_nunit(root, dotnet, projects, host, output):
    """Use the pinned official projects and VSTest adapter for concrete IDs."""
    output.mkdir()
    rows, runs = [], []
    for i, item in enumerate(projects):
        project = item["project"]
        print(f"NUnit discovery {i + 1}/{len(projects)} {project}", flush=True)
        directory = output / str(i)
        directory.mkdir()
        build = log_command(directory, "build",
                            [dotnet, "build", "--verbosity", "quiet", "--configuration", "Release",
                             "/p:NET=true", f"/p:Architecture={host['build_arch']}", project], root)
        run = dict(**item, build=build, status="discovery_blocked")
        runs.append(run)
        if build["exit_code"] != 0:
            continue
        started_ns = time.time_ns()
        discover = log_command(directory, "discover",
                               [dotnet, "test", "--no-build", "--configuration", "Release",
                                "--list-tests", project, "--", "NUnit.DisplayName=FullName",
                                "NUnit.DumpXmlTestDiscovery=true"], root)
        run["discovery"] = discover
        if discover["exit_code"] != 0:
            continue
        log = Path(discover["log"]).read_text(errors="replace")
        # VSTest ignores --filter in list mode. Use NUnit's discovery tree for
        # inherited categories and runnable state, and corroborate its IDs with
        # the adapter's VSTest listing.
        assembly = re.search(r"^Test run for (.+?\.dll) \(", log, re.MULTILINE)
        _, marker, listing = log.partition("The following Tests are available:")
        if not assembly or not marker:
            continue
        assembly_path = Path(assembly[1]).resolve()
        if not assembly_path.is_relative_to(root):
            continue
        dump = assembly_path.parent / "Dump" / f"D_{assembly_path.name}.dump"
        if not dump.is_file() or dump.stat().st_mtime_ns < started_ns:
            continue
        xml = Path(shutil.copy2(dump, directory / "discovery.xml"))
        try:
            nodes = list(nunit_nodes(ET.parse(xml).getroot()))
        except ET.ParseError as exc:
            run["error"] = str(exc)
            continue
        ids = Counter(line.strip() for line in listing.splitlines() if line.startswith("    "))
        tree_ids = Counter(node.attrib["fullname"] for node, _ in nodes)
        if not ids or ids != tree_ids:
            run.update(error="VSTest and NUnit discovery records differ",
                       listed=sum(ids.values()), xml_cases=len(nodes))
            continue
        run.update(status="discovered", count=len(nodes), evidence=str(xml),
                   selected_count=0, runnable_count=0)
        for node, categories in nodes:
            case_id = node.attrib["fullname"]
            method = node.attrib["methodname"]
            display_name = node.attrib["name"]
            engine_id = node.attrib["id"]
            # NUnit permits identical custom names (even for different arguments).
            # Retain the engine ID for those occurrences in this pinned discovery.
            unique_id = case_id if ids[case_id] == 1 else f"{case_id} [NUnit {engine_id}]"
            mapped = MAPPING.get((project, case_id))
            excluded = (not categories.isdisjoint(host["exclude"])
                        or runtime_host_excluded(node.attrib["classname"], method,
                                                 host["os"] == "windows"))
            runstate = node.attrib["runstate"]
            if excluded:
                status = "excluded"
            elif runstate != "Runnable":
                status = "skipped" if runstate in {"Ignored", "Skipped", "Explicit"} else "not_runnable"
            else:
                status = "not_run" if mapped else "unmapped"
            run["selected_count"] += not excluded
            run["runnable_count"] += (not excluded and runstate == "Runnable")
            rows.append(dict(id=f"{project}::{unique_id}", project=project, name=case_id,
                             nunit_id=engine_id, parameters=case_parameters(method, display_name),
                             categories=sorted(categories), runstate=runstate,
                             rust_test=mapped["rust_test"] if mapped else None,
                             evidence=str(xml), status=status))
    return rows, runs


def run_rust(row, output, target):
    name = row["rust_test"]
    info = log_command(output, name.split("::")[-1],
                       ["cargo", "test", "--locked", "--release", "--color", "never", "--bin", "penv",
                        "--target-dir", target, name, "--", "--exact"], ROOT)
    log = Path(info["log"]).read_text(errors="replace")
    match = re.search(rf"^test {re.escape(name)} \.\.\. (ok|FAILED)$", log, re.MULTILINE)
    executable = re.search(r"^\s*Running unittests .* \((.+)\)$", log, re.MULTILINE)
    if executable:
        path = ROOT / executable[1]
        if path.is_file():
            info.update(rust_binary=str(path), rust_binary_sha256=hashlib.sha256(path.read_bytes()).hexdigest())
    if match and "running 1 test" in log and "rust_binary_sha256" in info:
        status = "pass" if match[1] == "ok" and info["exit_code"] == 0 else "fail"
    else:
        status = "not_run"
    return dict(row, status=status, measurement=info)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renode", type=Path, default=ROOT.parent / "renode")
    parser.add_argument("--out", required=True, type=Path, help="new evidence directory")
    parser.add_argument("--inventory-only", action="store_true")
    parser.add_argument("--all", action="store_true", help="queue all host-selected Robot suites")
    parser.add_argument("--jobs", type=int, choices=range(1, 11), default=10)
    parser.add_argument("--suite", action="append", help="manifest-relative Robot suite path")
    parser.add_argument("--case", help="exact Robot test name")
    parser.add_argument("--negative-control", action="store_true", help="additional short-deadline HEX pilot")
    parser.add_argument("--nunit-root", type=Path, help="writable pinned clone with upstream build prerequisites")
    parser.add_argument("--dotnet", default="dotnet")
    args = parser.parse_args()
    renode, out = args.renode.resolve(), args.out.resolve()
    if not out.is_relative_to(ROOT) or out.is_relative_to(renode):
        parser.error("evidence must be inside this worktree")
    check_upstream(renode)
    penv_sha = git(ROOT, "rev-parse", "HEAD")
    if git(ROOT, "status", "--porcelain", "-uno"):
        parser.error("commit tracked changes before producing pinned evidence")
    os.environ["PYTHONDONTWRITEBYTECODE"] = "1"
    os.environ["DOTNET_CLI_UI_LANGUAGE"] = "en-US"
    system = {"Linux": "linux", "Darwin": "osx"}.get(platform.system(), "windows")
    arch = "aarch64" if platform.machine() in {"aarch64", "arm64"} else "i386"
    host = dict(os=system, arch=arch, build_arch="arm" if arch == "aarch64" else "i386",
                exclude=["excluded", f"exclude_{system}", f"exclude_host_{arch}"],
                skip=["skipped", f"skip_{system}"])
    rows, projects, providers, options = inventory(renode, host)
    if args.all and (args.suite or args.case):
        parser.error("--all cannot be combined with --suite or --case")
    selected = [row for row in rows if (args.all or row["path"] in (args.suite or DEFAULT_ROBOT))
                and (args.case is None or row["name"] == args.case)]
    if not args.inventory_only and not selected:
        parser.error("no host-selected Robot cases match")
    nunit_root = args.nunit_root.resolve() if args.nunit_root else None
    if nunit_root:
        if nunit_root == ROOT or not nunit_root.is_relative_to(ROOT) or nunit_root.is_relative_to(renode):
            parser.error("NUnit discovery writes only to a separate clone inside this worktree")
        check_upstream(nunit_root, submodules=True)
    out.mkdir(parents=True)
    metadata = dict(upstream_sha=PIN, penv_sha=penv_sha, host=host,
                    invocation=[sys.executable, *sys.argv], cwd=os.getcwd(),
                    renode=str(renode), robot_suites=len(providers), robot_instances=len(rows),
                    robot_host_skipped=sum(row["skipped"] for row in rows),
                    nunit_projects=len(projects))
    write_json(out / "inventory.json", dict(metadata=metadata, robot=rows, nunit=projects))
    results, nunit_runs, nunit = [], [], []
    if not args.inventory_only:
        metadata["rustc"] = log_command(out, "rustc-version", ["rustc", "-Vv"], ROOT)
        metadata["cargo"] = log_command(out, "cargo-version", ["cargo", "-V"], ROOT)
        build = log_command(out, "penv-build", ["cargo", "build", "--locked", "--release",
                                               "--bin", "penv", "--target-dir", out / "cargo"], ROOT)
        metadata["build"] = build
        if build["exit_code"] != 0:
            results.extend(dict(case, engine="penv", status="not_run", category="environment",
                                reason="penv build failed", evidence=build["log"])
                           for case in selected)
        else:
            binary = Path(shutil.copy2(out / "cargo/release/penv", out / "penv"))
            metadata["penv_sha256"] = hashlib.sha256(binary.read_bytes()).hexdigest()
            assets = out / "assets"
            assets.mkdir()
            from batch import run_batch
            results.extend(run_batch(selected, options, renode, binary, out, assets, args.jobs))
            if args.negative_control:
                pilot, = [row for row in rows if row["path"] == DEFAULT_ROBOT[0]
                          and row["name"] == "Should Load HEX"]
                directory = out / "negative-control"
                directory.mkdir()
                result = run_robot(pilot, providers[pilot["path"]], options, renode, binary,
                                   directory, assets, True)
                record = dict(pilot, engine="penv", **result)
                write_json(directory / "case.json", record)
                results.append(record)
    if nunit_root:
        metadata["dotnet"] = log_command(out, "dotnet-version", [args.dotnet, "--version"], nunit_root)
        nunit, nunit_runs = discover_nunit(nunit_root, args.dotnet, projects, host, out / "nunit")
        for row in nunit:
            if row["status"] == "not_run" and row["rust_test"] and not args.inventory_only:
                if "penv_sha256" in metadata:
                    print(f"NUnit Rust {row['id']}", flush=True)
                    row = run_rust(row, out / "nunit", out / "cargo")
                else:
                    row.update(reason="penv build failed", evidence=metadata["build"]["log"])
            row["engine"] = "penv-rust"
            row["category"] = {"pass": "passed", "fail": "assertion_failed",
                               "skipped": "skip", "excluded": "skip",
                               "not_runnable": "environment",
                               "not_run": "environment"}.get(row["status"], "adapter_missing")
            if row["status"] == "unmapped":
                row.update(status="unsupported", reason="no equivalent Rust test")
            results.append(row)
    else:
        nunit_runs = [dict(item, status="discovery_not_run") for item in projects]
    check_upstream(renode)
    if git(ROOT, "rev-parse", "HEAD") != penv_sha or git(ROOT, "status", "--porcelain", "-uno"):
        raise ValueError("penv sources changed during the run")
    completed_keys = {(row["id"], row["parameters"]["hotspot_action"])
                      for row in results if row.get("path") and row["status"] != "diagnostic"}
    results.extend(dict(row, engine="penv", status="not_run", reason="outside execution selection")
                   for row in rows if (row["id"], row["parameters"]["hotspot_action"])
                   not in completed_keys)
    counts = dict(Counter(row["status"] for row in results))
    coverage = {"pass": "PASS", "fail": "FAIL", "unsupported": "UNSUPPORTED",
                "skipped": "SKIP", "excluded": "SKIP"}
    for row in results:
        row["verdict"] = coverage.get(row["status"], "NOT_RUN") if row["status"] != "diagnostic" else None
    metadata["upstream_execution"] = "discovery only; no upstream results used as penv verdicts"
    metadata["nunit_resources_sha"] = (
        git(nunit_root / "lib/resources", "rev-parse", "HEAD")
        if nunit_root and (nunit_root / "lib/resources/.git").exists() else None
    )
    metadata["measured_cases"] = counts.get("pass", 0) + counts.get("fail", 0)
    write_json(out / "results.json", dict(metadata=metadata, counts=counts,
                                         verdicts=dict(Counter(row["verdict"] for row in results
                                                               if row["verdict"])),
                                         categories=dict(Counter(row.get("category", "not_selected")
                                                                 for row in results)),
                                         nunit=nunit_runs, cases=results))
    print(counts)
    accepted = {"pass", "skipped", "unmapped", "diagnostic", "excluded", "not_runnable"}
    if args.inventory_only:
        accepted.add("not_run")
    return int(any(row["status"] not in accepted and row.get("reason") != "outside execution selection"
                   and not (row.get("engine") == "penv-rust" and row["status"] == "unsupported")
                   for row in results)
               or any(row.get("status") == "diagnostic" and row["outcome"] != "fail"
                      for row in results)
               or bool(nunit_root and any(run["status"] != "discovered" for run in nunit_runs))
               or bool(nunit_root and {(row["project"], row["name"]) for row in nunit
                                       if row["rust_test"]} != set(MAPPING)))


if __name__ == "__main__":
    sys.exit(main())
