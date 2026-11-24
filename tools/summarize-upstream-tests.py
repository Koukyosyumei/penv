#!/usr/bin/env python3
"""Combine the six penv upstream test ledgers from committed Git snapshots."""

import argparse
from collections import Counter, defaultdict
import csv
import io
from pathlib import Path
import subprocess

LEDGERS = {
    "arm": ("docs/arm-upstream-cases.tsv", ("source", "file", "line", "parameter")),
    "riscv": ("docs/riscv-upstream-cases.tsv", ("component", "file", "line", "parameter")),
    "rv64": ("docs/rv64-upstream.tsv", ("tree", "file", "line", "parameter_source")),
    "network": ("tests/stm32h7-full-upstream.tsv", ("repository", "source", "lines", "parameter_bindings")),
    "nrf52": ("tests/nrf52840-full-upstream-index.tsv", ("repository", "file", "lines", "parameter")),
    "stm32f407": ("docs/stm32f407-upstream-cases.tsv", ("repository", "file", "line", "parameters")),
}
RENODE_REVISION = "c38ac3babcd38a7144b9498e3ca4c2226b76aa37"
STATES = ("pass", "partial", "open", "review", "outside", "conflict")


def git(path, *args):
    return subprocess.check_output(["git", "-C", str(path), *args], text=True)


def classify(lane, row):
    status = row.get("status", "").lower()
    reason = next((row[key] for key in (
        "evidence_or_pending", "basis", "evidence_or_reason", "evidence_or_gap"
    ) if key in row), "")
    if lane == "nrf52":
        scope = row["applicability"]
        reason = row["verification"]
        if row.get("result"):
            states = {"pass": "pass", "partial": "partial", "unmet": "open",
                      "unexpanded": "review", "excluded": "outside"}
            return states[row["result"]], f"{scope}; {reason}"
        if "対象外の根拠" in reason:
            return "outside", f"{scope}; {reason}"
        if reason.startswith("合格"):
            return "pass", reason
        if "部分移植" in reason or "照合待ち" in reason:
            return "partial", reason
        return ("open" if scope == "対象" or scope.endswith("・対象") else "review"), reason
    if status in {"out", "outside", "out of scope", "not applicable", "not-applicable"}:
        return "outside", reason
    if status in {"pass", "equivalent pass", "ported", "ported-pass"}:
        return "pass", reason
    if status in {"partial", "existing translation"}:
        return "partial", reason
    if (
        status in {"review", "open applicability", "expanded", "unexpanded"}
        or lane == "riscv" and row["scope"].startswith("review")
        or lane == "rv64" and status == "pending"
    ):
        return "review", reason
    if status in {"pending", "blocked-cpu", "open", "unmet"}:
        return "open", reason
    raise ValueError(f"{lane}: unknown ledger status {status!r}")


def load_scope_rules(workspaces):
    rules = {}
    path = Path(__file__).resolve().parents[1] / "docs/upstream-scope-rules.tsv"
    with path.open() as file:
        for row in csv.DictReader(file, delimiter="\t"):
            source = row["source"]
            fixture = git(workspaces / "renode", "show", f"{RENODE_REVISION}:{source}")
            if row["upstream_anchor"] not in fixture:
                raise ValueError(f"{source}: expected upstream fixture reference missing")
            for lane in row["lanes"].split(","):
                rules[(lane, source)] = row["reason"]
    return rules


def collect(workspaces):
    scope_rules = load_scope_rules(workspaces)
    applied_rules = Counter()
    records = defaultdict(list)
    revisions = {}
    total = 0
    for lane, (ledger, fields) in LEDGERS.items():
        workspace = workspaces / f"penv-{lane}"
        revision = revisions[lane] = git(workspace, "rev-parse", "HEAD").strip()
        rows = csv.DictReader(io.StringIO(git(workspace, "show", f"{revision}:{ledger}")), delimiter="\t")
        for row in rows:
            repo, source, line, variant = (row[field].strip() for field in fields)
            if row.get("configuration"):
                variant = f"{variant} | {row['configuration'].strip()}"
            repo = repo.lower().replace("-", "").replace(".", "")
            case = row["case"].strip()
            state, reason = classify(lane, row)
            rule = scope_rules.get((lane, source)) if repo == "renode" else None
            if state == "review" and rule is not None:
                state = "outside"
                reason = f"上流フィクスチャ判定: {rule}; 元台帳: {reason}"
                applied_rules[lane] += 1
            records[(repo, source, case, lane, line, variant)].append((state, reason))
            total += 1
    return records, revisions, total, applied_rules


