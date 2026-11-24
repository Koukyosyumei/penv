# 固定 Renode NUnit の移植進捗

対象は固定リビジョン `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` の
公式 Release discovery 6プロジェクト、87 fixture、3,413ケース。

| 状態 | ケース数 |
| --- | ---: |
| Linux 対象で Rust 移植・公式 ID 対応済み | 3,384 |
| 移植・対応済みの Windows 専用ケース | 3 |
| 移植待ち | 11 |
| 上流 Ignore | 14 |
| 上流 Inconclusive | 1 |

進捗表は公式インベントリと対応表から生成した。移植待ち 5 fixture の
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

EFM32 I²C のレジスタ初期化・制御・割り込み3件と、SymbolTests の名前復元1件を追加した。4件すべて対象実行で合格した。EFM32 I²C の残り1件は下記のバッチで移植した。名前復元には GNU c++filt を使用し、実行できない環境では元の名前を保持する。

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

公式 InitTest の97アサーションを、上流と同じパケット順で検証した。帯域幅レジスタの初期値は公式アサーションの 0x1f を採用した。固定 C# 本体の初期化式は 0x08 | 0x10 で 0x18 になるため、この点は公式テストと C# 本体に差がある。ReadDataTest は上流の単軸・複数軸の読み出しをそれぞれ5回検証する。計測式と姿勢の順序を維持し、乱数列は Rust 側で固定シードから生成する。

- [公式 ID ごとの結果](nunit-evidence/targeted-bmc050.tsv)と[実行ログ](nunit-evidence/targeted-bmc050.log)

## EFM32 I²C のアドレス切り替え

DualI2CAddressBMC050Test は同じ BMC050 を 0x18 と 0x10 に登録し、再 START を経て読み出したチップ ID、割り込み、ポーリング回数を検証する。モデルは同期的にパケットを転送する。上流の Ignore 2件はインベントリに残している。

- [公式 ID ごとの結果](nunit-evidence/targeted-efm32-dual-address.tsv)と[実行ログ](nunit-evidence/targeted-efm32-dual-address.log)

## Registers enum パーサー

公式の6入力を使い、レジスタ名・幅・値、条件付き行の有無、インデックス付きグループ、コメントと空行を確認した。パーサーは fixture が使う C enum の構文を扱う。

- [公式 ID ごとの結果](nunit-evidence/targeted-register_enum.tsv)と[実行ログ](nunit-evidence/targeted-register_enum.log)

## Migrant BlockingCollection

読み書きとも反射経路で、XML サポートを無効にした6件を移植した。型スタンプ有無それぞれで整数、null を含むオブジェクト列、文字列の多重集合の一致を確認した。コレクションを項目列として復元し、同期機構は受信側で初期化する。残り42件の内訳は反射経路で XML 有効の6件と、生成経路が必要な36件。

- [公式 ID ごとの結果](nunit-evidence/targeted-migrant-blocking-collection.tsv)と[実行ログ](nunit-evidence/targeted-migrant-blocking-collection.log)

## Migrant のサロゲート復元順と PacketTests

SurrogateTests の2ケースは、型スタンプ有無の両方で、同一参照を共有する入れ子リストと復元結果のカウンター値 3・2 を検証した。Rust 独自のオブジェクトストリームでは、復元型の情報を追加レコード種別で保持する。

PacketTests の4ケースは、固定上流のレイアウト長、ビットフィールド、入れ子、バイト順を検証した。Rust 側では C# の属性に対応する静的レイアウトを明示する。

- [サロゲートの公式 ID](nunit-evidence/targeted-migrant-surrogate-order.tsv)と[実行ログ](nunit-evidence/targeted-migrant-surrogate-order.log)
- [Packet の公式 ID](nunit-evidence/targeted-packet-layout.tsv)と[実行ログ](nunit-evidence/targeted-packet-layout.log)

## PacketTests の残り14件

静的レイアウトのエンコード、開始オフセット、配列要素幅、パディング、アラインメントを検証した。続いて先行フィールドで決まる条件付きフィールド、実行時の長さとオフセット、入れ子、読み取り専用プロパティを移植し、公式 discovery の18件すべてが対象実行で合格した。C# の公開メソッド TestDecodeInto には NUnit 属性がなく、公式インベントリに含まれない。

- [エンコード等の公式 ID](nunit-evidence/targeted-packet-encode.tsv)と[実行ログ](nunit-evidence/targeted-packet-encode.log)
- [条件付きフィールド等の公式 ID](nunit-evidence/targeted-packet-conditions.tsv)と[18件の実行ログ](nunit-evidence/targeted-packet-conditions.log)

## Monitor TokenizerTests

公式18件の入力、型、値、未消費文字数を検証した。専用の型付きトークナイザーで再現し、既存のモニターコマンドの引数解析は従来どおりの経路で動作する。

- [公式 ID](nunit-evidence/targeted-monitor_tokenizer.tsv)と[実行ログ](nunit-evidence/targeted-monitor_tokenizer.log)

## MachineTests のバス登録と名前検索

元の登録範囲を使い、重複名、空文字と null、1台または2台のパス検索を検証した。残りの管理スレッド1件は、マシンの仮想クロックへ同期区間を明示的に渡し、開始・停止前後のコールバック回数を確認した。

- [公式 ID](nunit-evidence/targeted-machine-registry.tsv)と[実行ログ](nunit-evidence/targeted-machine-registry.log)
- [管理スレッドの公式 ID](nunit-evidence/targeted-machine-managed.tsv)と[実行ログ](nunit-evidence/targeted-machine-managed.log)

## EFR32 I²C

元のベースアドレスと周辺機器アドレスで、1・2バイトの送受信、FIFO の非破壊参照、割り込み、BMP180 のリセット値を検証し、公式15件が合格した。コントローラーはテストと同じようにバスへ直接登録して使い、BMP180 は既存のレジスタモデルを再利用した。

- [公式 ID](nunit-evidence/targeted-efr32_i2c.tsv)と[実行ログ](nunit-evidence/targeted-efr32_i2c.log)

## ElfMergeTests

公式19件のシンボル、照会アドレス、期待区間を移植した。ゼロ長シンボルによる区間分割と、切り詰め後に同じ範囲となるシンボルの優先順位を修正した。対象19件と既存の SymbolLookupTests 23件が合格した。

