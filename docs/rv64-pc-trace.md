# RV64 の PC 実行トレース

固定 Renode の `tests/unit-tests/execution-tracing.robot` から「Should Dump 64-bit PCs」で始まる4ケースを移植した。CPU は `rv64imacv`。共通 RAM と CPU 専用 RAM の両方で同じ3命令を実行し、PC が `0x2000000000`、`0x2000000004`、`0x2000000006` の順に出力されることを確認した。テキストは3行、バイナリは `ReTrace` バージョン5の10バイトのヘッダーと9バイトのレコード3件で、固定 Robot の期待する37バイトに一致した。

`--trace-pc CPU:text:PATH` または `--trace-pc CPU:binary:PATH` は、指定ハートが完了した命令の実行前 PC を記録する。CLI では出力パスに新規ファイルを指定する。別名のない CPU 専用 RAM を扱うため、型付きバス登録の後にインラインのプロパティブロックが続く REPL 構文と、グローバル登録を伴わない RAM バッキングを追加した。CPU を指定しない読み出しからその RAM が見えないことも確認した。

## 再現

```sh
out=target/backlog-rv64-pc-trace
ref=/var/tmp/penv-rv64-act-upstream
work=/tmp/penv-rv64-full
elf=target/backlog-rv64-per-core/multibus.elf
mkdir -p "$out"
cargo test > "$out/cargo-test.log" 2>&1
cargo build --release > "$out/build.log" 2>&1
PATH="$ref/dotnet:$ref/robot-venv/bin:$PATH" \
  "$ref/renode/renode-test" "$ref/renode/tests/unit-tests/execution-tracing.robot" \
  --fixture 'Should Dump 64-bit PCs' --results-dir "$PWD/$out/upstream" -P 0 > "$out/upstream.log" 2>&1
python3 scripts/verify-rv64-pc-trace.py --logs "$out/penv" > "$out/penv.log" 2>&1
python3 scripts/verify-rv64-per-core.py "$elf" --logs "$out/firmware" > "$out/firmware.log" 2>&1
python3 scripts/verify-rv64-ram-move.py --logs "$out/moves" > "$out/moves.log" 2>&1
python3 scripts/probe-rv64-isa.py --act --robot-isa "$work/arch-git/work" \
  --jobs 4 --logs "$out/act" > "$out/act.tsv" 2> "$out/act.stderr.log"
```

固定 Renode と ACT の準備は [ACT 終了通知と署名](rv64-act-equivalence.md)、公開 ELF の入手とハッシュは [ハート別 RAM の記録](rv64-per-core.md) を参照。固定 Robot と Penv の4件、RAM 移動、公開 ELF の UART・メモリ回帰4件が合格し、`cargo test` は125件合格。固定 ACT の診断は前回と同じ216本成功・26本失敗だった。ケース別ログと終了状態は `out` に保存した。

## 残課題

今回のトレースは PC 形式で完了した命令を扱う。トラップ時の出力の等価性、オペコード形式、AMO オペランド等の追加レコードは未検証または未実装。
