#!/bin/sh
# python-engine.robot:97-102, before any platform is created.
set -eu
[ "$#" -eq 2 ] || exit 1
penv=$1
out=$2
mkdir -p "$out"
cat >"$out/invalid-type.resc" <<'RESC'
$a=1
py $a
RESC
status=0
"$penv" run --script "$out/invalid-type.resc" >"$out/run.stdout" 2>"$out/run.stderr" || status=$?
[ "$status" -eq 1 ]
[ ! -s "$out/run.stdout" ]
grep -Fq 'There was an error when executing command '\''py $a'\'': Variable type has to be a string.' "$out/run.stderr"
cat "$out/run.stderr"