- [公式 ID](nunit-evidence/targeted-elf_merge.tsv)と[実行ログ](nunit-evidence/targeted-elf_merge.log)

## RegistersTests のフィールド定義と全幅アクセス

9件を移植した。負の幅、8・16・32・64 ビットの末尾境界、重複、値の範囲外、全幅読み書き、書き込みコールバックを検証した。列挙型と数値は同じビットフィールド API を介して扱い、共有セットアップは移植したケースで使うフィールドに絞った。残り25件は下記のバッチで移植した。

- [公式 ID](nunit-evidence/targeted-register-fields.tsv)と[実行ログ](nunit-evidence/targeted-register-fields.log)

## RegistersTests のアクセスモードとコールバック

共有レジスタの全21フィールドを定義し、アクセスモードとリセットの20件、読み出し・書き込み・変更の通知と値プロバイダーの5件を追加した。列挙型の未定義値はビット値として保持する。公式34件すべてが対象実行で合格した。

- [アクセスモードの公式 ID](nunit-evidence/targeted-register-modes.tsv)と[実行ログ](nunit-evidence/targeted-register-modes.log)
- [コールバックの公式 ID](nunit-evidence/targeted-register-callbacks.tsv)と[34件の実行ログ](nunit-evidence/targeted-register-callbacks.log)

## SVDParserTests

ファイルと XML の入力検証18件に続き、レジスタ読み出し、権限、バイト順、未整列アクセス、設定継承と derivedFrom の20件を移植した。公式38件すべてが対象実行で合格した。XML の読み込みには roxmltree を使用し、読み出しは SVD API に直接渡す。バスのフォールバックへの組み込み、書き込みログ、クラスター自体の派生は未実装。

- [入力検証の公式 ID](nunit-evidence/targeted-svd-validation.tsv)と[実行ログ](nunit-evidence/targeted-svd-validation.log)
- [読み出しと継承の公式 ID](nunit-evidence/targeted-svd-reads.tsv)と[38件の実行ログ](nunit-evidence/targeted-svd-reads.log)

## OpenTitan AES

公式9件を移植した。固定上流のキー共有とデータベクトルを使い、ECB の自動・手動動作、復号、同一キーの再利用、入力追跡とランダム化を検証した。キー長の処理は上流の AesManaged と同様、設定値を検証してから全キーバッファを使う。乱数シードは Rust テストで固定した。アラート線や ECB 以外のモードはこのモデルの検証範囲外。

- [公式 ID](nunit-evidence/targeted-opentitan_aes.tsv)と[実行ログ](nunit-evidence/targeted-opentitan_aes.log)

## Monitor PythonCommands

対話モニターの組み込みコマンドとして next_value、sleep、echo、dump、dump_file を実装し、公式5件が合格した。上流の Python コマンドの動作を Rust で再現する。メモリ試験はマシンを Rust API で作成して元のアドレスに RAM を登録し、コマンドは対話モニターと同じディスパッチを通る。

- [基本コマンドの公式 ID](nunit-evidence/targeted-python-monitor-builtins.tsv)と[実行ログ](nunit-evidence/targeted-python-monitor-builtins.log)
- [メモリ出力の公式 ID](nunit-evidence/targeted-python-monitor-dump.tsv)と[5件の実行ログ](nunit-evidence/targeted-python-monitor-dump.log)

## OpenTitan I²C

公式18件のホスト転送、ターゲット側の取得・送信 FIFO、ウォーターマーク、オーバーフロー、モード切り替えを移植した。固定上流と同じアドレスに既存の BMP180 と PAC1934 を登録した。PAC1934 は識別レジスタを実装し、計測チャネルは未実装。I²C コントローラーは同期的に転送し、タイミング制御とアラート線は未実装。

- [公式 ID](nunit-evidence/targeted-opentitan_i2c.tsv)と[実行ログ](nunit-evidence/targeted-opentitan_i2c.log)

## MultiCPUTests

10個の mock CPU のスロットと、1〜4個の CPU から1秒間に行う共有周辺機器読み出しを検証した。外部 CPU はスレッドで動作し、読み出し要求は同じマシンのバス上で直列に実行する。外部 CPU の登録は命令を実行する既存の hart 登録とは独立している。

- [公式 ID](nunit-evidence/targeted-multi-cpu.tsv)と[実行ログ](nunit-evidence/targeted-multi-cpu.log)

## Migrant BlockingCollection の XML 有効設定

XML 有効・型スタンプ有無の6件を追加した。反射シリアライザーで XML サロゲートの許可を設定し、既存の XML 型ラウンドトリップでも同じ設定を使った。null ケースの整数はオブジェクト参照として格納する。反射経路は12件すべて合格した。生成経路の36件は後続のバッチで移植した。

- [公式 ID](nunit-evidence/targeted-migrant-blocking-xml.tsv)と[実行ログ](nunit-evidence/targeted-migrant-blocking-xml.log)

## Migrant StampingTests の生成経路

Rust マクロで型ごとの専用フィールドコーデックを生成し、読み出しと書き込みで独立に選択できるようにした。時刻のミリ秒値と自己参照を持つオブジェクトを使い、スタンプ設定が一致する4通りと不一致の組み合わせを含む公式16件すべてを確認した。コーデックは Rust のグラフ形式を使う。

- [追加12件の公式 ID](nunit-evidence/targeted-migrant-stamping-generated.tsv)と[実行ログ](nunit-evidence/targeted-migrant-stamping-generated.log)

## Migrant BuiltinSurrogatesTests の生成経路

正規表現と XML サロゲートに専用コーデックを追加した。読み出しのみ・書き込みのみ・両方の生成経路を、型スタンプの有無それぞれで実行し、公式16件すべてが合格した。

- [追加12件の公式 ID](nunit-evidence/targeted-migrant-builtin-generated.tsv)と[実行ログ](nunit-evidence/targeted-migrant-builtin-generated.log)

## Migrant BlockingCollection の生成経路

コレクションのレコードと boxed Int32 に専用コーデックを使い、配列と文字列のストリーム処理は共通経路を再利用した。生成読み書き、XML サポート、型スタンプの16設定で、整数、null を含むオブジェクト、文字列の多重集合を検証した。公式48件すべてが対象実行で合格した。

- [追加36件の公式 ID](nunit-evidence/targeted-migrant-blocking-generated.tsv)と[48件の実行ログ](nunit-evidence/targeted-migrant-blocking-generated.log)

