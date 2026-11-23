# RV64 の生アドレスでの QuadWord 検査

固定 Renode の `tests/unit-tests/memory-assertions.robot` の2ケースを、同じ RV64 ISA、RAM、アドレス `0x08` で照合した。`verify-rv64-memory-values.py` は固定 Robot の入力を読み、既存の `--write-memory` と `--read-memory-now` で同じアクセスを行う。各比較は取得した64ビット値に対して行い、値の欠落、比較不成立、CLI 異常終了を失敗とする。

| 固定ケース | 比較 |
| --- | --- |
| Should Read Quad Word At Raw Address | 等値、大小、境界値、不等値、非ゼロの9件 |
| Should Read Quad Word Zero At Raw Address | ゼロの1件 |

## 再現

```sh
out=target/backlog-rv64-fire-upstream
ref=/var/tmp/penv-rv64-act-upstream
mkdir -p "$out"
cargo build --release
i=0
for fixture in 'Should Read Quad Word At Raw Address' 'Should Read Quad Word Zero At Raw Address'; do
  i=$((i+1))
  PATH="$ref/dotnet:$ref/robot-venv/bin:$PATH" \
    "$ref/renode/renode-test" "$ref/renode/tests/unit-tests/memory-assertions.robot" \
    --fixture "$fixture" --results-dir "$PWD/$out/memory-upstream-$i" -P 0
done
python3 scripts/verify-rv64-memory-values.py ../renode/tests/unit-tests/memory-assertions.robot \
  --logs "$out/memory-values"
```

固定 Robot と同一 Penv バイナリで両ケースとも合格した。シンボル参照のケースは残る。

## 実行後の生アドレス

固定の `Should Read Quad Word At Raw Address After` が使うストアループを、固定 Robot から抽出して ELF 化した。固定開始 PC `0x10` から実行し、`0x1fff0` の8バイト値がゼロから `0x1fff0` に変わることを確認した。固定の3秒仮想時間ウォッチは、同じ値を繰り返し格納するこのループに対し、30,000ステップでの読み出しに置き換えた。固定 Robot と同じ比較が、その期限より短い命令枠内で成立した。

## 符号付きの診断

ARM から `96e0519ed65ff010c49b70682fe21464614a73cc` として負数書き込みの実装が返った。その証拠バイナリに固定 REPL を渡すと `unsupported RV64 ISA extension "c"` で停止した。`rv64gc` に縮小すると、固定の負数書き込みと符号付き比較9件が通過する。固定 ISA と共有 CLI 修正が同じバイナリに揃うまで台帳は部分移植とする。今回の変更はテストと台帳に限定した。

```sh
out=target/backlog-rv64-signed-memory
robot=../renode/tests/unit-tests/memory-assertions.robot
ref=/var/tmp/penv-rv64-act-upstream
work=/tmp/penv-rv64-full
binary="$HOME/.local/state/penv-dispatch/evidence/integration/shared-memory-setup-b69d351/release/penv"
mkdir -p "$out"
cargo test > "$out/cargo-test.log" 2>&1
for fixture in 'Should Read Signed Quad Word At Raw Address' 'Should Read Quad Word At Raw Address After'; do
  PATH="$ref/dotnet:$ref/robot-venv/bin:$PATH" \
    "$ref/renode/renode-test" "$ref/renode/tests/unit-tests/memory-assertions.robot" \
    --fixture "$fixture" --results-dir "$PWD/$out/upstream-${fixture// /-}" -P 0
done
python3 scripts/verify-rv64-memory-values.py "$robot" --penv "$binary" --signed --isa rv64gc \
  --logs "$out/integration-diagnostic"
python3 scripts/verify-rv64-memory-values.py "$robot" \
  --after "$work/xpack-riscv-none-elf-gcc-15.2.0-1/bin/riscv-none-elf-gcc" --logs "$out/penv"
python3 scripts/verify-rv64-per-core.py target/backlog-rv64-per-core/multibus.elf --logs "$out/firmware"
```

`cargo test` は125件合格。公開 `multibus.elf` の UART とメモリ回帰4件も通過した。実行ログと終了状態は `out` に保存した。
