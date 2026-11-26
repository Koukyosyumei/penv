#!/usr/bin/env python3
"""Compare two results.json files by engine, upstream ID and parameters."""

import argparse
from collections import Counter
import json
from pathlib import Path
import re


def load(path):
    report = json.loads(path.read_text())
    rows = {}
    for row in report["cases"]:
        if row["status"] == "diagnostic":
            continue
        key = (row["engine"], row["id"], json.dumps(row["parameters"], sort_keys=True))
        if key in rows:
            raise ValueError(f"duplicate case identity in {path}: {key!r}")
        rows[key] = row
    return report, rows


def reason_group(row):
    reason = (row.get("reason") or row.get("message") or "").splitlines()
    if not reason:
        return "no recorded reason"
    first = reason[0]
    monitor = re.match(r"Execute Command: (\S+) (\S+)", first)
    if monitor:
        device = re.sub(r"\d+$", "", monitor[1].removeprefix("sysbus."))
        return f"Execute Command: {device} {monitor[2]}"
    return first


def summarize(path):
    report, rows = load(path)
    engines = sorted({key[0] for key in rows})
    return report, rows, {
        engine: dict(Counter(row["verdict"] for key, row in rows.items() if key[0] == engine))
        for engine in engines
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    args = parser.parse_args()
    before_report, before, before_counts = summarize(args.before)
    after_report, after, after_counts = summarize(args.after)
    before_meta, after_meta = before_report["metadata"], after_report["metadata"]
    for name in ("upstream_sha", "host"):
        if before_meta[name] != after_meta[name]:
            parser.error(f"reports have different {name}")
    transitions = Counter()
    changed = []
    for engine, case_id, params in sorted(before.keys() | after.keys()):
        key = engine, case_id, params
        old = before.get(key, {}).get("verdict", "ABSENT")
        new = after.get(key, {}).get("verdict", "ABSENT")
        if old == new:
            continue
        transitions[engine, old, new] += 1
        changed.append(dict(engine=engine, id=case_id, parameters=json.loads(params),
                            before=old, after=new, category=after.get(key, {}).get("category"),
                            reason=after.get(key, {}).get("reason")))
    reasons = Counter((row.get("category", "unknown"), reason_group(row))
                      for row in after.values() if row["engine"] == "penv"
                      and row["verdict"] in {"FAIL", "UNSUPPORTED", "NOT_RUN"})
    print(json.dumps(dict(
        upstream_sha=after_meta["upstream_sha"], before_sha=before_meta["penv_sha"],
        after_sha=after_meta["penv_sha"], before=before_counts, after=after_counts,
        transitions=[dict(engine=e, before=b, after=a, count=count)
                     for (e, b, a), count in sorted(transitions.items())],
        reasons=[dict(category=category, reason=reason, count=count)
                 for (category, reason), count in reasons.most_common()],
        changed=changed,
    ), indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
