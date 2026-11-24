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
EXTERNAL_INPUTS = {
    "tests/unit-tests/riscv-isa-tests.robot": ("RISCV_TESTS_PATH", "/riscv-tests/isa"),
    "tests/unit-tests/riscv-arch-tests.robot": ("RISCV_ARCH_TESTS_PATH", "/riscv-arch-test/work"),
}


def status_record(case, status, reason):
    return dict(case, status=status, reason=reason, engine="penv", category="environment")


def write_case(case, record, directory):
    from run import write_json
    write_json(directory / "case.json", dict(case, engine="penv", **record))


def assess(test, endpoint):
    # Robot can time out while an XML-RPC keyword is still running.
    failures = [call for call in endpoint.calls if call.get("status") not in (None, "ok")]
    if test.status == "PASS":
        # A typed rejection came from an actual monitor lookup. Robot's
        # successful error assertion supplies the expected-outcome check.
        failures = [call for call in failures if call.get("monitor", {}).get("reply") != "REJECTED"]
    in_flight = any("status" not in call for call in endpoint.calls)
    issue = next((call for call in failures if call["status"] == "unsupported"), None)
    if issue:
        status, category, reason = "unsupported", issue["category"], issue["message"]
    elif test.status == "SKIP":
        status, category, reason = "skipped", "skip", test.message
    elif failures:
        issue = failures[-1]
        category, reason = issue["category"], issue["message"]
        status = ("fail" if category in {"assertion_failed", "penv_execution_failure"}
                  and issue.get("stage") != "launch" else "not_run")
        if test.status == "PASS":
            status, category = "not_run", "environment"
            reason = f"error handled by Robot without a verified penv assertion: {reason}"
    elif in_flight:
        status, category = "not_run", "environment"
        reason = test.message or "Robot ended while penv monitor operation was still pending"
    elif endpoint.measurements and test.status == "PASS":
        status, category, reason = "pass", "passed", "penv observation and Robot case passed"
    elif endpoint.measurements and test.status == "FAIL":
        status, category, reason = "fail", "assertion_failed", test.message
    else:
        status, category = "not_run", "no_measurement"
        reason = test.message or "no measured penv operation"
    return dict(status=status, category=category, reason=reason, robot_status=test.status,
                message=test.message, remote_calls=endpoint.calls[:],
                measurements=endpoint.measurements[:])


class SuiteListener:
    ROBOT_LISTENER_API_VERSION = 3

    def __init__(self, endpoint, active, out, xml):
        self.endpoint, self.active, self.out, self.xml = endpoint, active, out, xml
        self.results = {}
        self.endpoint.states = {}
        self.blocked = {}

    def start_test(self, data, result):
        from state_dependency_listener import keyword_states

        _, directory = self.active[data.name]
        self.endpoint.begin_case(directory)
        missing = [state for state in keyword_states(data, "Requires")
                   if state not in self.endpoint.states]
        if missing:
            reason = f"required state not produced by a successful replayable provider: {missing}"
            self.blocked[data.name] = reason
            data.config(setup=None, teardown=None)
            data.body.clear()
            data.body.create_keyword("Skip", [reason])

    def end_test(self, data, result):
        case, directory = self.active[data.name]
        record = assess(result, self.endpoint)
        if data.name in self.blocked:
            record.update(status="skipped", category="dependency_blocked", reason=self.blocked[data.name])
        if result.status == "PASS" and self.endpoint.pending_state and all(
                call["status"] == "ok" for call in self.endpoint.calls):
            self.endpoint.publish_state()
        record["evidence"] = str(self.xml)
        self.results[data.name] = record
        write_case(case, dict(record, provisional=True), directory)


class VirtualSleepListener:
    ROBOT_LISTENER_API_VERSION = 2

    def __init__(self, endpoint):
        self.endpoint = endpoint
        self.mirrored = {}

    def start_keyword(self, name, attrs):
        if name != "BuiltIn.Sleep" or not self.endpoint.started or not self.endpoint.proc:
            return
        from robot.libraries.BuiltIn import BuiltIn
        from robot.utils import timestr_to_secs
        builtin = BuiltIn()
        try:
            seconds = timestr_to_secs(builtin.replace_variables(attrs["args"][0]))
            seconds = self.endpoint.duration(str(seconds))
        except (ValueError, TypeError, IndexError):
            return
        result = self.endpoint.run_keyword("Execute Command", [f"emulation RunFor {seconds}"])
        if result["status"] == "PASS":
            test = builtin.get_variable_value("${TEST NAME}")
            self.mirrored[test] = self.mirrored.get(test, 0) + 1


