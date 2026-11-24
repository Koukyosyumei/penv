#!/usr/bin/env python3
"""Track exact first-blocking include/i and set operations by upstream case ID."""

import argparse
from collections import Counter
import json
from pathlib import Path
import sys
import xml.etree.ElementTree as ET

sys.dont_write_bytecode = True

from compare import load


LIFECYCLE = {"Reset Emulation", "Close Log File", "Set Crash On Abort"}


def command_call(call):
    args = call.get("args", [])
    if call["keyword"] == "Execute Command" and args and isinstance(args[0], str):
        return args[0].strip()
    return ""


def next_from_xml(row, command, cache):
    evidence = row.get("evidence")
    if not evidence or not Path(evidence).is_file():
        return None
    if evidence not in cache:
        cache[evidence] = {node.get("name"): node
                           for node in ET.parse(evidence).getroot().findall("./suite/test")}
    test = cache[evidence].get(row.get("name"))
    if test is None:
        return None
    found = False
    for parent in test.findall("kw"):
        if parent.get("type") in {"SETUP", "TEARDOWN"}:
            continue
        for keyword in parent.iter("kw"):
            name = keyword.get("name")
            status = keyword.find("status")
            if status is None or status.get("status") == "NOT RUN":
                continue
            if found:
                return {"keyword": name, "status": status.get("status"), "evidence": evidence}
            if name == "Execute Command" and status.get("status") == "PASS" and any(
                    (arg.text or "").strip() == command for arg in keyword.findall("arg")):
                found = True
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("rerun", type=Path)
    args = parser.parse_args()
    before_report, before = load(args.baseline)
    after_report, after = load(args.rerun)
    if before_report["metadata"]["upstream_sha"] != after_report["metadata"]["upstream_sha"]:
        parser.error("reports have different upstream SHAs")
    if before_report["metadata"]["host"] != after_report["metadata"]["host"]:
        parser.error("reports have different hosts")
    cohorts, cases, xml = {}, [], {}
    for key, row in before.items():
        blocked = next((call for call in row.get("remote_calls", [])
                        if call["status"] == "unsupported"), None)
        command = command_call(blocked) if blocked else ""
        verb = command.partition(" ")[0]
        if verb not in {"include", "i", "set"}:
            continue
        family = "include/i" if verb in {"include", "i"} else "set"
        rerun = after.get(key, {})
        calls = rerun.get("remote_calls", [])
        index = next((i for i, call in enumerate(calls) if command_call(call) == command), None)
        retry = calls[index] if index is not None else {}
        following = next((call for call in calls[index + 1:] if call["keyword"] not in LIFECYCLE),
                         None) if index is not None else None
        if retry.get("status") == "ok":
            following = following or next_from_xml(rerun, command, xml)
            outcome = "next_operation" if following or rerun.get("verdict") == "PASS" else "accepted_only"
        elif retry.get("penv_ready") or retry.get("command"):
            outcome = "penv_attempted_same_operation"
        elif retry.get("include"):
            outcome = "resolved_same_operation"
        elif retry:
            outcome = "still_blocked"
        else:
            outcome = "not_reached"
        cohorts.setdefault(family, Counter())[outcome] += 1
        cases.append(dict(id=row["id"], parameters=row["parameters"], family=family,
                          command=command, progress=outcome, call=retry, next_call=following,
                          verdict=rerun.get("verdict"), category=rerun.get("category"),
                          reason=rerun.get("reason"), evidence=rerun.get("evidence")))
    print(json.dumps(dict(cohorts={name: dict(counts, total=sum(counts.values()))
                                  for name, counts in cohorts.items()}, cases=cases),
                     indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
