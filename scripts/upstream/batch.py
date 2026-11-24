"""One Robot invocation per suite variant, isolated in a suite worker."""
from collections import Counter
import json
from functools import partial
import os
from pathlib import Path
import re
from threading import Thread
import xml.etree.ElementTree as ET

from live import LiveEndpoint

# These suites launch host services or invoke the upstream runner recursively.
HOST_SUITES = {"tests/peripherals/VirtIO.robot", "tests/tools/renode-test.robot",
               "tests/network-logging/network-logging.robot"}


def status_record(case, status, reason):
    return dict(case, status=status, reason=reason, engine="penv")


def write_case(case, record, directory):
    from run import write_json
    write_json(directory / "case.json", dict(case, engine="penv", **record))


def assess(test, endpoint, missing):
    last = endpoint.measurements[-1]["result"] if endpoint.measurements else None
    unsupported = next((call["message"] for call in endpoint.calls
                        if call["status"] == "unsupported"), None)
    if missing or unsupported:
        status, reason = "unsupported", missing or unsupported
    elif test.status == "SKIP":
        status, reason = "skipped", test.message
    elif last == "pass" and test.status == "PASS" and all(
            call["status"] == "ok" for call in endpoint.calls):
        status, reason = "pass", "penv assertion and Robot case passed"
    elif test.status == "FAIL" and last in {"pass", "fail"}:
        status, reason = "fail", test.message
    elif last or any(call["status"] == "error" for call in endpoint.calls):
        status, reason = "not_run", test.message or last or "remote startup error"
    else:
        status, reason = "not_run", test.message or "no measured penv operation"
    return dict(status=status, reason=reason, robot_status=test.status,
                message=test.message, remote_calls=endpoint.calls[:],
                measurements=endpoint.measurements[:])


class SuiteListener:
    ROBOT_LISTENER_API_VERSION = 3

    def __init__(self, endpoint, active, out, xml):
        self.endpoint, self.active, self.out, self.xml = endpoint, active, out, xml
        self.results = {}

    def start_test(self, data, result):
        case, directory = self.active[data.name]
        self.endpoint.begin_case(directory)

    def end_test(self, data, result):
        case, directory = self.active[data.name]
        record = assess(result, self.endpoint, None)
        record["evidence"] = str(self.xml)
        self.results[data.name] = record
        write_case(case, record, directory)


