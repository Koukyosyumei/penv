"""Canonical mapping of concrete NUnit IDs to Rust tests."""

import csv
from pathlib import Path


PIN = "c38ac3babcd38a7144b9498e3ca4c2226b76aa37"


def load_mapping():
    path = Path(__file__).with_name("nunit_cases.tsv")
    with path.open(newline="") as source:
        rows = list(csv.DictReader(source, delimiter="\t", quoting=csv.QUOTE_NONE))
    mapping = {(row["project"], row["official_fullname"]): row for row in rows}
    if len(mapping) != len(rows) or len({row["rust_test"] for row in rows}) != len(rows):
        raise ValueError("NUnit case IDs and Rust test names must each be unique")
    return mapping


def case_parameters(method, display_name):
    parameters = display_name.removeprefix(method)
    if method in {"ShouldParseHexadecimalLong", "ShouldParseNumber", "ShouldNotParseNumber"}:
        if parameters.startswith('("') and parameters.endswith('")'):
            return {"number": parameters[2:-2]}
    return {"display": parameters} if parameters else {}
