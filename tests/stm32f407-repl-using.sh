#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

penv=${PENV_BIN:-target/release/penv}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

expect_error() {
    local expected=$1 source=$2 output status=0
    output=$("$penv" platform check --platform-root "$work" --string "$source" 2>&1) || status=$?
    if [[ $status != 1 || $output != *": $expected:"* ]]; then
        printf 'Expected %s, got exit %s\n%s\n' "$expected" "$status" "$output" >&2
        return 1
    fi
    printf '%s\n' "$output"
}

cat >"$work/A" <<'REPL'
cpu:
    Placeholder: "set by A first"
cpu:
    Placeholder: "set by A second"
REPL
cat >"$work/B" <<'REPL'
cpu:
    Placeholder: "set by B first"
cpu:
    Placeholder: "set by B second"
REPL
expect_error DuplicateUsing '
using "A"
using "B"
using "A"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
'

cat >"$work/A" <<'REPL'
using "B"
p2: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral
REPL
cat >"$work/B" <<'REPL'
using "A"
p: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral
REPL
expect_error RecurringUsing '
using "B"
p3: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral'

cat >"$work/A" <<'REPL'
using "A"
p: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral
REPL
expect_error RecurringUsing '
using "A"
p2: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral'

rm "$work/A"
expect_error UsingFileNotFound '
using "A"'
