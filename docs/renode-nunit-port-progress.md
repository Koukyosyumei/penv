# 固定 Renode NUnit の移植進捗

対象は固定リビジョン `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` の
公式 Release discovery 6プロジェクト、87 fixture、3,413ケース。

| 状態 | ケース数 |
| --- | ---: |
| Linux 対象で Rust 移植・公式 ID 対応済み | 1,062 |
| 移植・対応済みの Windows 専用ケース | 3 |
| 移植待ち | 2,334 |
| 上流 Ignore | 14 |

進捗表は公式インベントリと対応表から生成した。移植待ち 37 fixture の
ケース数と不足 API を各行に記録した。合否を示す列は含めていない。
[現在の fixture 別進捗](nunit-evidence/port-progress.tsv)

前回の追加は52件。Migrant の TypeDescriptorComparisonTests が11件、
SystemBus の幅変換が1件、レジスタバイトのアクセス追跡が11件、
アクセス条件の DNF 変換と状態マスクが15件、MultiTree と MultiValueDictionary が各7件。
各 fixture を対象にした Rust 実行で、新規対応52件すべてが合格した。
Migrant 型記述子の11件は固定 C# 側でも合格した。
[公式 ID 別の Migrant 比較](nunit-evidence/migrant-descriptor-comparison.tsv)
[前回の Rust 実行結果](nunit-evidence/targeted-52-rust.tsv)

前回の追加は13件。Migrant の型スタンプ無効・リフレクション設定での
SerializationTests が9件、既存の AddressRange を使う SymbolTests が4件。
対象モジュールの Rust 実行では両方とも合格した。
[公式 ID 別の実行結果](nunit-evidence/targeted-object-stream-symbol.tsv) · [対象実行ログ](nunit-evidence/targeted-object-stream-symbol.log)

前回の追加は22件。Migrant の型スタンプ付き SerializationTests が9件、
StampingTests が4件、OpenStreamSerializationTests が5件、ObjectReaderWriterTests が1件。
既存の範囲 API を使った SymbolLookup の DeduplicationTests が3件。
新規22件は対象に絞った Rust 実行で合格した。
[公式 ID 別の結果](nunit-evidence/targeted-stamps-symbol-dedup.tsv) · [Rust の対象実行ログ](nunit-evidence/targeted-stamps-symbol-dedup.log)

前回の追加は32件。Migrant の SurrogateTests が12件、配列の SerializationTests が6件、
既存の範囲検索を使う SymbolLookupTests が14件。対象を絞った Rust 実行で
新規32件すべてが合格した。対応するサロゲート12件は固定 C# 側でも合格した。
[公式 ID 別の結果](nunit-evidence/targeted-surrogates-arrays-symbols.tsv) · [対象実行ログ](nunit-evidence/targeted-surrogates-arrays-symbols.log)

前回の追加は28件。Migrant の SurrogateTests が22件、既存の範囲検索を使う
SymbolLookupTests が6件。対象を絞った Rust 実行で新規28件すべてが合格した。
[公式 ID 別の結果](nunit-evidence/targeted-hierarchy-symbol-lookup.tsv) · [対象実行ログ](nunit-evidence/targeted-hierarchy-symbol-lookup.log)

前回の追加は5件。Migrant の OpenStreamSerializationTests の弱参照設定2件と、
SymbolLookupTests の残り3件を移植した。SymbolLookupTests の Rust 対象実行は
公式 ID 対応済み23件すべてが合格した。
[公式 ID 別の結果](nunit-evidence/targeted-weak-stream-symbol.tsv) · [対象実行ログ](nunit-evidence/targeted-weak-stream-symbol.log)

前回の追加は24件。SimpleCacheTests 全10件と、Migrant の多次元配列6件、
型付きフィールドと文字列タプル8件を移植した。対象を絞った Rust 実行で
新規24件すべてが合格した。
[公式 ID 別の結果](nunit-evidence/targeted-cache-rectangular-records.tsv) · [対象実行ログ](nunit-evidence/targeted-cache-rectangular-records.log)

前回の追加は25件。MemoryTests の同時セグメント割り当て1件と、
Migrant の nullable と連続ストリーム10件、リストとボックス化された値14件を移植した。
対象を絞った Rust 実行で新規25件すべてが合格した。
[公式 ID 別の結果](nunit-evidence/targeted-memory-nullable-lists.tsv) · [対象実行ログ](nunit-evidence/targeted-memory-nullable-lists.log)

前回の追加は35件。Migrant のコレクション20件、OptionsParser の ValidationTests
全4件と ParserTests 11件。対象を絞った Rust 実行で新規35件すべてが合格した。
[公式 ID 別の結果](nunit-evidence/targeted-collections-options.tsv) · [対象実行ログ](nunit-evidence/targeted-collections-options.log)

