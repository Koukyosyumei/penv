# PolarFire RV64 のハート指定ステップと LR/SC

固定 Renode の `tests/unit-tests/riscv-lr-sc-instructions.robot` から、ハート指定ステップで検証する5ケースを移植した。上流と同じ `platforms/cpus/polarfire-soc.repl` を使い、`u54_1` と `u54_2` だけを指定して実行する。変更前は複数ハート構成で `--step-cpu` がエラーになった。名前付きステップは指定ハートを1ステップ進め、通常の実行では既存のラウンドロビンを使用する。

| 固定ケース | 台帳 | 照合内容 |
| --- | --- | --- |
| Should Handle Multiple Writes To Same Reservation | pass | 最初の SC は成功、同じ予約で2回目の SC は失敗しメモリ値6を保持 |
| Should Work On Multiple Cores | pass | 別アドレスを予約した両ハートの SC が成功 |
| Should Drop Other Core Reservation On SC | partial | 最初のハートの SC 失敗と第2ハートの実際のトラップを再現。第2ハートに SC を実行させる補足ケースも成功 |
| Should Handle Looping LR/SC Pairs To Same Reservation | pass | 固定の100反復でロード値、SC 成否、残り反復数、共有メモリ値を毎回照合 |
| Should Trap On Unaligned LR/SC | pass | ワード・ダブルワード両方で SC の例外原因6と LR の例外原因4、トラップ先 PC を照合 |

部分移植ケースでは、第2ハートは LR の後に PC `0x8000010e` に進む。固定 Robot が SC を配置するのは `0x80000110` なので、その次の1ステップは SC を実行せず、固定 Renode では PC `0x1010` にトラップする。Robot の `a3=0` はこの場合も成立する。Penv の補足ケースでは空白に NOP を置き、SC まで進めて成功を検査した。

## 再現

```sh
out=target/backlog-rv64-lrsc-step
ref=/var/tmp/penv-rv64-act-upstream
work=/tmp/penv-rv64-full
mkdir -p "$out"
cargo test > "$out/cargo-test.log" 2>&1
cargo build --release > "$out/build.log" 2>&1
i=0
for fixture in 'Should Handle Multiple Writes To Same Reservation' 'Should Work On Multiple Cores' \
  'Should Drop Other Core Reservation On SC' 'Should Handle Looping LR/SC Pairs To Same Reservation' \
  'Should Trap On Unaligned LR/SC'; do
  i=$((i+1))
  PATH="$ref/dotnet:$ref/robot-venv/bin:$PATH" \
    "$ref/renode/renode-test" "$ref/renode/tests/unit-tests/riscv-lr-sc-instructions.robot" \
    --fixture "$fixture" --results-dir "$PWD/$out/upstream-$i" -P 0 > "$out/upstream-$i.log" 2>&1
done
python3 scripts/verify-rv64-lrsc-step.py \
  --cc "$work/xpack-riscv-none-elf-gcc-15.2.0-1/bin/riscv-none-elf-gcc" \
  --logs "$out/penv" > "$out/penv.log" 2>&1
python3 scripts/probe-rv64-isa.py --act --robot-isa "$work/arch-git/work" \
  --jobs 4 --logs "$out/act" > "$out/act.tsv" 2> "$out/act.stderr.log"
python3 scripts/verify-rv64-virt.py "$virt_work" --logs "$out/virt" > "$out/virt.log" 2>&1
```

固定 Renode と ACT の準備は [ACT 終了通知と署名](rv64-act-equivalence.md)、公開ファームウェア素材は [README の RV64 virt 手順](../README.md)を参照。ケース別ログと終了状態は `out` に保存した。

固定 Robot の5ケースと Penv の対応する観測が通過した。上記の1ケースは部分移植として記録した。スタックアクセスの既存21ケースも同一バイナリで合格し、`cargo test` は125件合格。固定 ACT の診断は前回と同じ216本成功・26本失敗。公開 RV64 virt で OpenSBI から Linux を起動し、UART の root ログイン、`id -u`、`uname -m` の実行を確認した。

## 残課題

連続実行で各ハートが1000万回更新する2ケースは未達。完了 PC のフックと共有値の最終判定を、固定回数とスケジュール条件で照合する必要がある。別スイートの強い原子性ケースは RV32／RV64 混在と MMIO を同じケースで使用するため、ハート指定ステップだけでは等価移植できない。