## Migrant OpenStreamSerializationTests の生成経路

生成読み書きの3組み合わせで、参照を保持しない場合、強参照、弱参照、バッファなしの連続復元を検証した。整数と浮動小数点のタプル、および空のオブジェクトには型別コーデックを使う。追加の公式21件が対象実行で合格した。

- [公式 ID](nunit-evidence/targeted-migrant-open-generated.tsv)と[実行ログ](nunit-evidence/targeted-migrant-open-generated.log)

## Migrant HooksTests の生成経路

型別コーデックでレコードを読み書きし、フックの実行順序と例外時の後処理には既存のフック処理を使った。生成する側の組み合わせと型スタンプの有無を維持して120件を追加し、公式160件すべてが対象実行で合格した。

- [追加120件の公式 ID](nunit-evidence/targeted-migrant-hooks-generated.tsv)と[160件の実行ログ](nunit-evidence/targeted-migrant-hooks-generated.log)

## Migrant SurrogateTests の生成経路

型別の読み書きコーデックを選び、既存のサロゲート処理で型階層・共有参照・自己参照を検証した。生成書き込みでは閉じたジェネリック型ごとに変換規則をキャッシュし、int と float の2エントリが追加される公式アサーションを維持した。追加132件を含む公式176件が対象実行で合格した。

- [公式 ID](nunit-evidence/targeted-migrant-surrogate-generated.tsv)と[176件の実行ログ](nunit-evidence/targeted-migrant-surrogate-generated.log)

## Migrant SerializationTests のグラフ生成経路

8メソッドについて、コレクションをユーザーオブジェクトとして扱わない設定の生成読み書き3組み合わせと型スタンプ有無を移植した。共有参照、自己参照、循環、null、読み取り専用フィールド、45,000段のグラフを検証し、追加48件が合格した。

- [公式 ID](nunit-evidence/targeted-migrant-serialization-graph.tsv)と[実行ログ](nunit-evidence/targeted-migrant-serialization-graph.log)

## Migrant SerializationTests の列挙型・構造体・スタック

上流の列挙値とフラグ、nullable 列挙型、キー値ペア、継承、構造体、汎用・非汎用スタックを、生成読み書き3組み合わせと型スタンプ有無で検証した。追加66件が合格した。

- [公式 ID](nunit-evidence/targeted-migrant-serialization-values.tsv)と[実行ログ](nunit-evidence/targeted-migrant-serialization-values.log)

## Migrant SerializationTests のボックスとリスト

nullable 整数、ボックス化された整数の参照共有、文字列タプル、空・入れ子・null を含むリスト、100,000要素のリストを追加した。生成読み書き3組み合わせと型スタンプ有無の公式102件が対象実行で合格した。今回のコレクション設定は treatCollectionsAsUserObjects=false。

- [公式 ID](nunit-evidence/targeted-migrant-serialization-boxes-lists.tsv)と[実行ログ](nunit-evidence/targeted-migrant-serialization-boxes-lists.log)

## Migrant SerializationTests の辞書と集合

12メソッドについて、辞書とハッシュテーブル、キュー、集合、ArrayList、読み取り専用コレクションを型別コーデックで検証した。キーと値の多重集合、共有参照、読み出し順、カスタムキーの個数を維持し、生成経路と型スタンプを組み合わせた72件が合格した。

- [公式 ID](nunit-evidence/targeted-migrant-serialization-maps-sets.tsv)と[実行ログ](nunit-evidence/targeted-migrant-serialization-maps-sets.log)

## Migrant SerializationTests の GUID とカスタムデータ

GUID と BigInteger の型別フィールド処理を追加した。カスタムシリアライズのデータと参照共有、ストリーム位置のずれ、入れ子のジェネリック型、インターフェース型リストも検証し、54件が対象実行で合格した。可変長データや配列は既存の Rust ストリーム表現を再利用する。

- [公式 ID](nunit-evidence/targeted-migrant-serialization-payloads.tsv)と[実行ログ](nunit-evidence/targeted-migrant-serialization-payloads.log)

## Migrant SerializationTests の復元時初期化

一時クラス・フィールドの除外、コンストラクターでの復元、後処理フック、GitHub Issue 60 の Decimal を生成読み書き3組み合わせと型スタンプ有無で検証した。追加42件が合格した。

- [公式 ID](nunit-evidence/targeted-migrant-serialization-initialization.tsv)と[実行ログ](nunit-evidence/targeted-migrant-serialization-initialization.log)

## Migrant SerializationTests の書き込み境界

null・一時オブジェクトのルートとネイティブリソースの拒否、問題のフィールドまでの参照経路、同一設定での連続シリアライズを検証した。拒否ケースでは書き込み時点で処理が終わるため、読み出し方式は公式設定として維持されるが実行には到達しない。対象54件が合格した。

- [公式 ID](nunit-evidence/targeted-migrant-serialization-write-boundaries.tsv)と[実行ログ](nunit-evidence/targeted-migrant-serialization-write-boundaries.log)

## Migrant SerializationTests の生成配列とユーザーオブジェクトのリスト

自己参照、混合型、バイト列、2次元・4次元、ゼロ長次元を含む配列の66件を追加した。生成経路では配列の整数・参照フィールドに型別の処理を使い、バイト列は共通の一括処理を使う。

treatCollectionsAsUserObjects=true の6つのリストメソッドを追加した。Rust のグラフ形式にバッキング配列・件数・バージョンを持つレコードとして格納し、空・入れ子・null・100,000要素のリストと循環参照を検証した。読み書きと型スタンプの8設定で48件が合格した。

- [配列の公式 ID](nunit-evidence/targeted-migrant-serialization-arrays.tsv)と[実行ログ](nunit-evidence/targeted-migrant-serialization-arrays.log)
- [リストの公式 ID](nunit-evidence/targeted-migrant-serialization-user-lists.tsv)と[実行ログ](nunit-evidence/targeted-migrant-serialization-user-lists.log)

### Migrant のユーザーオブジェクト設定でのレコード復元

`TreatCollectionsAsUserObjects=True` の8構成で、空オブジェクト、共有参照、循環、継承、構造体、深さ45,000のグラフを含む13メソッドを移植した。Rust のストリームにコレクション設定を渡し、サポート済みの `List<T>` では設定に対応するレコード構造を検証する。新規104件と既存のリスト48件が対象実行で合格した。[公式IDとの対応](nunit-evidence/targeted-migrant-serialization-user-records.tsv)・[実行ログ](nunit-evidence/targeted-migrant-serialization-user-records.log)