def run_suite(task, *, options, renode, binary, assets, host_case_timeout, rust_monitor):
    """Pool worker entry point. Each worker uses its own Robot process state."""
    from robot_tests_provider import RobotTestSuite
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
                records[test.name] = dict(status="skipped", category="skip", reason="upstream host skip tag")
            elif (path in HOST_SUITES or path.startswith(("tests/external-control/",
                    "tests/native-interface/", "tests/unit-tests/gdb/"))):
                reason = "requires host process, debugger or native integration"
            elif ("profiling" in test.tags or provider.timeout_expected_tag in test.tags):
                reason = "requires Renode-specific lifecycle"
            if reason:
                records[test.name] = dict(status="unsupported", category="adapter_missing", reason=reason)
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
            if not test.timeout or timestr_to_secs(str(test.timeout)) > host_case_timeout:
                test.timeout = f"{host_case_timeout} seconds"
        suite.tests = [test for test in suite.tests if test.name in active]
        if active:
            xml = out / "robot.xml"
            endpoint = LiveEndpoint(renode, binary, out, assets, False)
            endpoint.rust_monitor = rust_monitor
            listener = SuiteListener(endpoint, active, out, xml)
            # This suite sleeps while a GPIO button is held. Advance its single
            # Cortex-M machine during the same keyword before releasing the button.
            virtual_sleep = VirtualSleepListener(endpoint) if path == "tests/platforms/STM32F072b.robot" else None
            listeners = [listener, virtual_sleep] if virtual_sleep else [listener]
            with endpoint.server() as server:
                worker = Thread(target=server.serve_forever, daemon=True)
                worker.start()
                try:
                    suite.run(output=str(xml), log=None, report=None, console="none",
                              listener=listeners, loglevel="DEBUG",
                              skiponfailure=[TestTag.NON_CRITICAL, TestTag.SKIPPED,
                                             TestTag.UNSTABLE],
                              variable=["SKIP_RUNNING_SERVER:True", "SAVE_LOGS:False",
                                        "CREATE_SNAPSHOT_ON_FAIL:False",
                                        f"PORT_NUMBER:{server.server_address[1]}",
                                        f"HOTSPOT_ACTION:{hotspot or 'None'}",
                                        f"RESULTS_DIRECTORY:{out}", f"RENODETOOLS:{renode / 'tools'}",
                                        f"RENODEKEYWORDS:{renode / 'tests/renode-keywords.robot'}"])
                finally:
                    endpoint.close()
                    server.shutdown()
                    worker.join()
            records.update(listener.results)
            for node in ET.parse(xml).getroot().findall("./suite/test"):
                name = node.get("name")
                case, directory = active[name]
                if name not in records:
                    status = node.find("status")
                    monitor = directory / "monitor.jsonl"
                    records[name] = dict(
                        status="not_run", category="environment",
                        reason=status.text or "Robot listener did not finish collecting the case",
                        robot_status=status.get("status"), evidence=str(xml),
                        measurements=[json.loads(line) for line in monitor.read_text().splitlines()]
                        if monitor.is_file() else [],
                    )
                missing = next((msg.text for msg in node.iter("msg") if msg.get("level") == "FAIL"
                                and re.search(r"No keyword with name .* found", msg.text or "")), None)
                if missing:
                    records[name].update(status="unsupported", category="adapter_missing", reason=missing)
                sleeps = sum(keyword.get("name") == "Sleep" and keyword.get("library") == "BuiltIn"
                             and keyword.find("status").get("status") != "NOT RUN"
                             for keyword in node.iter("kw"))
                mirrored = virtual_sleep.mirrored.get(name, 0) if virtual_sleep else 0
                if records[name]["status"] in {"pass", "fail"} and sleeps > mirrored:
                    records[name].update(status="unsupported", category="adapter_missing",
                                         reason="requires asynchronous emulation during host Sleep")
                if node.find("status").get("status") == "FAIL" and records[name]["status"] == "pass":
                    records[name].update(status="fail", category="assertion_failed", reason="Robot XML reports failure")
                if records[name]["category"] == "no_measurement" and re.search(
                        r"FileNotFoundError|ModuleNotFoundError|Importing library|PermissionError|No such file|"
                        r"Parent suite setup failed|Connection refused|Timeout exceeded",
                        records[name]["reason"], re.I):
                    records[name]["category"] = "environment"
                if records[name]["category"] == "no_measurement" and path in EXTERNAL_INPUTS:
                    variable, default = EXTERNAL_INPUTS[path]
                    source = Path(os.environ.get(variable, default))
                    if not source.is_dir():
                        records[name].update(category="environment",
                                             reason=f"{variable} input directory missing: {source}")
                case, directory = active[name]
                write_case(case, records[name], directory)
        return [dict(case, engine="penv", **records.get(case["name"], dict(
            status="not_run", category="environment", reason="case did not reach Robot listener"))) for case in cases]
    finally:
        os.chdir(old_cwd)


def run_batch(selected, options, renode, binary, out, assets, jobs, host_case_timeout=130, rust_monitor=False):
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
    runner = partial(run_suite, options=options, renode=renode, binary=binary, assets=assets,
                     host_case_timeout=host_case_timeout, rust_monitor=rust_monitor)
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
                         for r in [json.loads(p.read_text())] if not r.get("provisional")}
                records = [saved.get((r["id"], hotspot), r) for r in records]
            all_results[path, hotspot] = records
            counts.update(record["status"] for record in records)
            snapshot = dict(completed=index, total=len(tasks), path=path, hotspot=hotspot,
                            counts=dict(counts))
            print(json.dumps(snapshot), file=progress, flush=True)
            print(f"Robot {index}/{len(tasks)} {path} {hotspot} {dict(counts)}", flush=True)
            write_json(directory.with_suffix(".json"), records)
    return [record for key in groups for record in all_results[key]]
