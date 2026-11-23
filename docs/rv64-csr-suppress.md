# RV64 の CSR 読み出し抑止

`CSRRW`、`CSRRWI` で `rd=x0` の場合、書き込みだけを実行し、ゲストの CSR 読み出しを省略します。`CSRRSI` の読み書きは `rd=x0` でも実行します。書き込み先の存在、権限、読み取り専用の検査を保持します。

既存の `RegisterCustomCSR "BPM" 0x7C0 Machine` に `traceCustomCsr: true` を併用すると、ゲストからの読み出しと書き込みが別々にログへ出ます。固定テストの Python ハンドラーが行うログ観測をこの方法で移植しました。任意の Python CSR ハンドラーの実行は未実装です。

## ケース対応

- `riscv-unit-tests.robot:518` は名前に CSRRW とありますが、実際の命令は `csrrwi x0, 0x7c0, 13` です。
- 同 `riscv-unit-tests.robot:534` は `csrrw x0, 0x7c0, a0` を実行します。
- 固定 Renode と Penv で同じ ISA、PC `0x2000`、命令、1ステップを使用し、両ケースで読み出し通知なし・書き込み通知ありを確認しました。比較用 ELF の `csrrsi x0, 0x7c0, 1` は読み出し、書き込みの順に通知しました。

`cargo test` は122件合格しました。ログと終了状態は `target/backlog-rv64-csr-suppress/` に保存しました。

## 再現

`$renode_work` はビルド済み固定 Renode です。REPL は [Monitor EPC 検証の構成](rv64-monitor-register.md) と同じで、CPU に `traceCustomCsr` と BPM 登録を追加します。

```sh
out=target/backlog-rv64-csr-suppress
mkdir -p "$out"
cargo test
cargo build --release
"$renode_work/renode-test" "$renode_work/tests/unit-tests/riscv-unit-tests.robot" \
  --fixture 'Should Not Read From Csr When CSRRW* RD Is Zero' --results-dir "$out/upstream" -P 0
cat > "$out/csr.repl" <<'REPL'
clint: IRQControllers.CoreLevelInterruptor @ sysbus 0x44000000
    frequency: 66000000
cpu: CPU.RiscV64 @ sysbus
    timeProvider: clint
    cpuType: "rv64gc_Zfh"
    traceCustomCsr: true
    init:
        RegisterCustomCSR "BPM" 0x7C0 Machine
mem: Memory.MappedMemory @ sysbus 0x1000
    size: 0x40000
REPL
for variant in IMMEDIATE REGISTER RMW; do
  riscv-none-elf-gcc -D"$variant" -march=rv64gc -mabi=lp64d -nostdlib -static \
    -Wl,-n,-Ttext=0x2000,-e,_start tests/fixtures/rv64-csr-trace.S -o "$out/$variant.elf"
  status=0
  target/release/penv run --platform "$out/csr.repl" --max-steps 1 "$out/$variant.elf" \
    > "$out/$variant.log" 2>&1 || status=$?
  test "$status" -eq 2 || exit 1
  if [ "$variant" = RMW ]; then
    test "$(grep CSR "$out/$variant.log")" = "$(printf 'penv: CSR read! 0x7c0\npenv: CSR written! 0x7c0')" || exit 1
  else
    test "$(grep CSR "$out/$variant.log")" = 'penv: CSR written! 0x7c0' || exit 1
  fi
done
```
