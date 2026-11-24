# 固定 Renode NUnit の移植進捗

対象は固定リビジョン `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` の
公式 Release discovery 6プロジェクト、87 fixture、3,413ケース。

| 状態 | ケース数 |
| --- | ---: |
| Linux 対象で Rust 移植・公式 ID 対応済み | 591 |
| 移植・対応済みの Windows 専用ケース | 3 |
| 移植待ち | 2,805 |
| 上流 Ignore | 14 |

進捗表は公式インベントリと対応表から生成した。移植待ち 64 fixture の
ケース数と不足 API を各行に記録した。合否を示す列は含めていない。
[現在の fixture 別進捗](nunit-evidence/port-progress.tsv)

今回追加した対応は169件。内訳は GPIODetaching の7件、Migrant の103件、
ReadExtensions の30件、WriteExtensions の26件、DataStorage の2件、LBA Storage の1件。
GPIODetaching の7件は既存の GPIO ルート管理を使い、上流と同じ接続トポロジーと
接続解除後の観測を再現した。汎用の C# SystemBus オブジェクト登録機構は使わない。

方針変更前に開始した対応済みケースの一括実行は、変更の指示を受けた時点で
実行済みだった。変更後の確認は ReadExtensions 30件、WriteExtensions 26件、
DataStorage 2件、Storage 1件の各 fixture に限定し、全件合格した。この進捗表では、
旧一括結果を新しい対応件数の合否として集計しない。全対象の実装と公式 ID 対応が
揃ってから一括実行する。

```sh
python3 scripts/upstream/nunit_port_progress.py \
  docs/nunit-evidence/inventory.jsonl.gz docs/nunit-evidence/port-progress.tsv
```