同じ8構成で、列挙値、nullable、ボックス化された値、GUID、BigInteger、多次元配列、配列自身への参照を含む28メソッドを追加した。対象実行で224件が合格した。[公式IDとの対応](nunit-evidence/targeted-migrant-serialization-user-values-arrays.tsv)・[実行ログ](nunit-evidence/targeted-migrant-serialization-user-values-arrays.log)

ユーザーオブジェクト設定のリストに、ボックス化した整数 `1234` の参照を共有するケースと、2種類の実装を交互に100要素格納するケースを追加した。新規16件を含むリストグループ64件が合格した。[公式IDとの対応](nunit-evidence/targeted-migrant-serialization-user-list-identity.tsv)・[実行ログ](nunit-evidence/targeted-migrant-serialization-user-list-identity.log)

同じ8構成で、null・transient・ネイティブリソースの書き込み拒否と、同一セッションでの再シリアライズを検証する9メソッドを移植した。対象実行で72件すべてが合格した。[公式IDとの対応](nunit-evidence/targeted-migrant-serialization-user-boundaries.tsv)・[実行ログ](nunit-evidence/targeted-migrant-serialization-user-boundaries.log)

同じ8構成で、transient メンバーの除外、復元時の初期化とフック、カスタムペイロードの読み取り境界、入れ子のジェネリック配列、および Issue 60 の decimal 値を検証する12メソッドを移植した。対象実行で96件すべてが合格した。[公式IDとの対応](nunit-evidence/targeted-migrant-serialization-user-lifecycle.tsv)・[実行ログ](nunit-evidence/targeted-migrant-serialization-user-lifecycle.log)

非ジェネリックとジェネリックのスタック、およびキューを、バック配列・要素数・バージョンと、キューの読み取り・書き込み位置を持つユーザーオブジェクトとして復元した。新規24件を含む対象グループ88件が合格した。[公式IDとの対応](nunit-evidence/targeted-migrant-serialization-user-sequences.tsv)・[実行ログ](nunit-evidence/targeted-migrant-serialization-user-sequences.log)

同じ8構成で、プリミティブと混在要素の `ArrayList` および `ReadOnlyCollection<object>` を移植した。配列の容量と論理要素数を区別し、読み取り専用ラッパーは元のリストへの参照を保持する。対象グループ112件が合格し、うち24件が新規対応。[公式IDとの対応](nunit-evidence/targeted-migrant-serialization-user-array-list.tsv)・[実行ログ](nunit-evidence/targeted-migrant-serialization-user-array-list.log)

整数およびカスタムオブジェクトの `HashSet` を、バケット、衝突チェーン、スロット、比較器を持つ内部表現で移植した。カスタムオブジェクトのハッシュには Rust 側で割り当てた値を用いる。対象グループ128件が合格し、うち16件が新規対応。[公式IDとの対応](nunit-evidence/targeted-migrant-serialization-user-hash-set.tsv)・[実行ログ](nunit-evidence/targeted-migrant-serialization-user-hash-set.log)

`TransientDerivedShouldAlsoBeTransient` の全16構成を移植した。基底型情報を使って配列の派生インスタンスを除外し、復元結果がボックス化された整数 `0` と `null` になることを検証した。対象実行で16件すべてが合格した。[公式IDとの対応](nunit-evidence/targeted-migrant-transient-derived.tsv)・[実行ログ](nunit-evidence/targeted-migrant-transient-derived.log)

`Dictionary<TKey,TValue>` の4メソッドを、バケット・エントリ・比較器を保持する内部表現で移植した。文字列には決定的な UTF-16 ハッシュを使い、カスタムキーは衝突チェーンに格納する。対象グループ160件が合格し、うち32件が新規対応。[公式IDとの対応](nunit-evidence/targeted-migrant-serialization-user-dictionaries.tsv)・[実行ログ](nunit-evidence/targeted-migrant-serialization-user-dictionaries.log)

`Hashtable` の100組を、ボックス化したキー・値、空バケット、負荷係数と更新状態を持つ内部表現で移植した。新規8件すべてが対象実行で合格した。[公式IDとの対応](nunit-evidence/targeted-migrant-serialization-user-hashtable.tsv)・[実行ログ](nunit-evidence/targeted-migrant-serialization-user-hashtable.log)

`ConcurrentDictionary<int,int>` の10組を、テーブル、バケット、ノード、ロック配列、ロック別カウンターを持つ内部表現で復元した。テスト環境に依存しないようロックは4個に固定した。対象実行で8件が合格した。[公式IDとの対応](nunit-evidence/targeted-migrant-serialization-user-concurrent-dictionary.tsv)・[実行ログ](nunit-evidence/targeted-migrant-serialization-user-concurrent-dictionary.log)

イベントの単独・複数購読、transient な購読先を含むイベント、および有効な購読先が残らないイベントの4メソッドを各16構成で移植した。Rust では登録済みコールバックを名前で解決し、復元された購読先を購読順に呼び出す。transient な購読先の参照は `null` として復元し、呼び出し対象から除外する。対象実行で64件が合格した。[公式IDとの対応](nunit-evidence/targeted-migrant-serialization-events.tsv)・[実行ログ](nunit-evidence/targeted-migrant-serialization-events.log)

別モジュールの受信側に対する拡張メソッド、`null` 引数を渡すインターフェースメソッド、受信側をキャプチャするラムダを各16構成で移植した。Rust では型スタンプで受信側のモジュールを識別し、登録されたコールバックに復元済みの引数と受信側を渡す。対象実行で48件が合格した。[公式IDとの対応](nunit-evidence/targeted-migrant-bound-methods.tsv)・[実行ログ](nunit-evidence/targeted-migrant-bound-methods.log)

イベント配列の各要素が共有キャプチャを通じて親を参照するケースと、`List<bool>` を型引数とする閉じたジェネリックメソッドのデリゲートを各16構成で移植した。閉じたメソッドは Rust のグラフ上で型引数を含む登録名と受信側への自己参照で表現する。対象実行で32件が合格し、Migrant `SerializationTests` の公式1,504件すべてに対応が揃った。[公式IDとの対応](nunit-evidence/targeted-migrant-delegate-graphs.tsv)・[実行ログ](nunit-evidence/targeted-migrant-delegate-graphs.log)

