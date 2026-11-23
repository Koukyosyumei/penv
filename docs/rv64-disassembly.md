# RV64 のハート別逆アセンブル

`--disassemble CPU@BASE:WIDTH` を追加した。指定したハートから見える RAM の2または4バイトを、CPU の基本 ISA 拡張に合わせて `llvm-mc` で逆アセンブルする。CLI ではこの操作を指定したときだけ PATH 上の `llvm-mc` を使用する。検証に使用した版は15.0.7。

| 固定 Robot のケース | 結果 |
| --- | --- |
| `per-core-registration.robot` / Disassemble Code From Per Core Memory | 同じ公開 ELF と REPL を使用。同じ `0x3000000` から cpu1 は `1234 addi a3, sp, 296`、cpu2 は `5678 lw a4, 108(a2)` |
| `llvm-asm-disasm.robot` / Should Handle Illegal Instruction When Disassembling | `rv64g` で両方のゼロ入力から `Disassembly error detected` |
| `llvm-asm-disasm.robot` / Should Handle Disassembly From Invalid Address | `0x1234` の `02051613` は `slli a2, a0, 32`。未マップの `0x02000000` は診断を返す |

未マップアドレスの上流ケースは、RAM への書き込みを試みた後に逆アセンブルする。その書き込みは RAM を変更しないため、CLI では未マップアドレスをそのまま逆アセンブルに渡した。診断は後続のモニター操作を妨げない。

## 再現

公開 ELF の取得とハッシュ確認は [ハート別 RAM の記録](rv64-per-core.md)を参照。以下は同じ保存先を使う。

```sh
out=target/backlog-rv64-disassemble
ref=/var/tmp/penv-rv64-act-upstream
work=/tmp/penv-rv64-full
elf=target/backlog-rv64-per-core/multibus.elf
mkdir -p "$out"
cargo test > "$out/cargo-test.log" 2>&1
cargo build --release > "$out/build.log" 2>&1
export PATH="$ref/dotnet:$ref/robot-venv/bin:$PATH"
"$ref/renode/renode-test" "$ref/renode/tests/unit-tests/per-core-registration.robot" \
  --fixture 'Disassemble Code From Per Core Memory' --results-dir "$PWD/$out/upstream-core" -P 0
"$ref/renode/renode-test" "$ref/renode/tests/unit-tests/llvm-asm-disasm.robot" \
  --fixture 'When Disassembling' --results-dir "$PWD/$out/upstream-llvm" -P 0
"$ref/renode/renode-test" "$ref/renode/tests/unit-tests/llvm-asm-disasm.robot" \
  --fixture 'Should Handle Disassembly From Invalid Address' --results-dir "$PWD/$out/upstream-address" -P 0
python3 scripts/verify-rv64-disassembly.py "$elf" --logs "$out/penv" > "$out/penv.log" 2>&1
python3 scripts/verify-rv64-per-core.py "$elf" --logs "$out/firmware" > "$out/firmware.log" 2>&1
python3 scripts/probe-rv64-isa.py --act --robot-isa "$work/arch-git/work" \
  --jobs 4 --logs "$out/act" > "$out/act.tsv" 2> "$out/act.stderr.log"
```

固定 Robot と同一 Penv バイナリで3ケースとも合格。公開 ELF の UART とメモリ回帰4件も通過し、`cargo test` は125件合格した。固定 ACT の診断は前回と同じ216本成功・26本失敗。ケース別ログと終了状態は `out` に保存した。

## 残課題

アセンブルと逆アセンブルの往復ケースには、CPU のアセンブラー操作とバイト列比較が必要。複数命令を含むブロック、独自命令の逆アセンブル、ハート専用 UART の動的移動も未対応。
