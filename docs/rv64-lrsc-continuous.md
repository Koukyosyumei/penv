# PolarFire RV64 の連続実行 LR/SC

固定 Renode の `tests/unit-tests/riscv-lr-sc-instructions.robot` の2ケースを、同じ PolarFire REPL、共有アドレス `0x80000600`、ハートごとに1000万回の更新で照合した。`u54_1` と `u54_2` が終了ジャンプに達すると、それぞれのハートを停止する。代表ケースの変更前実行では、ハートの初期停止を設定する CLI 操作がなかった。

| 固定ケース | 両ハートの終了 PC | 共有カウンター |
| --- | --- | --- |
| Two LR/SC Loops Should Increment To Correct Sum | `0x80000010`、`0x80000110` | 20,000,000 |
| LR/SC And Amoadd Loops Should Increment To Correct Sum | `0x80000008`、`0x80000110` | 20,000,000 |

上流のループを GAS で組み立て、実行前に停止する最後のジャンプには同じ位置でラベルを定義した。Penv では `--halt-cpu` で他の3ハートを停止し、`--halt-cpu-at` と `--stop-when-halted` で両ハートの完了を待つ。終了 PC、ループカウンターのゼロ、共有カウンターを照合する。実行枠の使い切りは失敗となる。

固定 Robot のログ待ちは Renode の仮想時間で10秒、CPU の既定は100 MIPS。Penv はハート単位のラウンドロビンで10億スロットを上限とした。5ハートで停止中のハートにもスロットを割り当てるため、各実行ハートの上限は2億スロットで、固定 Robot の10秒に対応する10億命令より短い。両ケースがこの制限内で成功した。等価判定はループと完了・共有値の条件に基づき、Renode の時間量子ごとのスケジューリングは再現していない。

## 再現

```sh
out=target/backlog-rv64-lrsc-continuous
ref=/var/tmp/penv-rv64-act-upstream
work=/tmp/penv-rv64-full
mkdir -p "$out"
cargo test > "$out/cargo-test.log" 2>&1
cargo build --release > "$out/build.log" 2>&1
PATH="$ref/dotnet:$ref/robot-venv/bin:$PATH" \
  "$ref/renode/renode-test" "$ref/renode/tests/unit-tests/riscv-lr-sc-instructions.robot" \
  --fixture 'Loops Should Increment To Correct Sum' --results-dir "$PWD/$out/upstream" -P 0 \
  > "$out/upstream.log" 2>&1
python3 scripts/verify-rv64-lrsc-continuous.py \
  --cc "$work/xpack-riscv-none-elf-gcc-15.2.0-1/bin/riscv-none-elf-gcc" \
  --logs "$out/penv" > "$out/penv.log" 2>&1
python3 scripts/probe-rv64-isa.py --act --robot-isa "$work/arch-git/work" \
  --jobs 4 --logs "$out/act" > "$out/act.tsv" 2> "$out/act.stderr.log"
python3 scripts/verify-rv64-virt.py "$virt_work" --logs "$out/virt" > "$out/virt.log" 2>&1
```

固定 Renode と ACT の準備は [ACT 終了通知と署名](rv64-act-equivalence.md)、公開ファームウェア素材は [README の RV64 virt 手順](../README.md)を参照。ログと終了状態は `out` に保存した。

固定 Robot と Penv で2ケースとも合格。ハート指定ステップの5ケースと補足1件も再確認した。`cargo test` は125件合格。固定 ACT の診断は前回と同じ216本成功・26本失敗。公開 RV64 virt イメージで OpenSBI、Linux、UART の root ログイン、および `id -u` と `uname -m` の実行を確認した。

## 残課題

ハート指定ステップの部分移植1件と、RV32／RV64 混在と MMIO を含む強い原子性ケースが残る。詳細は [ハート指定ステップの記録](rv64-lrsc-step.md)を参照。
