# 固定 Renode の NUnit Rust 移植

上流は `/home/ec2-user/Dev/renode` の
`c38ac3babcd38a7144b9498e3ca4c2226b76aa37`。NUnit 対象は、このリビジョンの
`tests/tests.yaml` にある6プロジェクトで、公式 VSTest アダプターの Release
discovery と NUnit XML を照合した。全87 fixture・3,413ケースの公式 ID を取得した。

| 状態 | ケース数 |
| --- | ---: |
| Rust で実測合格 | 276 |
| Rust で実測失敗 | 0 |
| 実行対象の移植済みケースで未実行 | 0 |
| API 不足で未移植 | 3,120 |
| 上流の Ignore | 14 |
| Linux で対象外の Windows 専用ケース | 3 |

対応表は21 fixture・279ケースを収録する。前回の54件から225件追加した。
追加分は新たな Rust ケース150件と、既存 Rust テストを公式 ID に照合した75件。
Windows 専用の3件も移植済みで、Linux での合否集計から除外した。

| Fixture | 発見 | 合格 | API 不足 |
| --- | ---: | ---: | ---: |
| ParserPrimitivesTest | 25 | 25 | 0 |
| ParserTests | 66 | 66 | 0 |
| PreLexerTests | 21 | 21 | 0 |
| UsingTests | 20 | 8 | 9 |
| TimeHandleTests | 29 | 29 | 0 |
| TimeSourceTests | 17 | 11 | 3 |
| TimeIntervalTests | 3 | 3 | 0 |
| BitHelperTests | 10 | 10 | 0 |
| CircularBufferTests | 15 | 15 | 0 |
| RangeTests | 20 | 20 | 0 |
| SystemBusTests | 27 | 19 | 7 |
| GPIOTests | 4 | 4 | 0 |
| GPIODetachingTests | 10 | 3 | 7 |
| MemoryTests | 2 | 1 | 1 |
| CRCTests | 15 | 15 | 0 |
| MiscTests | 1 | 1 | 0 |
| FT5336Test | 4 | 4 | 0 |
| STM32H7_RCC_TEST | 7 | 7 | 0 |
| STMCANTest | 3 | 1 | 0 |
| LimitTimerTests | 9 | 9 | 0 |
| ComparingTimerTests | 5 | 4 | 1 |

SystemBus の `ShouldReadBytes` は
`isFirstMemory` と `isSecondMemory` の4組をそれぞれ実行した。
ParserPrimitivesTest の `[Values]` も値ごとに対応する。
TimeInterval は penv が使う `u64` ナノ秒と標準の checked 演算で検証した。
タイマーの周期とカウンター表現の差は fixture 側で換算した。
TimeSource のブロッキング検証では、固定時間のスリープをチャネル同期に置き換えた。
上流が `#if DEBUG` に含める TimeHandle の13件は、今回の Release discovery
の対象外。

## 証跡

- 6プロジェクト全件の公式 ID、NUnit エンジン ID、パラメーター、状態。 [inventory.jsonl.gz](nunit-evidence/inventory.jsonl.gz)
- 全87 fixture の件数と、72 fixture に残る具体的な API 不足。 [fixtures.tsv](nunit-evidence/fixtures.tsv)
- Rust テスト名との一対一対応。 [nunit_cases.tsv](../scripts/upstream/nunit_cases.tsv)
- 一括実行コマンド、プロジェクト別アセンブリ SHA-256、Rust バイナリ SHA-256、ケース別結果。 [results.json](nunit-evidence/results.json)
- Rust 一括実行の生ログ。 [rust.log](nunit-evidence/rust.log)

未移植のうち2,247件は Migrant のオブジェクトグラフとバイナリ形式などに依存する。
OptionsParser の53件には属性ベースの引数バインダー、DmaEngine の144件には汎用転送 API が必要。
SystemBus、REPL の CreationDriver、各未実装モデルなどの不足は fixture ごとに記録した。

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

固定リビジョンで再取得した discovery XML と VSTest 生ログは、実行先ディレクトリの
`nunit-1/` から `nunit-6/` に出力される。今回分はローカルの
`target/nunit-complete-inventory/` に保存した。

## Rust 回帰確認

`cargo test --locked --color never --bin penv` は721件合格、49件失敗、
3件無視。前回の全単体テストで失敗していた58件のうち、SystemBus ケース9件が解消した。
今回失敗した49件は前回も失敗していた。全単体テストの生ログはローカルの
`target/nunit-resume-cargo-all.log` に保存した。
