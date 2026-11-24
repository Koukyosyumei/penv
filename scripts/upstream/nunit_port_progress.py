#!/usr/bin/env python3
"""Join canonical mappings and pinned discovery without executing any tests."""

import argparse
from collections import Counter, defaultdict
import csv
import gzip
import json
from pathlib import Path
import sys

sys.dont_write_bytecode = True

from nunit_mapping import load_mapping


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("inventory", type=Path, help="pinned discovery inventory .jsonl or .jsonl.gz")
    parser.add_argument("output", type=Path, help="fixture progress TSV")
    args = parser.parse_args()
    mapping = load_mapping()
    gaps = Path(__file__).with_name("nunit_gaps.tsv")
    with gaps.open(newline="", encoding="utf-8") as file:
        notes = {row["fixture"]: row["missing_api"] for row in csv.DictReader(file, delimiter="\t")}
    read = gzip.open if args.inventory.suffix == ".gz" else open
    progress = defaultdict(Counter)
    discovered = set()
    matched = set()
    with read(args.inventory, "rt", encoding="utf-8") as source:
        for row in map(json.loads, source):
            project, name = row["id"].split("::", 1)
            key = (project, name)
            if key in discovered:
                raise ValueError(f"Duplicate discovery ID {row['id']}")
            discovered.add(key)
            if key in mapping:
                status = "ported_excluded" if row["status"] == "excluded" else "ported"
                matched.add(key)
            elif row["status"] in {"ignored", "skipped", "excluded", "explicit", "not_runnable"}:
                status = "inactive"
            else:
                status = "pending"
            progress[(project, row["fixture"])][status] += 1

    if matched != set(mapping):
        raise ValueError(f"Mapped IDs missing from discovery: {sorted(set(mapping) - matched)}")
    states = ("ported", "ported_excluded", "pending", "inactive")
    with args.output.open("w", newline="", encoding="utf-8") as output:
        writer = csv.writer(output, delimiter="\t", lineterminator="\n")
        writer.writerow(("project", "fixture", "discovered", *states, "remaining_api"))
        for (project, fixture), counts in sorted(progress.items()):
            note = notes.get(fixture, "Requires API review") if counts["pending"] else "-"
            writer.writerow((project, fixture, sum(counts.values()), *(counts[state] for state in states), note))
    totals = sum(progress.values(), Counter())
    print(json.dumps({state: totals[state] for state in states}))


if __name__ == "__main__":
    main()
