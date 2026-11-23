# RV64 AndeStar V5 の GP 相対命令

固定 Renode の `tests/unit-tests/riscv-andestar-v5.robot` の2ケースを `rv64imac_xandesperf`、RAM `0x0..0xfffff`、開始 PC `0x2000` で照合した。固定 Infrastructure の `Andes_AndeStarV5Extension` に従い、バイト・ハーフワード用の18ビット即値とワード用の19ビット即値を符号拡張し、`gp` と64ビットで加算する。

| 固定ケース | 照合結果 |
| --- | --- |
| Should Sign Extend GP Loads On RV64 | `nds.lbgp`、`nds.lhgp`、`nds.lwgp` の符号拡張と `nds.lbugp`、`nds.lhugp` のゼロ拡張を確認 |
| Should Preserve Full Width Global Pointer On RV64 | `nds.addigp` で上位ビットの保持、負の即値、64ビット加算の折り返しを確認 |

全8組で固定フィクスチャの RAM 内容とレジスター値を設定し、1ステップ後の `a2` と PC `0x2004` を比較した。固定 Renode の GP ロードはシステムバスを直接読み、MMU・PMP・アラインメントは上流ソースでも未処理とされている。Penv では既存の RV64 ロード経路を使用する。等価判定は固定ケースの M モード整列 RAM アクセスに基づく。

## 再現

```sh
out=target/backlog-rv64-andes-gp
ref=/var/tmp/penv-rv64-act-upstream
work=/tmp/penv-rv64-full
mkdir -p "$out"
cargo test > "$out/cargo-test.log" 2>&1
cargo build --release > "$out/build.log" 2>&1
for mode in load add; do
  if [ "$mode" = load ]; then
    filter='Should Sign Extend GP Loads On RV64'
  else
    filter='Should Preserve Full Width Global Pointer On RV64'
  fi
  PATH="$ref/dotnet:$ref/robot-venv/bin:$PATH" \
    "$ref/renode/renode-test" "$ref/renode/tests/unit-tests/riscv-andestar-v5.robot" \
    --fixture "$filter" --results-dir "$PWD/$out/upstream-$mode" -P 0 \
    > "$out/upstream-$mode.log" 2>&1
done
python3 scripts/verify-rv64-andes-lea.py ../renode/tests/unit-tests/riscv-andestar-v5.robot \
  --logs "$out/penv" > "$out/penv.log" 2>&1
python3 scripts/probe-rv64-isa.py --act --robot-isa "$work/arch-git/work" \
  --jobs 4 --logs "$out/act" > "$out/act.tsv" 2> "$out/act.stderr.log"
python3 scripts/verify-rv64-virt.py "$virt_work" --logs "$out/virt" > "$out/virt.log" 2>&1
```

固定 Renode と ACT の準備は [ACT 終了通知と署名](rv64-act-equivalence.md)、公開ファームウェア素材は [README の RV64 virt 手順](../README.md)を参照。ログと終了状態は `out` に保存した。

固定 Robot 2ケースと Penv 2ケースが成功し、既存の LEA とビット命令9ケースも再確認した。`cargo test` は125件合格。ACT 242本の診断は前回と一致し、216本成功・26本失敗だった。公開 RV64 virt イメージで OpenSBI と Linux を起動し、UART から root ログインして `id -u` の `0` と `uname -m` の `riscv64` を確認した。

## 残課題

GP 相対ストア、定数比較分岐、バイト検索などの AndeStar 命令が残る。GP ロードについては、アドレス変換・PMP・非整列アクセスを含む構成で上流との差分が残る。