def combine(evidence):
    states = {state for state, _ in evidence}
    if "outside" in states and states & {"pass", "partial", "open"}:
        return "conflict"
    return next(state for state in ("pass", "partial", "open", "review", "outside") if state in states)


def summarize(records, revisions, total, applied_rules, output):
    source_cases = defaultdict(set)
    counts = defaultdict(Counter)
    output.mkdir(parents=True, exist_ok=True)
    fields = ("repository", "source", "case", "lane", "source_line", "variant", "state", "evidence")
    with (output / "upstream-merged.tsv").open("w") as file:
        writer = csv.writer(file, delimiter="\t", lineterminator="\n")
        writer.writerow(fields)
        for key, evidence in sorted(records.items()):
            state = combine(evidence)
            counts[key[3]][state] += 1
            source_cases[key[:3]].add(key[3])
            writer.writerow((*key, state, " | ".join(sorted({reason for _, reason in evidence if reason}))))
    aggregate = sum(counts.values(), Counter())
    confirmed = sum(aggregate[state] for state in ("pass", "partial", "open"))
    report = [
        "# Renode 上流テストの集計",
        "",
        "担当ごとのコミット済み台帳を集約したスナップショットです。上流のリポジトリ・ファイル・ケース名の組を集計し、同じ出典が複数担当に現れる件数も数えます。",
        "実行候補では担当・出典行・パラメーターを保持し、同じケースでも別構成の検証を分けます。展開待ちのケースや記述の異なる構成は候補として残ります。",
        "",
        f"- 入力台帳の行数 {total:,}",
        f"- 重複を除いた上流のファイル・ケース名の組 {len(source_cases):,}",
        f"- 複数担当に現れる上流ケース名の組 {sum(len(lanes) > 1 for lanes in source_cases.values()):,}",
        f"- 担当・構成別の台帳行 {len(records):,}",
        f"- 対象確定分の明示合格 {aggregate['pass']:,} / {confirmed:,}"
        + (f"（{aggregate['pass'] / confirmed:.1%}）" if confirmed else ""),
        f"- 上流フィクスチャの確認で追加判定した対象外 {sum(applied_rules.values()):,} 行",
        "",
        "[判定規則](upstream-scope-rules.tsv)は固定 Renode にあるボード・CPU 参照を検証し、適用未判定の行だけに反映します。共有の機能テストは別途棚卸しします。",
        "",
        "| 担当 | 明示合格 | 部分移植・照合待ち | 対象・未達 | 適用・展開未確定 | 対象外 | 判定競合 |",
        "|---|---:|---:|---:|---:|---:|---:|",
    ]
    for lane in (*LEDGERS, "合計"):
        count = counts[lane] if lane in LEDGERS else aggregate
        report.append(f"| {lane} | " + " | ".join(f"{count[state]:,}" for state in STATES) + " |")
    report += [
        "",
        "明示合格は元台帳が合格と記録した行です。統合ビルドでの再実行は別途必要です。",
        "適用・展開未確定には、共有ケースの適用判断と未展開パラメーターが含まれます。共通の対象総数と合格率は、これらを解消してから確定します。",
        "",
        "## 再集計",
        "",
        "```sh",
        "python3 tools/summarize-upstream-tests.py --output /tmp/penv-upstream-audit",
        "```",
        "",
        "構成別の明細は出力先の `upstream-merged.tsv` に保存します。",
        "",
        "## 参照コミット",
        "",
        *(f"- {lane} `{revision}`" for lane, revision in revisions.items()),
    ]
    (output / "upstream-summary.md").write_text("\n".join(report) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspaces", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    summarize(*collect(args.workspaces), args.output)


if __name__ == "__main__":
    main()