def run_suite(task, *, options, renode, binary, assets):
    """Pool worker entry point. Each worker uses its own Robot process state."""
    from robot_tests_provider import RobotTestSuite
    from state_dependency_listener import keyword_states
    from tests_engine import TestTag

    path, hotspot, cases, out = task
    out.mkdir()
    old_cwd = Path.cwd()
    os.chdir(out)
    try:
        provider = RobotTestSuite(str(renode / path))
        suite = provider._get_test_suite(options, [c["name"] for c in cases], None, hotspot)
        by_name = {c["name"]: c for c in cases}
        records, active = {}, {}
        for index, test in enumerate(suite.tests):
            case = by_name.get(test.name)
            if case is None:
                continue
            directory = out / f"case-{index:04d}"
            directory.mkdir()
            reason = None
            if case["skipped"]:
                records[test.name] = dict(status="skipped", reason="upstream host skip tag")
            elif (path in HOST_SUITES or path.startswith(("tests/external-control/",
                    "tests/native-interface/", "tests/unit-tests/gdb/"))):
                reason = "requires host process, debugger or native integration"
            elif (hotspot is not None or keyword_states(test, "Requires")
                    or keyword_states(test, "Provides") or "profiling" in test.tags
                    or provider.timeout_expected_tag in test.tags):
                reason = "requires Renode-specific lifecycle"
            if reason:
                records[test.name] = dict(status="unsupported", reason=reason)
            if test.name in records:
                write_case(case, records[test.name], directory)
                continue
            active[test.name] = case, directory
            if not test.setup:
                test.setup.config(name="Renode Test Setup")
            if not test.teardown:
                test.teardown.config(name="Renode Test Teardown")
            # Preserve shorter upstream timeouts and bound unbounded host waits.
            from robot.utils import timestr_to_secs
            if not test.timeout or timestr_to_secs(str(test.timeout)) > 130:
                test.timeout = "130 seconds"
        suite.tests = [test for test in suite.tests if test.name in active]
        if active:
            xml = out / "robot.xml"
            endpoint = LiveEndpoint(renode, binary, out, assets, False)
            listener = SuiteListener(endpoint, active, out, xml)
            with endpoint.server() as server:
                worker = Thread(target=server.serve_forever, daemon=True)
                worker.start()
                try:
                    suite.run(output=str(xml), log=None, report=None, console="none",
                              listener=listener, loglevel="DEBUG",
                              skiponfailure=[TestTag.NON_CRITICAL, TestTag.SKIPPED,
                                             TestTag.UNSTABLE],
                              variable=["SKIP_RUNNING_SERVER:True", "SAVE_LOGS:False",
                                        "CREATE_SNAPSHOT_ON_FAIL:False",
                                        f"PORT_NUMBER:{server.server_address[1]}",
                                        f"RESULTS_DIRECTORY:{out}", f"RENODETOOLS:{renode / 'tools'}",
                                        f"RENODEKEYWORDS:{renode / 'tests/renode-keywords.robot'}"])
                finally:
                    endpoint.close()
                    server.shutdown()
                    worker.join()
            records.update(listener.results)
            for node in ET.parse(xml).getroot().findall("./suite/test"):
                name = node.get("name")
                missing = next((msg.text for msg in node.iter("msg") if msg.get("level") == "FAIL"
                                and re.search(r"No keyword with name .* found", msg.text or "")), None)
                if missing:
                    records[name].update(status="unsupported", reason=missing)
                if records[name]["status"] in {"pass", "fail"} and any(
                        keyword.get("name") == "Sleep" and keyword.get("library") == "BuiltIn"
                        for keyword in node.iter("kw")):
                    records[name].update(status="unsupported", reason="requires asynchronous emulation during host Sleep")
                if node.find("status").get("status") == "FAIL" and records[name]["status"] == "pass":
                    records[name].update(status="fail", reason="Robot XML reports failure")
                case, directory = active[name]
                write_case(case, records[name], directory)
        return [dict(case, engine="penv", **records.get(case["name"], dict(
            status="not_run", reason="case did not reach Robot listener"))) for case in cases]
    finally:
        os.chdir(old_cwd)


def run_batch(selected, options, renode, binary, out, assets, jobs):
    from concurrent.futures import ProcessPoolExecutor, as_completed
    from run import write_json

    groups = {}
    for case in selected:
        key = case["path"], case["parameters"]["hotspot_action"]
        groups.setdefault(key, []).append(case)
    tasks = [(*key, cases, out / f"suite-{index:04d}")
             for index, (key, cases) in enumerate(groups.items())]
    all_results = {}
    counts = Counter()
    print(f"Robot queued {len(selected)} cases in {len(tasks)} suite variants, jobs={jobs}", flush=True)
    runner = partial(run_suite, options=options, renode=renode, binary=binary, assets=assets)
    # One worker per suite avoids sharing Robot's global BuiltIn execution context.
    with ProcessPoolExecutor(max_workers=jobs) as pool, (out / "progress.jsonl").open("w") as progress:
        futures = {pool.submit(runner, task): task for task in tasks}
        for index, future in enumerate(as_completed(futures), 1):
            path, hotspot, cases, directory = futures[future]
            try:
                records = future.result()
            except Exception as exc:
                records = [status_record(case, "not_run", f"suite worker error: {exc!r}")
                           for case in cases]
                # A late exception can leave valid per-case results.
                saved = {(r["id"], r["parameters"]["hotspot_action"]): r
                         for p in directory.glob("case-*/case.json")
                         for r in [json.loads(p.read_text())]}
                records = [saved.get((r["id"], hotspot), r) for r in records]
            all_results[path, hotspot] = records
            counts.update(record["status"] for record in records)
            snapshot = dict(completed=index, total=len(tasks), path=path, hotspot=hotspot,
                            counts=dict(counts))
            print(json.dumps(snapshot), file=progress, flush=True)
            print(f"Robot {index}/{len(tasks)} {path} {hotspot} {dict(counts)}", flush=True)
            write_json(directory.with_suffix(".json"), records)
    return [record for key in groups for record in all_results[key]]
