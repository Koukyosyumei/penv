# Migrant NUnit の後続バッチ

固定 Renode の Migrant gitlink は
`169677b3e43dbbc530c9f7c3f4a13ba49e40bb2d`。
13 fixture・2,247ケースについて、公式 Release discovery の完全名を
C# の `[TestFixture(...)]` と突き合わせた。各 fixture で、コンストラクター設定の
組がソースと一致し、設定間でメソッドとその引数の組が揃っていることを確認した。
[パラメーター群と件数](nunit-evidence/migrant-parameters.tsv)

- SerializationTests は16設定 × 94ケース。生成・リフレクションの読み書き、
  コレクション処理、型スタンプの組を保持する。
- PrimitiveReaderWriterTests はバッファ設定2種 × 51ケース。両設定を合わせ、
  メソッド引数付きの90ケースを含む。
- VersionToleranceTests は読み書きの4設定 × 37ケース。メソッド引数付きの
  140ケースを含む。
- OpenStreamSerializationTests は読み書きの4設定 × 7ケース。メソッド引数付きの
  20ケースを含む。残る fixture の設定名と件数は TSV に記録した。

等価性は公式 ID ごとに判定する。各設定を Rust 側でも選び、元のセットアップ、
テスト値、例外とアサーションを保つ。参照の同一性と循環、ストリーム間の状態、
サロゲート、フック順序、型変更の許容条件、低レベルのバイト表現は、
それぞれ観測対象となるケースで照合する。生成とリフレクションの設定が同じ
Rust バックエンドへ集約される場合、その機能差が各ケースで観測できるかを
確認するまで別設定の合格には数えない。現在の RAM スナップショット API では
これらの観測を再現できないため、Rust の等価性と合否は後続バッチで測定する。

```sh
python3 scripts/upstream/audit_migrant_nunit.py \
  docs/nunit-evidence/inventory.jsonl.gz \
  /home/ec2-user/Dev/penv-renode-ci/target/renode-discovery/lib/Migrant \
  /tmp/migrant-parameters.tsv
```

## 完了した独立マイルストーン

PrimitiveReaderWriterTests の全102件と IdGeneratorTests の1件を移植した。
バッファ有無、件数、長文字列の指定を元の組のまま残した。
上流のランダム入力と同じ値生成範囲と条件を、再現できる Rust 側の固定シードで実施した。
上流の `ShouldSerializeTimeSpan` は DateTime を書き込み、DateTime として読む。
Rust 版も同じ観測を維持する。固定 C# と Rust の結果は103件の ID すべてで合格した。
C# の出力から得た整数バイト列と128・1024・4096バイトのフレーム境界も、
追加の Rust テストで照合した。
[公式 ID 別の比較](nunit-evidence/migrant-comparison.tsv) · [公式生ログ](nunit-evidence/migrant-official.log) · [C# のバイト列と境界](nunit-evidence/migrant-wire.log)

TypeDescriptorComparisonTests の11件を追加した。型階層上のフィールド移動、
隠蔽フィールドの挿入、transient フィールド、アセンブリ版とモジュール GUID の許容条件を
固定 C# で確認し、Rust では同じ形を明示的な型記述子で組み立てた。
固定 C# と Rust の対象実行で11件とも合格した。
[公式 ID 別の比較](nunit-evidence/migrant-descriptor-comparison.tsv) · [公式ログ](nunit-evidence/migrant-descriptor-official.log)

残りは10 fixture・2,133ケース。オブジェクトストリーム、サロゲート、フック、
グラフと型変更の等価性は引き続き移植待ち。