`TypeExtensionsCacheTests` の11件を移植した。CLR リフレクションの入力を Rust のメンバー記述子で明示し、キャッシュを初期化して同じ問い合わせを2回実行したときのミス数、列挙型の要素型、公開問い合わせの網羅を検証した。[公式IDとの対応](nunit-evidence/targeted-type-extensions-cache.tsv)・[実行ログ](nunit-evidence/targeted-type-extensions-cache.log)

`CortexA9GICTests.ShouldSendAndReceiveIPI`（公式 NUnit ID `0-1036`）の固定上流メソッド本体は `Assert.Inconclusive()` のみ。進捗表では非実行対象に、NUnit ランナーでは `inconclusive` に分類する。[公式IDと分類根拠](nunit-evidence/upstream-cortex-a9-gic-inconclusive.tsv)

`UsingTests` の残り9件を移植した。REPL を解析し、MockCPU を生成する Rust の経路でファイル内ローカル参照、インクルード間の参照とオーバーライド順を検証した。登録後の `sysbus.cpu` から最終プロパティ値を取得し、9件が合格した。[公式IDとの対応](nunit-evidence/targeted-using-creation.tsv)・[実行ログ](nunit-evidence/targeted-using-creation.log)

`VerificationTests` の8件を移植した。空宣言、不明な型、文字列・列挙値の型不一致、読み取り専用・重複・未知のプロパティ、および未解決参照について、上流と同じ入力と診断分類を検証した。対象実行で8件が合格した。[公式IDとの対応](nunit-evidence/targeted-repl-verification.tsv)・[実行ログ](nunit-evidence/targeted-repl-verification.log)

`VerificationTests` に6件を追加した。空の周辺機器の生成とアドレス範囲への登録、宣言前後の登録、重複宣言、外部オブジェクトの登録拒否、CPU参照の型不一致を検証した。[公式IDとの対応](nunit-evidence/targeted-repl-verification-objects.tsv)・[実行ログ](nunit-evidence/targeted-repl-verification-objects.log)

`VerificationTests` の登録点診断を3件追加した。既存パーサーで登録点を分類し、未解決参照、型不一致、使用可能なコンストラクタがない文字列を区別する。対象の17件が合格した。[公式IDとの対応](nunit-evidence/targeted-repl-verification-registration.tsv)・[実行ログ](nunit-evidence/targeted-repl-verification-registration.log)

REPL のインラインオブジェクト生成を追加し、不明な型、参照型の不一致、入れ子のプロパティ型不一致、孫オブジェクトのプロパティ設定の4件を移植した。対象の21件が合格した。[公式IDとの対応](nunit-evidence/targeted-repl-inline.tsv)・[実行ログ](nunit-evidence/targeted-repl-inline.log)

インラインオブジェクトを登録点として指定した際の型不一致と、別名で登録した周辺機器から複数のアドレス範囲を宣言順に取得するケースを移植した。対象の23件が合格した。[公式IDとの対応](nunit-evidence/targeted-repl-registration-list.tsv)・[実行ログ](nunit-evidence/targeted-repl-registration-list.log)

`MergeAndCreationTests` の4件を移植した。CPU の登録別名、プロパティの上書き、エスケープされた二重引用符、`none` による列挙値の既定値への復帰を検証し、対象の27件が合格した。[公式IDとの対応](nunit-evidence/targeted-repl-cpu-properties.tsv)・[実行ログ](nunit-evidence/targeted-repl-cpu-properties.log)

`MergeAndCreationTests` の三重引用符入力を6件移植した。改行、引用符のエスケープ、文字列中の `//` を保持し、登録された CPU の実際のプロパティ値を検証した。対象の33件が合格した。[公式IDとの対応](nunit-evidence/targeted-repl-multiline.tsv)・[実行ログ](nunit-evidence/targeted-repl-multiline.log)

列挙型コンストラクタ引数の7件を移植した。型名付き・省略形の列挙定数、数値を受理する条件、任意の数値を受理する属性付き列挙型を区別し、登録後の値と失敗時の診断を検証した。[公式IDとの対応](nunit-evidence/targeted-repl-enum.tsv)・[実行ログ](nunit-evidence/targeted-repl-enum.log)

三重引用符の不正入力5件を移植した。未終了、閉じた引用符の後の余分な文字、同一行での複数の区切り、`using` 内の三重引用符に対する構文診断を検証した。上流が位置をアサートする4件では行・列も一致した。[公式IDとの対応](nunit-evidence/targeted-repl-string-position.tsv)・[実行ログ](nunit-evidence/targeted-repl-string-position.log)

`MergeAndCreationTests` の `empty` 入力5件を移植した。整数・文字列・範囲・オブジェクト参照のコンストラクタ引数と、CPU の列挙型プロパティへの既定値を、登録後の値で検証した。[公式IDとの対応](nunit-evidence/targeted-repl-default-values.tsv)・[実行ログ](nunit-evidence/targeted-repl-default-values.log)

同じ行に4つのエスケープ済み三重引用符を含む上流ケースを追加し、登録後の CPU のプロパティ値を照合した。対象実行で合格した。[公式IDとの対応](nunit-evidence/targeted-repl-repeated-escapes.tsv)・[実行ログ](nunit-evidence/targeted-repl-repeated-escapes.log)

リストと配列のコンストラクタ引数9件を移植した。`empty` と `[]` を区別し、要素順、型不一致、オーバーレイによる置換を検証した。対象の60件が合格した。[公式IDとの対応](nunit-evidence/targeted-repl-collection-lists.tsv)・[実行ログ](nunit-evidence/targeted-repl-collection-lists.log)

整数キーと文字列値の辞書を検証する3件を移植した。`{}` と `empty` を区別し、登録後の件数とキーごとの値を照合した。対象の63件が合格した。[公式IDとの対応](nunit-evidence/targeted-repl-collection-dicts.tsv)・[実行ログ](nunit-evidence/targeted-repl-collection-dicts.log)

CPU参照を要素、キー、値に含むコレクション4件を移植した。前方参照を解決し、登録済みCPUと同一のオブジェクトIDを参照していることを検証した。対象の67件が合格した。[公式IDとの対応](nunit-evidence/targeted-repl-collection-refs.tsv)・[実行ログ](nunit-evidence/targeted-repl-collection-refs.log)

