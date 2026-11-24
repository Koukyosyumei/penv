#!/usr/bin/env python3
"""Track exact first-blocking include/i and set operations by upstream case ID."""

import argparse
from collections import Counter
import json
from pathlib import Path

from compare import load


LIFECYCLE = {"Reset Emulation", "Close Log File", "Set Crash On Abort"}


def command_call(call):
    args = call.get("args", [])
    if call["keyword"] == "Execute Command" and args and isinstance(args[0], str):
        return args[0].strip()
    return ""


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
    cohorts, cases = {}, []
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
