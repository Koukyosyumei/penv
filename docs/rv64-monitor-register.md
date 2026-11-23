# RV64 Monitor レジスター照合

固定 tlib の `arch/riscv/cpu_registers.c` では Monitor の `MSTATUS_64`、`MEPC_64`、`SEPC_64` が保存状態を直接参照します。Penv に `--read-register CPU@NAME` を追加し、指定ハートの `mstatus`、`mepc`、`sepc`、`pc`、`x0`～`x31` を停止時に標準エラーへ出します。`--set-mstatus CPU:VALUE` は起動前に Penv が実装する状態フィールドを設定します。

Penv のゲスト CSR 読み出しは従来の UXL・SXL・SD 合成を使います。Monitor 観測では保存状態を読みます。Zcb を有効にした固定構成で `mstatus=0x200` から `csrwi vcsr, 3` を1ステップ実行すると、Monitor 値は `0x600` です。

## 検証と残課題

| 固定 Robot | Penv の照合 |
| --- | --- |
| `riscv-vector-csr.robot:191` Should Mark MSTATUS.VS As Dirty | 固定 ISA、初期値、1ステップ、Monitor 値 `0x600` |
| `riscv-unit-tests.robot:401` Should Keep MEPC First Bit As Zero | 固定 ISA、PC `0x2000`、2ステップ、MEPC `0xfe` |
| `riscv-unit-tests.robot:508` Should Keep SEPC First Bit As Zero | 固定 ISA、PC `0x2000`、2ステップ、SEPC `0xfe` |

固定 Renode と Penv の ELF 移植で3ケースの結果が一致しました。Penv は指定ステップ数に達すると終了値2を返すため、検証では終了理由と表示値を両方確認しました。`cargo test` は122件合格しました。

`Sm-00.elf` が比較するのはゲスト CSR の署名です。Monitor で観測できるようになってもビット23・41の署名差分は残ります。カスタム CSR の読み書きフックを要する Robot 2ケースも未達です。

## 再現

ログと終了状態は `target/backlog-rv64-register/` に保存しました。`$renode_work` はビルド済み固定 Renode、`$out` は検証ログディレクトリです。REPL は [Zcb 検証のデフォルト VLEN 構成](rv64-zcb.md) を `mstatus.repl` にし、同じ REPL の `cpuType` を `rv64gc_Zfh` に置き換えたものを `epc.repl` にします。

```sh
cargo test
cargo build --release
"$renode_work/renode-test" "$renode_work/tests/unit-tests/riscv-vector-csr.robot" \
  --fixture 'Should Mark MSTATUS.VS As Dirty' --results-dir "$out/upstream-mstatus" -P 0
"$renode_work/renode-test" "$renode_work/tests/unit-tests/riscv-unit-tests.robot" \
  --fixture 'Should Keep ?EPC First Bit As Zero' --results-dir "$out/upstream-epc" -P 0
for reg in mstatus mepc sepc; do
  define=-DEPC="$reg"
  if [ "$reg" = mstatus ]; then define=-DMSTATUS; fi
  riscv-none-elf-gcc "$define" -march=rv64gcv -mabi=lp64d -nostdlib -static \
    -Wl,-n,-Ttext=0x2000,-e,_start tests/fixtures/rv64-monitor-register.S -o "$out/$reg.elf"
  if [ "$reg" = mstatus ]; then
    args=(--platform "$out/mstatus.repl" --max-steps 1 --mtvec 0x1010 --set-mstatus cpu:0x200)
  else
    args=(--platform "$out/epc.repl" --max-steps 2)
  fi
  status=0
  target/release/penv run "${args[@]}" --read-register "cpu@$reg" "$out/$reg.elf" \
    > "$out/$reg.log" 2>&1 || status=$?
  test "$status" -eq 2 || exit 1
done
```
