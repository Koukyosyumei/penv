# RV64 AndeStar V5 のスカラー BF16 変換

固定 Renode の `tests/unit-tests/riscv-andestar-v5.robot` にある「Should Handle Scalar BF16 NaN Boxing And Overflow On RV64」を移植した。CPU は `rv64gc_xandesbfhcvt`、RAM は `0x0..0xfffff`。固定ケースと同じ `mstatus`、F1、丸めモード、初期例外フラグを設定し、1ステップ後の F5、FFLAGS、PC を比較する。

| 固定入力 | 比較結果 |
| --- | --- |
| `nds.fcvt.s.bf16`、F1=`0x0` | 不正な NaN ボクシングを正規 NaN として変換し、F5=`0xffffffff7fc00000`、FFLAGS=0 |
| `nds.fcvt.bf16.s`、F1=`0xffffffff7f7fffff` | 最近接偶数丸めで無限大となり、F5=`0xffffffffffff7f80`、FFLAGS=5 |

代表ケースは変更前の Penv で ISA 文字列の解析時に失敗した。スカラー変換は Penv の整数ベースの浮動小数点変換に BF16 形式を追加して実装した。命令エンコードは固定 Infrastructure の `Andes_AndeStarV5Extension/FloatingPoint.cs` に対応する。

## 再現

```sh
out=target/backlog-rv64-andes-bf16
ref=/var/tmp/penv-rv64-act-upstream
work=/tmp/penv-rv64-full
mkdir -p "$out"
cargo test > "$out/cargo-test.log" 2>&1
cargo build --release > "$out/build.log" 2>&1
PATH="$ref/dotnet:$ref/robot-venv/bin:$PATH" \
  "$ref/renode/renode-test" "$ref/renode/tests/unit-tests/riscv-andestar-v5.robot" \
  --fixture 'Should Handle Scalar BF16 NaN Boxing And Overflow On RV64' \
  --results-dir "$PWD/$out/upstream" -P 0 > "$out/upstream.log" 2>&1
python3 scripts/verify-rv64-andes-lea.py ../renode/tests/unit-tests/riscv-andestar-v5.robot \
  --logs "$out/penv" > "$out/penv.log" 2>&1
python3 scripts/probe-rv64-isa.py --act --robot-isa "$work/arch-git/work" \
  --jobs 4 --logs "$out/act" > "$out/act.tsv" 2> "$out/act.stderr.log"
python3 scripts/verify-rv64-virt.py "$virt_work" --logs "$out/virt" > "$out/virt.log" 2>&1
```

固定 Renode と ACT の準備は [ACT 終了通知と署名](rv64-act-equivalence.md)、公開ファームウェア素材は [README の RV64 virt 手順](../README.md)を参照。ログと終了状態は `out` に保存した。

固定 Robot と Penv で対象ケースが合格した。既存の AndeStar 12ケースも同じ Penv バイナリで合格した。`cargo test` は125件合格。固定 ACT 242本の診断結果は前回と同じ216本成功・26本失敗。公開 RV64 virt イメージで OpenSBI から Linux を起動し、UART から root ログインして `id -u` と `uname -m` を実行した。

## 残課題

ベクトル BF16 ケースには `xandesvbfhcvt` のデコード、SEW=16 に対する16↔32ビット変換、拡幅・縮幅時のレジスターグループ重複検査、設定 CSR `0xfc2` と `0xfc7` が必要。Quad MAC も未達。
