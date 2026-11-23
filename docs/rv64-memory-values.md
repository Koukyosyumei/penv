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

固定 Robot と同一 Penv バイナリで両ケースとも合格した。符号付きケースの負数入力は現在の共有 CLI では拒否されるため、ARM 担当への `rv64-memory-values-73d7174.md` の回答待ち。シンボル参照と、実行中の値を待つケースも今回の判定には含めていない。