`empty` を型、登録先、引数名、`using` の引数に置く4件を移植し、すべての構文エラーを照合した。[公式IDとの対応](nunit-evidence/targeted-repl-empty-syntax.tsv)・[実行ログ](nunit-evidence/targeted-repl-empty-syntax.log)

真偽値と周辺機器参照に `empty` を渡す2件、および整数引数を `none` で上書きして既定コンストラクタの生成結果を検証する1件を移植した。対象の74件が合格した。[公式IDとの対応](nunit-evidence/targeted-repl-empty-binding.tsv)・[実行ログ](nunit-evidence/targeted-repl-empty-binding.log)

`VerificationTests` の4件を追加した。登録のない別名と登録を取り消した宣言での別名を区別し、既存の `sysbus` に対するコンストラクタ引数と、公開コンストラクタのない型を拒否した。[公式IDとの対応](nunit-evidence/targeted-repl-registration-diagnostics.tsv)・[実行ログ](nunit-evidence/targeted-repl-registration-diagnostics.log)

コンストラクタの診断3件を移植した。インライン引数の型解決をコンストラクタ候補の判定より先に行い、オーバーレイを適用した引数から候補なしと候補の競合を区別した。対象の81件が合格した。[公式IDとの対応](nunit-evidence/targeted-repl-constructor-candidates.tsv)・[実行ログ](nunit-evidence/targeted-repl-constructor-candidates.log)

コンストラクタ依存の6件を移植した。入れ子の候補なし・候補競合、未解決参照、自己循環と複数オブジェクトの循環、インクルードをまたぐ循環を検証した。対象の87件が合格した。[公式IDとの対応](nunit-evidence/targeted-repl-dependencies.tsv)・[実行ログ](nunit-evidence/targeted-repl-dependencies.log)

親周辺機器への登録5件を移植した。前方参照、登録先の更新と取り消し、複数の親への登録、未登録の親の拒否を、登録後の完全修飾名と診断で照合した。[公式IDとの対応](nunit-evidence/targeted-repl-parent-registration.tsv)・[実行ログ](nunit-evidence/targeted-repl-parent-registration.log)

登録診断4件を移植した。整数による登録点コンストラクタの競合、浮動小数点による登録インターフェースの競合、両方の型を実装する登録点、および周辺機器ではない登録先を区別した。最終の対象実行で96件が合格した。[公式IDとの対応](nunit-evidence/targeted-repl-registration-types.tsv)・[実行ログ](nunit-evidence/targeted-repl-registration-types.log)

登録処理の4件を移植した。先行宣言の登録点について型の互換性を確認し、範囲の重複と登録名の衝突を別の診断として検証した。[公式IDとの対応](nunit-evidence/targeted-repl-registration-errors.tsv)・[実行ログ](nunit-evidence/targeted-repl-registration-errors.log)

通常の宣言とインライン生成でのコンストラクタ例外、およびプロパティ設定例外の3件を移植した。登録処理を含む最終の対象実行で103件が合格した。[公式IDとの対応](nunit-evidence/targeted-repl-lifecycle-errors.tsv)・[実行ログ](nunit-evidence/targeted-repl-lifecycle-errors.log)

初期化スクリプトと既存 `sysbus` の参照に関する5件を移植した。初期化ハンドラーへ渡る置換・追記後のコマンド列と、`sysbus` をコンストラクタの依存先として解決できることを照合した。[公式IDとの対応](nunit-evidence/targeted-repl-init.tsv)・[実行ログ](nunit-evidence/targeted-repl-init.log)

`preinit` の置換・追記・実行順、`preinit` と `init` の検証失敗、および重複する `init` ブロックの計7件を移植した。呼び出し引数と順序は上流と同様に記録用ハンドラーで照合した。最終の対象実行では115件が合格した。[公式IDとの対応](nunit-evidence/targeted-repl-preinit.tsv)・[実行ログ](nunit-evidence/targeted-repl-preinit.log)

リセットスクリプトの置換と追記の2件を移植した。上流と同じコマンド列をリセット用ハンドラーに登録することを検証した。[公式IDとの対応](nunit-evidence/targeted-repl-reset.tsv)・[実行ログ](nunit-evidence/targeted-repl-reset.log)

IRQ の検証8件を移植した。モック型の出力と受信インターフェースに基づき、接続先、出力名、既定出力、ピン番号、接続数の診断を照合した。[公式IDとの対応](nunit-evidence/targeted-repl-irq.tsv)・[実行ログ](nunit-evidence/targeted-repl-irq.log)

IRQ の数値範囲、重複する既定出力、既定名と明示名が同じ出力を指す場合の3件を移植した。`VerificationTests` は公式46件すべての対応が揃い、最終の対象実行では128件が合格した。[公式IDとの対応](nunit-evidence/targeted-repl-irq-overlap.tsv)・[実行ログ](nunit-evidence/targeted-repl-irq-overlap.log)

IRQ 接続生成の3件を移植した。明示出力と既定出力の接続の追記、および複数の出力から複数の受信側へ接続する際のピン順と参照同一性を検証した。[公式IDとの対応](nunit-evidence/targeted-repl-wire.tsv)・[実行ログ](nunit-evidence/targeted-repl-wire.log)

IRQ の切断と再接続4件を移植した。単一出力、数値範囲、複数回の切断と置換で、影響する出力と最終接続先を照合した。[公式IDとの対応](nunit-evidence/targeted-repl-wire-cancel.tsv)・[実行ログ](nunit-evidence/targeted-repl-wire-cancel.log)

インクルードをまたぐ複数 IRQ のマージ1件を移植した。上流の各接続先番号、受信側の同一性、指定された接続数を照合した。最終の対象実行で136件が合格した。[公式IDとの対応](nunit-evidence/targeted-repl-wire-overlay.tsv)・[実行ログ](nunit-evidence/targeted-repl-wire-overlay.log)

同じ受信ピンへ向かう出力を共有 `CombinedInput` に接続する1件を移植した。入力番号、両出力が参照する結合器の同一性、下流のピンと受信側を照合した。[公式IDとの対応](nunit-evidence/targeted-repl-fanin.tsv)・[実行ログ](nunit-evidence/targeted-repl-fanin.log)

