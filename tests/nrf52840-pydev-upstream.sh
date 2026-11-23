#!/bin/sh
set -eu
[ "$#" -eq 2 ] || { echo "Usage: $0 PENV ARTIFACT_DIR" >&2; exit 1; }
penv=$1
out=$2
mkdir -p "$out"
cat > "$out/quadword.resc" <<'RESC'
mach create
numbersMode Hexadecimal
machine PyDevFromString """
if request.IsInit:
    value = 0x1234567890abcdef
elif request.IsRead:
    request.Value = value
elif request.IsWrite:
    value = request.Value
""" 0x8 8 True
sysbus ReadQuadWord 0x8
sysbus ReadDoubleWord 0x8
sysbus WriteQuadWord 0x8 0xfedcba0987654321
sysbus ReadQuadWord 0x8
RESC
status=0
"$penv" run --script "$out/quadword.resc" > "$out/quadword.log" 2> "$out/quadword.stderr" || status=$?
printf '%s\n' "$status" > "$out/quadword.status"
[ "$status" -eq 0 ] || { cat "$out/quadword.stderr" >&2; exit 1; }
printf '%s\n' 0x1234567890abcdef 0x90abcdef 0xfedcba0987654321 > "$out/quadword.expected"
diff -u "$out/quadword.expected" "$out/quadword.log"
printf '%s\n' 'PASS python-engine.robot:72-96 PyDev Should Handle QuadWord Accesses'

# A bad handler must fail registration even without a bus access.
sed '/^sysbus /d; s/^if request.IsInit:/if request.IsInit/' "$out/quadword.resc" > "$out/invalid.resc"
status=0
"$penv" run --script "$out/invalid.resc" > "$out/invalid.log" 2>&1 || status=$?
printf '%s\n' "$status" > "$out/invalid.status"
[ "$status" -eq 1 ]
grep -q 'SyntaxError' "$out/invalid.log"
