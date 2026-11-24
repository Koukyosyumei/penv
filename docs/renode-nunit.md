# 固定 Renode の NUnit Rust 移植

上流は `/home/ec2-user/Dev/renode` の
`c38ac3babcd38a7144b9498e3ca4c2226b76aa37`。
Infrastructure は gitlink の `09ac7c563095c5410258ecc1b1a6b841a013db85`。
別のビルド済み checkout から、対象3プロジェクトの NUnit XML と VSTest 一覧を再取得した。
3プロジェクトの発見総数は1,028ケース。そのうち次の5 fixture の全54ケースを
公式 ID と Rust の完全一致テスト名で一対一に照合した。

| Fixture | 新規移植 | 既存移植を確認 | 実測合格 | 失敗 |
| --- | ---: | ---: | ---: | ---: |
| ParserPrimitivesTest | 20 | 5 | 25 | 0 |
| MiscTests | 1 | 0 | 1 | 0 |
| CRCTests | 0 | 15 | 15 | 0 |
| FT5336Test | 0 | 4 | 4 | 0 |
| LimitTimerTests | 0 | 9 | 9 | 0 |
| **合計** | **21** | **33** | **54** | **0** |

対応済みケースの未実行は0件。ParserPrimitivesTest の `[Values]` は各値を独立した
テストとして実行した。MiscTests の全17アサーションは上流と同じ1ケースとして数えた。
FT5336 はレジスタ選択と応答スライスで I²C の書き込みと4バイト読み出しに対応する。
LimitTimer は既存の STM32H7 タイマーを使用し、周期とカウンターの表現差を
fixture 側で換算する。方向変更の終端イベントはゼロ到達時の比較割り込みで観測する。

公式完全名と Rust テスト名の対応は [nunit_cases.tsv](../scripts/upstream/nunit_cases.tsv)。
公式 NUnit エンジン ID、パラメーター、ケース別結果、実行コマンド、
バイナリの SHA-256 は [results.json](nunit-evidence/results.json)。
一括実行の生ログは [rust.log](nunit-evidence/rust.log)。
ローカルの `target/nunit-verified/nunit-{1,2,3}/` に各プロジェクトの
`discovery.xml` と `discovery.log` がある。

## 再実行

固定 SHA のビルド済み Release checkout と .NET SDK を指定する。
チェックアウトとビルドの前提は [Renode upstream CI](renode-upstream-ci.md) に記載。

```sh
python3 scripts/upstream/run_nunit.py \
  --renode /home/ec2-user/Dev/renode \
  --built-renode /home/ec2-user/Dev/penv-renode-ci/target/renode-discovery \
  --dotnet /home/ec2-user/Dev/penv-renode-ci/target/renode-dotnet/dotnet \
  --out target/nunit-recheck
```

## Rust 回帰確認

`cargo test --locked --color never` のバイナリ単体テストは565件合格、58件失敗。
失敗した58件の完全名は、先行ブランチの `target/upstream-cargo-test.log` と全件一致した。
全体ログはローカルの `target/nunit-verified/cargo-all.log` に保存した。
単体テストで終了した後、統合テストを別コマンドで実行した。
`cli` は42件合格・11件無視、`rv32_vector_slice` は7件合格。失敗は0件。
ログはローカルの `target/nunit-verified/cargo-integration.log` に保存した。