既存プラットフォームに読み込む2件を移植した。事前登録名の再宣言を拒否し、新しい CPU から既存 CPU を参照した際には同一インスタンスと登録スロットを確認した。最終の対象実行で139件が合格した。[公式IDとの対応](nunit-evidence/targeted-repl-existing.tsv)・[実行ログ](nunit-evidence/targeted-repl-existing.log)

`MachineTestPeripheral` の2件を移植した。暗黙のプラットフォーム依存と、明示的に設定できる整数引数 `machine` を区別し、`mach` の手動指定は上流と同じ診断で拒否した。[公式IDとの対応](nunit-evidence/targeted-repl-machine.tsv)・[実行ログ](nunit-evidence/targeted-repl-machine.log)

三重引用符の残り3件を移植した。行末または単独行のエスケープ済み区切りの文字列値と、途中で閉じ忘れた場合の `At 12:9:` を照合した。最終の対象実行で144件が合格した。[公式IDとの対応](nunit-evidence/targeted-repl-line-escapes.tsv)・[実行ログ](nunit-evidence/targeted-repl-line-escapes.log)

辞書の重複キー1件を移植した。回復可能エラーと型付きの原因を分け、上流と同じ原因メッセージの先頭を照合した。[公式IDとの対応](nunit-evidence/targeted-repl-dict-duplicate.tsv)・[実行ログ](nunit-evidence/targeted-repl-dict-duplicate.log)

CLINT の CPU スコープ付き登録を実際のプラットフォームローダーで扱い、CLINT へのタイムプロバイダー参照と CPU への IRQ 配線を持つ上流入力が読み込めることを検証した。同 fixture の残りは、同じアドレスに CPU 別の NVIC を登録する2コアのケース1件。[公式IDとの対応](nunit-evidence/targeted-repl-signal-cycle.tsv)・[実行ログ](nunit-evidence/targeted-repl-signal-cycle.log)

モニターのコマンド登録と `help` の統合を追加し、登録したコマンドの説明が表示される `AutoLoadCommandTest` を移植した。Rust 側では型を直接登録する。[公式IDとの対応](nunit-evidence/targeted-monitor-features.tsv)・[実行ログ](nunit-evidence/targeted-monitor-features.log)

`ActionArguments` の17件を移植した。上流のコマンド文字列と出力包含アサーションを維持し、名前・位置・省略可能引数と可変長整数引数を検証した。実行環境には C# 拡張の代わりに対応する Rust コールバックを登録する。このバッチは emulation メソッドに絞り、上流セットアップで用意する external オブジェクトとその後処理は今後の indexer バッチに残る。[公式IDとの対応](nunit-evidence/targeted-monitor-action-integers.tsv)・[実行ログ](nunit-evidence/targeted-monitor-action-integers.log)

`ActionArguments` の配列・リスト22件を追加した。角括弧を含む引数を保持し、空配列、末尾カンマ、名前付き配列、`null` を含む整数配列、文字列の可変長引数を、上流と同じコマンドと出力で照合した。同名の2件は NUnit ID で区別した。[公式IDとの対応](nunit-evidence/targeted-monitor-action-arrays.tsv)・[実行ログ](nunit-evidence/targeted-monitor-action-arrays.log)

引数の失敗12件を追加した。重複、名前付き引数の後の不正な位置指定、要素型、閉じていない角括弧などを拒否し、モニターの診断に候補メソッド一覧が含まれることを照合した。対象の51件が合格した。[公式IDとの対応](nunit-evidence/targeted-monitor-action-errors.tsv)・[実行ログ](nunit-evidence/targeted-monitor-action-errors.log)

外部オブジェクトの1次元・2次元インデクサー4件を追加した。ネイティブの `MockExternal` を `emulation AddMockExternal` で作成し、上流と同じ入力で読み書きし、`external Clear` を実行する。対象の55件が合格した。[公式IDとの対応](nunit-evidence/targeted-monitor-action-indexers.tsv)・[実行ログ](nunit-evidence/targeted-monitor-action-indexers.log)

外部コレクションの `Select`・`ForEach` 4件と空の nullable リスト1件を移植した。上流のコマンドと出力アサーションを維持し、続けて実行するコマンド間の状態、可変長引数、入れ子の結果を検証した。ネイティブの拡張登録で `ActionArguments` の全60件が合格した。[公式IDとの対応](nunit-evidence/targeted-monitor-action-external-collections.tsv)・[実行ログ](nunit-evidence/targeted-monitor-action-external-collections.log)

Migrant の `TestGuidVerification` 20件を移植した。Rust の型スタンプのモジュールリビジョンを GUID 差分に対応させ、GUID 変更許可を含意するフラグも含めて、4つの読み書き構成で検証した。[公式IDとの対応](nunit-evidence/targeted-migrant-revision.tsv)・[実行ログ](nunit-evidence/targeted-migrant-revision.log)

直接フィールドの追加・削除40件を移植した。入力スタンプから復元先のフィールド名への対応を求め、追加フィールドは既定値に、削除フィールドは復元結果から除外する。生成読み込みは入力スキーマ用の Rust コーデックで値を読み、復元先へ写像する。最終の対象実行で60件が合格した。[公式IDとの対応](nunit-evidence/targeted-migrant-field-projection.tsv)・[実行ログ](nunit-evidence/targeted-migrant-field-projection.log)

基底型の挿入・削除・名前変更60件を移植した。Rust の型スタンプに継承階層を明示する構成を追加し、上流の4読み書き構成と5許可レベルで復元の成否を照合した。[公式IDとの対応](nunit-evidence/targeted-migrant-hierarchy.tsv)・[実行ログ](nunit-evidence/targeted-migrant-hierarchy.log)

基底型から派生型へ移された同名フィールドの20件を移植した。宣言型で修飾したフィールド名により、削除と追加の両方を許可したときだけ復元に成功することを照合した。最終の対象実行では140件が合格した。[公式IDとの対応](nunit-evidence/targeted-migrant-moved-fields.tsv)・[実行ログ](nunit-evidence/targeted-migrant-moved-fields.log)

型スタンプ重複排除の8件を移植した。値が `null` のリストフィールドも宣言型と型引数を保持し、型名をストリーム内のIDで共有する。上流と同じASCII範囲の抽出でプリミティブ型名と開いたジェネリック型名がそれぞれ1回だけ現れることを照合した。`VersionToleranceTests` の全148件が対象実行で合格した。[公式IDとの対応](nunit-evidence/targeted-migrant-declared-types.tsv)・[実行ログ](nunit-evidence/targeted-migrant-declared-types.log)

