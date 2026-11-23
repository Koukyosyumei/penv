# RV64 の AMO オペランドトレース

固定 Renode の `tests/unit-tests/execution-tracing.robot` の AMO 2ケースを移植した。同じ `rv64imacv`、RAM、初期レジスター値で `amoadd.d.aqrl a5, a4, (a3)` を1ステップ実行した。テキストでは命令1行と実行前後のオペランド2行を比較し、バイナリは固定 Renode の `Parse Binary Trace` で1命令・3行にデコードして同じアサーションを適用した。

`--trace-amo CPU:text:PATH` は PATH 上の `llvm-mc` で命令名を取得する。`--trace-amo CPU:binary:PATH` は ReTrace v5 の PC・オペコードと、実行前後の追加レコードを出力する。ヘッダーの LLVM モデルは、記録対象の基本 A 命令をデコードできる `rv64ima` を使う。記録したメモリ値は、整列された RAM 上で命令が実際に読み書きした値。

## 再現

```sh
out=target/backlog-rv64-amo-trace
ref=/var/tmp/penv-rv64-act-upstream
work=/tmp/penv-rv64-full
mkdir -p "$out"
cargo test > "$out/cargo-test.log" 2>&1
cargo build --release > "$out/build.log" 2>&1
PATH="$ref/dotnet:$ref/robot-venv/bin:$PATH" \
  "$ref/renode/renode-test" "$ref/renode/tests/unit-tests/execution-tracing.robot" \
  --fixture 'Amo Operands' --results-dir "$PWD/$out/upstream" -P 0 > "$out/upstream.log" 2>&1
"$ref/robot-venv/bin/python" scripts/verify-rv64-amo-trace.py \
  --renode "$ref/renode" --logs "$out/penv" > "$out/penv.log" 2>&1
"$ref/robot-venv/bin/python" scripts/verify-rv64-amo-trace.py \
  --renode "$ref/renode" --logs "$out/reset" --reset-before > "$out/reset.log" 2>&1
python3 scripts/verify-rv64-pc-trace.py --logs "$out/pc" > "$out/pc.log" 2>&1
python3 scripts/probe-rv64-isa.py --act --robot-isa "$work/arch-git/work" \
  --jobs 4 --logs "$out/act" > "$out/act.tsv" 2> "$out/act.stderr.log"
python3 scripts/verify-rv64-virt.py "$virt_work" --logs "$out/virt" > "$out/virt.log" 2>&1
```

固定 Renode と ACT の準備は [ACT 終了通知と署名](rv64-act-equivalence.md)、公開ファームウェア素材は [README の RV64 virt 手順](../README.md)を参照。ケース別ログと終了状態は `out` に保存した。

固定 Robot と Penv で2件合格。ハートリセットの前にトレースを設定した場合にも両形式の記録が継続することを補足確認した。最終ビルドで `cargo test` 125件、PC トレース4件、公開 `multibus.elf` の UART・メモリ回帰4件が合格した。リセット初期化順の修正前に実行した固定 ACT は前回と同じ216本成功・26本失敗で、公開 RV64 virt は Linux の UART ログインおよびコマンド実行まで通過した。

## 残課題

AMO モードは通常の整列 RAM 上の AMO を記録する。LR/SC、AMOCAS、非整列・MMIO のアトミックアクセス、AMO 以外の命令を含む汎用 PCAndOpcode トレースは未対応。
