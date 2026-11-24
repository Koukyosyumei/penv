#!/usr/bin/env python3
"""Compare pinned Migrant fixture constructor matrices with NUnit discovery."""

import argparse
from collections import defaultdict
import csv
import gzip
import json
from pathlib import Path
import re
import subprocess


PROJECT = "lib/Migrant/Tests/Tests.csproj"
# Compare the gitlink itself, not an arbitrary checkout with similarly named tests.
PIN = "169677b3e43dbbc530c9f7c3f4a13ba49e40bb2d"


def constructor_matrix(source, fixture):
    declarations = re.findall(r"\[TestFixture\(([^)]*)\)\]", source)
    tuples = {tuple(value.strip() == "true" for value in declaration.split(","))
              for declaration in declarations}
    if not tuples:
        return (), {()}
    constructor = re.search(rf"public\s+{re.escape(fixture)}\s*\(([^)]*)\)", source)
    if not constructor:
        raise ValueError(f"Missing constructor for {fixture}")
    names = tuple(re.findall(r"\bbool\s+(\w+)", constructor[1]))
    if len(tuples) != len(declarations) or any(len(values) != len(names) for values in tuples):
        raise ValueError(f"Unexpected fixture matrix for {fixture}")
    return names, tuples


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("inventory", type=Path)
    parser.add_argument("migrant", type=Path, help="pinned Migrant submodule checkout")
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    def git(*command):
        return subprocess.check_output(["git", "-C", str(args.migrant), *command], text=True)

    if git("rev-parse", "HEAD").strip() != PIN:
        parser.error(f"expected pinned Migrant source at {PIN}")
    read = gzip.open if args.inventory.suffix == ".gz" else open
    fixtures = defaultdict(lambda: defaultdict(set))
    ids = set()
    with read(args.inventory, "rt", encoding="utf-8") as source:
        for case in map(json.loads, source):
            project, name = case["id"].split("::", 1)
            if project != PROJECT:
                continue
            if name in ids:
                raise ValueError(f"Duplicate Migrant case ID {name}")
            ids.add(name)
            tail = name.removeprefix(case["fixture"])
            match = re.fullmatch(r"(?:\((True|False)(?:,(?:True|False))*\))?\.(.+)", tail)
            if not match:
                raise ValueError(f"Unrecognized Migrant case {name}")
            config = tail[1:tail.index(")")].split(",") if match[1] else []
            values = tuple(value == "True" for value in config)
            fixtures[case["fixture"]][values].add(match[2])
    if len(ids) != 2247 or len(fixtures) != 13:
        raise ValueError(f"Expected 2,247 Migrant cases in 13 fixtures, got {len(ids)} in {len(fixtures)}")

    with args.output.open("w", newline="", encoding="utf-8") as output:
        writer = csv.writer(output, delimiter="\t", lineterminator="\n")
        writer.writerow(("fixture", "constructor_parameters", "constructor_variants",
                         "cases_per_variant", "total_cases", "method_parameter_cases"))
        for name, variants in sorted(fixtures.items()):
            fixture = name.rsplit(".", 1)[1]
            source = git("show", f"HEAD:Tests/{fixture}.cs")
            parameters, expected = constructor_matrix(source, fixture)
            if set(variants) != expected:
                raise ValueError(f"Discovery differs from source matrix for {fixture}")
            baseline = next(iter(variants.values()))
            if any(cases != baseline for cases in variants.values()):
                raise ValueError(f"Different method signatures between variants of {fixture}")
            writer.writerow((name, ",".join(parameters) or "-", len(variants), len(baseline),
                             len(variants) * len(baseline),
                             sum("(" in case for case in baseline) * len(variants)))


if __name__ == "__main__":
    main()