`TwoDomainTests` の24件を移植した。書き込みと読み込みに個別のスキーマを設定し、クラス、構造体、入れ子の構造体のフィールド変更と基底型の追加・削除を検証した。上流の復元値と4つの読み書き構成を維持し、対象実行では24件が合格した。[公式IDとの対応](nunit-evidence/targeted-migrant-two-domains.tsv)・[実行ログ](nunit-evidence/targeted-migrant-two-domains.log)

インターフェースをジェネリック型引数に持つ `TwoDomainTests` の4件を移植した。Rust のスキーマで閉じた型を明示し、個別の読み書きスキーマ間で整数フィールド `f = 1` を復元した。[公式IDとの対応](nunit-evidence/targeted-migrant-domain-interface.tsv)・[実行ログ](nunit-evidence/targeted-migrant-domain-interface.log)

復元時のコンストラクタがオブジェクトを生成する4件を移植した。Rust の読み込みは生成オブジェクトをフレーム内に追加し、復元されたフィールド参照から取得できることを検証した。この初期化方式はルート単位の参照管理で利用できる。[公式IDとの対応](nunit-evidence/targeted-migrant-domain-constructor.tsv)・[実行ログ](nunit-evidence/targeted-migrant-domain-constructor.log)

アセンブリバージョン変更の4件を移植した。Rust のフレームに1つのアセンブリバージョンを記録し、モジュールリビジョンとは独立に変更可否を判断する。上流の `1.0` から `1.1` へ移行した際に、入れ子の整数フィールド `a.c == 1` を照合した。`TwoDomainTests` の全36件が対象実行で合格した。[公式IDとの対応](nunit-evidence/targeted-migrant-domain-assembly.tsv)・[実行ログ](nunit-evidence/targeted-migrant-domain-assembly.log)

eMMC カードのコマンド・データ API を追加し、SPI モードの拒否とセクター指定による読み書きの2件を移植した。[公式IDとの対応](nunit-evidence/targeted-sd-emmc-sectors.tsv)・[実行ログ](nunit-evidence/targeted-sd-emmc-sectors.log)

CMD6 のパーティション切り替え、CMD8 の `EXT_CSD` 転送、CMD0 のリセットを eMMC カード API に追加し、残り3件を移植した。ユーザーデータ領域とブート領域の内容、切り替えエラーと設定維持を照合した。`SDCardEmmcTest` の全5件が合格した。テストは上流同様にカード API を直接使用し、ホストコントローラー統合は対象外。[公式IDとの対応](nunit-evidence/targeted-sd-emmc-partitions.tsv)・[実行ログ](nunit-evidence/targeted-sd-emmc-partitions.log)

OpenTitan SPI Device の FIFO 4件を移植した。バスに同じアドレス範囲で登録し、SRAM のバイトアクセス、レジスタのワードアクセス、SPI ホストからの転送を使用した。[公式IDとの対応](nunit-evidence/targeted-opentitan-spi-fifo.tsv)・[実行ログ](nunit-evidence/targeted-opentitan-spi-fifo.log)

OpenTitan SPI Device の割り込み5件を移植した。RX 満杯、RX オーバーフロー、RX/TX ウォーターマーク、TX アンダーフローの境界を上流入力で照合した。同 fixture の全9件が合格した。実装範囲は上流 fixture が使う汎用 FIFO と割り込み。[公式IDとの対応](nunit-evidence/targeted-opentitan-spi-interrupts.tsv)・[実行ログ](nunit-evidence/targeted-opentitan-spi-interrupts.log)

CAST_CANCTRL のリセット値、送信バッファ、割り込み、カウンターの7件を移植した。
対象実行は7件合格、0件失敗。
[公式 ID 別の結果](nunit-evidence/targeted-cast-can-transmit.tsv) · [対象実行ログ](nunit-evidence/targeted-cast-can-transmit.log)

CAST_CANCTRL の受信6件を追加した。受信フレームの形式、フィルター、FIFO、
割り込みのラッチ、オーバーフローの両モードとソフトウェアリセットを確認した。
この fixture の対象実行は13件合格、0件失敗。
[追加6件の公式 ID 別結果](nunit-evidence/targeted-cast-can-receive.tsv) · [13件の実行ログ](nunit-evidence/targeted-cast-can-receive.log)

OpenTitan KeyManager の5件を移植した。KMAC-256 で上流の状態遷移時の鍵を導出する。
ROMイメージを読み込まないセットアップは、上流ROMコントローラーの初期ダイジェストを注入して再現した。
対象実行は5件合格、0件失敗。
[公式 ID 別の結果](nunit-evidence/targeted-opentitan-key-manager.tsv) · [対象実行ログ](nunit-evidence/targeted-opentitan-key-manager.log)

PythonPeripherals.SerializationTests の単体クローン2件を移植した。既存の CPython
ワーカーの変数とインポートを保存し、新しいワーカーに復元する。
対象実行は公式2件と復元基盤の回帰1件が合格、0件失敗。
[公式 ID 別の結果](nunit-evidence/targeted-python-peripheral-snapshot.tsv) · [対象実行ログ](nunit-evidence/targeted-python-peripheral-snapshot.log)

PythonPeripherals.SerializationTests のリスト・辞書ケースを移植した。CPython の
辞書復元と Migrant の既存プリミティブ I/O を使い、メモリーストリームを巻き戻して復元する。
対象実行は公式3件と回帰1件が合格、0件失敗。
[追加ケースの公式 ID 別結果](nunit-evidence/targeted-python-peripheral-stream.tsv) · [対象実行ログ](nunit-evidence/targeted-python-peripheral-stream.log)

SerializationTests の MappedMemory 単体ケースを移植した。128 MiB のメモリーに
100 KiB の非重複バッファー30個を書き込み、LZ4 圧縮したストリームから復元して全バイトを比較した。
Rust 側では既存の固定シード乱数器を使用した。対象実行は1件合格、0件失敗。
[公式 ID 別の結果](nunit-evidence/targeted-emulation-mapped-memory.tsv) · [対象実行ログ](nunit-evidence/targeted-emulation-mapped-memory.log)