前回の追加は48件。OptionsParser の ParserTests の残り38件と、
Migrant のバイト配列・空コレクション10件を移植した。対象を絞った Rust 実行で
新規48件すべてが合格した。OptionsParser の対象実行は既移植分を含む53件が合格した。
Rust のオプション定義には明示的なスキーマを使う。
[公式 ID 別の結果](nunit-evidence/targeted-parser-empty-arrays.tsv) · [対象実行ログ](nunit-evidence/targeted-parser-empty-arrays.log)

前回の追加は10件。TimeTests の4件、WeakMultiTableTest の実行対象3件、
PseudorandomNumberGeneratorTests の3件を移植した。対象を絞った Rust 実行で全件合格した。
仮想時計はコールバックの時点で観測できるカウンターを計算する。乱数のスナップショットは
インスタンスごとのシードとスレッド別の状態を Rust の形式で保存する。
[公式 ID 別の結果](nunit-evidence/targeted-clock-weak-table-random.tsv) · [対象実行ログ](nunit-evidence/targeted-clock-weak-table-random.log)

前回の追加は26件。Migrant の列挙型、スタック、継承と値型の24件、
SerializableMappedSegmentTests の全2件。対象を絞った Rust 実行で全件合格した。
セグメントはバッファを Rust のグラフ形式に保存し、未割り当て状態と割り当て済み状態を復元する。
[公式 ID 別の結果](nunit-evidence/targeted-enums-structs-segments.tsv) · [対象実行ログ](nunit-evidence/targeted-enums-structs-segments.log)

前回の追加は12件。Migrant のカスタムキーと異なるルート型の6件、GUIDの4件、
SHT21の実行対象2件を移植した。対象を絞った Rust 実行で全件合格した。
SHT21 はコマンドパケットをモデルに直接渡し、固定シードで測定値を再現できるようにした。
[公式 ID 別の結果](nunit-evidence/targeted-guid-collections-sht21.tsv) · [対象実行ログ](nunit-evidence/targeted-guid-collections-sht21.log)

前回の追加は4件。BMA180 の初期化と測定2件、多倍長整数の型スタンプ有無2件を
移植した。対象を絞った Rust 実行で全件合格した。BMA180 はコマンドをモデルに
直接渡し、固定シードで測定値を再現する。
[公式 ID 別の結果](nunit-evidence/targeted-bma180-big-integer.tsv) · [対象実行ログ](nunit-evidence/targeted-bma180-big-integer.log)

前回の追加は17件。Migrant の一時オブジェクト・フィールドとネイティブ資源の16件、
ComparingTimerTests の比較値上限1件。対象を絞った Rust 実行で全件合格した。
比較値上限は汎用タイマーの型付きエラーで検証し、既移植の4件も対象実行で合格した。
[公式 ID 別の結果](nunit-evidence/targeted-transients-native-compare.tsv) · [対象実行ログ](nunit-evidence/targeted-transients-native-compare.log)

前回の追加は8件。Migrant の入れ子の decimal とネイティブ資源の経路4件、
サロゲート登録のライフサイクル4件。対象を絞った Rust 実行で全件合格した。
[公式 ID 別の結果](nunit-evidence/targeted-decimal-path-surrogates.tsv) · [対象実行ログ](nunit-evidence/targeted-decimal-path-surrogates.log)

前回の追加は14件。Migrant のカスタムペイロード8件、既定設定の正規表現2件、
ジェネリック配列とインターフェースのリスト4件。対象を絞った Rust 実行で全件合格した。
[公式 ID 別の結果](nunit-evidence/targeted-custom-regex-generics.tsv) · [対象実行ログ](nunit-evidence/targeted-custom-regex-generics.log)

前回の追加は10件。Migrant のボックス化された整数のサロゲート2件、
コンストラクター指定のフィールド6件、復元後のフック2件を移植した。
対象を絞った Rust 実行で全件合格した。
[公式 ID 別の結果](nunit-evidence/targeted-boxed-constructors-late-hook.tsv) · [対象実行ログ](nunit-evidence/targeted-boxed-constructors-late-hook.log)

前回の追加は16件。Migrant の HooksTests を反射読み書き・型スタンプ有無の
2設定で8メソッド移植した。対象を絞った Rust 実行で全件合格した。
[公式 ID 別の結果](nunit-evidence/targeted-migrant-read-global-hooks.tsv) · [対象実行ログ](nunit-evidence/targeted-migrant-read-global-hooks.log)

前回の追加は24件。Migrant の HooksTests で書き込み前後の変更、失敗時の後処理、
呼び出し順と仮想メソッドのケースを移植した。既移植分を含む反射読み書き40件が
固定 C# と対象 Rust 実行の両方で合格した。
[公式 ID 別の結果](nunit-evidence/targeted-migrant-hooks-reflection.tsv) · [対象実行ログ](nunit-evidence/targeted-migrant-hooks-reflection.log)

