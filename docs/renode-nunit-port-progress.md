# 固定 Renode NUnit の移植進捗

対象は固定リビジョン `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` の
公式 Release discovery 6プロジェクト、87 fixture、3,413ケース。

| 状態 | ケース数 |
| --- | ---: |
| Linux 対象で Rust 移植・公式 ID 対応済み | 678 |
| 移植・対応済みの Windows 専用ケース | 3 |
| 移植待ち | 2,718 |
| 上流 Ignore | 14 |

進捗表は公式インベントリと対応表から生成した。移植待ち 57 fixture の
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

今回の追加は22件。Migrant の型スタンプ付き SerializationTests が9件、
StampingTests が4件、OpenStreamSerializationTests が5件、ObjectReaderWriterTests が1件。
既存の範囲 API を使った SymbolLookup の DeduplicationTests が3件。
新規22件は対象に絞った Rust 実行で合格した。
[公式 ID 別の結果](nunit-evidence/targeted-stamps-symbol-dedup.tsv) · [Rust の対象実行ログ](nunit-evidence/targeted-stamps-symbol-dedup.log)

SystemBus の残り4件には、汎用登録の一時停止・再開、単一領域内の複数アクセスコールバック、
パラメーター付き登録、null CPU の登録に対応する API が必要。
Migrant の未移植分には、弱参照保持、生成読み書き、組み込みサロゲート、
シリアライズ時のフック、型変更を伴うオブジェクトの復元が含まれる。
全対象の移植と公式 ID 対応が揃ってから一括実行する。

```sh
python3 scripts/upstream/nunit_port_progress.py \
  docs/nunit-evidence/inventory.jsonl.gz docs/nunit-evidence/port-progress.tsv
```
