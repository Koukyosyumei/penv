# 固定 Renode の NUnit Rust 移植

現在の対応件数と未移植 API は [移植進捗](renode-nunit-port-progress.md) に記録する。以下は DMA バッチ終了時の実測。

上流は `/home/ec2-user/Dev/renode` の
`c38ac3babcd38a7144b9498e3ca4c2226b76aa37`。NUnit 対象は、このリビジョンの
`tests/tests.yaml` にある6プロジェクトで、公式 VSTest アダプターの Release
discovery と NUnit XML を照合した。全87 fixture・3,413ケースの公式 ID を取得した。

| 状態 | ケース数 |
| --- | ---: |
| Rust で実測合格 | 422 |
| Rust で実測失敗 | 0 |
| 実行対象の移植済みケースで未実行 | 0 |
| API 不足で未移植 | 2,974 |
| 上流の Ignore | 14 |
| Linux で対象外の Windows 専用ケース | 3 |

対応表は22 fixture・425ケースを収録する。今回の追加は DMA 全144件と、
既存の MMIO 登録を使う SystemBus 2件。Windows 専用の3件も移植済みで、
Linux での合否集計から除外した。

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
| SystemBusTests | 27 | 21 | 5 |
| DmaEngineTests | 144 | 144 | 0 |
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
- 全87 fixture の件数と、71 fixture に残る具体的な API 不足。 [fixtures.tsv](nunit-evidence/fixtures.tsv)
- Rust テスト名との一対一対応。 [nunit_cases.tsv](../scripts/upstream/nunit_cases.tsv)
- 一括実行コマンド、プロジェクト別アセンブリ SHA-256、Rust バイナリ SHA-256、ケース別結果。 [results.json](nunit-evidence/results.json)
- Rust 一括実行の生ログ。 [rust.log](nunit-evidence/rust.log)

未移植のうち2,247件は Migrant のオブジェクトグラフとバイナリ形式などに依存する。
OptionsParser の53件には属性ベースの引数バインダーが必要。Migrant のパラメーター監査と等価性条件は別資料に記録した。 [Migrant 後続バッチ](renode-migrant-nunit.md)
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
`target/nunit-dma-verified/` に保存した。

## DMA

公式 C# 側と Rust 側の結果を、同じ144件の公式 ID で一対一に照合して全件合格を確認した。
転送元・先の3種と、読み書きそれぞれの1・2・4・8バイト幅をすべて展開した。
元の 5 KiB バッファ、16バイトの初期パターン、オフセットと32バイト転送の観測を保持した。
固定アドレスと負のストライドの3件を API の追加テストとして検証し、公式件数から除いた。
`Bus` アダプターの8バイト単位アクセスは RAM と ArrayMemory が対象。
[公式・Rust 結果照合](nunit-evidence/dma-comparison.tsv)

公式 C# 側の生ログは [official-dma.log](nunit-evidence/official-dma.log)。

公式実行は固定済みビルド checkout で次のフィルターを使用した。
TRX の VSTest 完全名と discovery の完全名を照合した。

```sh
dotnet test --no-build --configuration Release \
  src/Infrastructure/src/Emulator/Main/Tests/UnitTests/UnitTests.csproj \
  --filter 'FullyQualifiedName~Antmicro.Renode.UnitTests.DmaEngineTests' \
  --logger 'trx;LogFileName=dma.trx'
```

## Rust 回帰確認

`cargo test --locked --color never --bin penv` は870件合格、49件失敗、
3件無視。失敗した49件の完全名は前回と全件一致した。
全単体テストの生ログはローカルの `target/nunit-dma-verified/cargo-all.log` に保存した。
