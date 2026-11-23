# RV64 MENVCFG の固定 Renode 互換マスク

固定 tlib の `op_helper.c` にある `CSR_MENVCFG` への書き込みは、U 拡張がある場合に `cpu_bits.h` の `MENVCFG_RO_ZERO_MASK` を適用します。Penv の RV64 REPL で `renodeMenvcfg: true` を指定すると、この固定版の読み書き動作を使います。マスク外のビットは固定版と同様に読み返せます。既定の CSR と既存の Zicboz プロファイルは従来の動作です。

## 検証と残課題

- 固定 Renode の `riscv-unit-tests.robot:550` と Penv で `rv64gc_Zfh`、CLINT、RAM、初期 PC を合わせ、全ビットを書き込んで3ステップ目に `a1=0x07fffffcffffff02` を照合しました。台帳を上流等価の合格へ更新しました。
- 同スイートの単精度・半精度のゼロ除算2ケースは、固定 Renode で合格しました。Rust でも元の NaN ボックス化入力、`f2` の初期値と結果、PC、FFLAGS の DZ を照合しました。固定 tlib は F/D 有効時にリセットで FS を Initial に設定します。Rust ポートは FS を明示的に初期化するため、この段階では部分移植でした。後続の [RV64 浮動小数点リセット](rv64-fdiv-reset.md) で等価照合が完了しました。
- `cargo test` は124件合格しました。固定 ACT 242本の結果は変更前と一致しました。ログと終了状態は `target/backlog-rv64-menvcfg/` に保存しました。

## 再現

```sh
out=target/backlog-rv64-menvcfg
mkdir -p "$out"
cargo test
cargo build --release
"$renode_work/renode-test" "$renode_work/tests/unit-tests/riscv-unit-tests.robot" \
  --fixture 'Should Ignore Writes To MENVCFG Bits Of Disabled Or Unsupported Extensions' \
  --results-dir "$out/upstream-menvcfg" -P 0
"$renode_work/renode-test" "$renode_work/tests/unit-tests/riscv-unit-tests.robot" \
  --fixture 'Should Set Flag On * Precision Floating Point Division By Zero' \
  --results-dir "$out/upstream-fdiv" -P 0
cat > "$out/menvcfg.repl" <<'REPL'
clint: IRQControllers.CoreLevelInterruptor @ sysbus 0x44000000
    frequency: 66000000
cpu: CPU.RiscV64 @ sysbus
    timeProvider: clint
    cpuType: "rv64gc_Zfh"
    renodeMenvcfg: true
mem: Memory.MappedMemory @ sysbus 0x1000
    size: 0x40000
REPL
riscv-none-elf-gcc -march=rv64gc -mabi=lp64d -nostdlib -static \
  -Wl,-n,-Ttext=0x2000,-e,_start tests/fixtures/rv64-menvcfg.S -o "$out/menvcfg.elf"
status=0
target/release/penv run --platform "$out/menvcfg.repl" --max-steps 3 \
  --read-register cpu@x11 "$out/menvcfg.elf" > "$out/menvcfg.log" 2>&1 || status=$?
test "$status" -eq 2
grep -Fx 'penv: register cpu@x11 = 0x07fffffcffffff02' "$out/menvcfg.log"
```