前回の追加は5件。Migrant の XML サロゲート2件、PeripheralsGroupTests 全3件。
周辺機器グループは SystemBus のネイティブアクセス幅変換対応の登録経路で検証した。
対象を絞った Rust 実行で新規5件すべてが合格した。
[公式 ID 別の結果](nunit-evidence/targeted-xml-peripheral-groups.tsv) · [対象実行ログ](nunit-evidence/targeted-xml-peripheral-groups.log)

前回の追加は12件。NullRegistrationPointPeripheralContainerTests 全10件と、
SystemBusTests の複数機能・パラメーター付き登録2件。ネイティブ周辺機器の登録ハンドルで
子の同一性を表し、アドレス範囲ごとにアクセス幅とコールバックを選択する。
対象 Rust 実行では関連する既移植分を含む18件が合格した。
[公式 ID 別の結果](nunit-evidence/targeted-child-and-native-ports.tsv) · [対象実行ログ](nunit-evidence/targeted-child-and-native-ports.log)

前回の追加は4件。SystemBus の一時停止・再開と非クラスタ CPU の登録点検証2件、
TimeSource の即時進行と切断済みスレーブ2件。対象 Rust 実行で新規4件が合格した。
SystemBus の実行対象26件の公式 ID 対応が揃った。上流 Ignore は1件。
CPU のテストにはバス登録用の識別子とアーキテクチャ名を使う。
[公式 ID 別の結果](nunit-evidence/targeted-bus-lifecycle-paced-time.tsv) · [対象実行ログ](nunit-evidence/targeted-bus-lifecycle-paced-time.log)

前回の追加は3件。TimeSourceTests の外部メソッド呼び出し記録・再生1件と、
Migrant のジェネリックサロゲート2件。TimeSourceTests の実行対象14件の公式 ID 対応が
揃った。記録・再生では同じマシンティックで呼び出しを再生し、固定シードの待機時間を
使ってテストのタイムラインを進める。Rust ではディスパッチャーを明示的に進行させる。
[公式 ID 別の結果](nunit-evidence/targeted-event-replay-generic-surrogate.tsv) · [対象実行ログ](nunit-evidence/targeted-event-replay-generic-surrogate.log)

Migrant の未移植分には、生成読み書き、組み込みサロゲート、
シリアライズ時のフック、型変更を伴うオブジェクトの復元が含まれる。
全対象の移植と公式 ID 対応が揃ってから一括実行する。

```sh
python3 scripts/upstream/nunit_port_progress.py \
  docs/nunit-evidence/inventory.jsonl.gz docs/nunit-evidence/port-progress.tsv
```

## EFM32 I²C と SymbolTests

EFM32 I²C のレジスタ初期化・制御・割り込み3件と、SymbolTests の名前復元1件を追加した。4件すべて対象実行で合格した。EFM32 I²C の残り1件には BMC050 のアドレス登録と START・STOP によるデータ交換が必要。名前復元には GNU c++filt を使用し、実行できない環境では元の名前を保持する。

- [公式 ID ごとの結果](nunit-evidence/targeted-efm32-symbols.tsv)
- [対象実行ログ](nunit-evidence/targeted-efm32-symbols.log)

## LPC43xx GPIO と Cadence TTC

LPC43xx GPIO の5件は上流と同じバス登録後に周辺機器のレジスタを直接操作する。Cadence TTC の6件はカウンターのリセット、3系統の独立したプリスケーラ、増減両方向のオーバーフロー・インターバルと IRQ を検証する。時間は TTC に直接進め、比較一致、外部クロック、イベントタイマーは今回の実装範囲外。

- [GPIO の公式 ID ごとの結果](nunit-evidence/targeted-lpc43xx_gpio.tsv)と[実行ログ](nunit-evidence/targeted-lpc43xx_gpio.log)
- [TTC の公式 ID ごとの結果](nunit-evidence/targeted-cadence_ttc.tsv)と[実行ログ](nunit-evidence/targeted-cadence_ttc.log)

## Cadence UART

割り込みの読み出しクリア・書き込みクリア、TX 無効時の FIFO 閾値とオーバーフロー、RX オーバーフロー設定の6件が対象実行で合格した。実装の送信は上流の既定値と同じ即時送信。遅延送信やボーレート設定はこの実装の対象外。

- [公式 ID ごとの結果](nunit-evidence/targeted-cadence_uart.tsv)と[実行ログ](nunit-evidence/targeted-cadence_uart.log)

## BMC050

公式 InitTest の97アサーションを、上流と同じパケット順で検証した。帯域幅レジスタの初期値は公式アサーションの 0x1f を採用した。固定 C# 本体の初期化式は 0x08 | 0x10 で 0x18 になるため、この点は公式テストと C# 本体に差がある。ReadDataTest は計測値生成と複数軸の応答フレームが必要。

- [公式 ID ごとの結果](nunit-evidence/targeted-bmc050.tsv)と[実行ログ](nunit-evidence/targeted-bmc050.log)
