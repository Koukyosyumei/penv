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

SerializationTests は、生成読み書き・コレクション特殊設定・型スタンプを
すべて無効にする公式設定 `(False,False,False,False)` から9件を移植した。
Rust のオブジェクトレコードは既存の Migrant プリミティブでフレーミングする。
参照 ID をフィールドの走査前に確保し、読み手は各参照を検証してから
オブジェクト群を公開する。二ノード循環、自己参照、配列の自己参照、
共有文字列、45,000リンクの復元を確認した。

型スタンプ対応後は、同じ9ケースを `(False,False,False,True)` にも対応付けた。
Rust 形式では型名、モジュール、版、フィールド名と種別を記録し、読み手が
ローカルスキーマと照合してからグラフを公開する。組み込み文字列と配列の種別は
レコードタグで表す。CLR のバイト列との相互運用は移植範囲に含めていない。

StampingTests は反射書き込み・反射読み込みの4組を移植した。スタンプ有無が
一致する場合は自己参照を復元し、不一致の場合は拒否した。固定 C# でも対応する
4件が合格した。Rust ではフィールド名の異なるスタンプの拒否も追加で確認した。
ObjectReaderWriterTests の1件と OpenStreamSerializationTests の反射・強参照または
ルート単位参照の5件も型スタンプを有効にして移植した。
[公式 ID 別の結果](nunit-evidence/targeted-stamps-symbol-dedup.tsv) · [固定 C# の StampingTests ログ](nunit-evidence/stamping-official.log)

SurrogateTests は反射読み書き・型スタンプの有無2設定で6メソッド計12件を
移植した。書き手は元のオブジェクト ID に対して変換を一度適用し、読み手は
復元先の ID を維持して置換する。書き込み側の同一性、読み書き双方の型の
置換、同じ登録での反復、読み込み側で作る自己参照を検証した。
既存のグラフ API を使い、プリミティブ、文字列、異種混在の一次元配列も
型スタンプの有無2設定で計6件を移植した。
[公式 ID 別の結果](nunit-evidence/targeted-surrogates-arrays-symbols.tsv) · [固定 C# の SurrogateTests ログ](nunit-evidence/surrogates-official.log)

SurrogateTests の継承、インターフェース、ジェネリック定義による選択を
反射読み書き・型スタンプの有無2設定で11メソッド、計22件追加した。
型関係は Rust 側で明示的に登録する。適用可能な候補の中で型の特殊化を優先し、
比較できない候補は上流の登録順を維持する。具体型での無効化は基底型への
フォールバックを止める。固定 C# の保存済み結果と Rust の対象実行で22件が合格した。
[公式 ID 別の結果](nunit-evidence/targeted-hierarchy-symbol-lookup.tsv)

OpenStreamSerializationTests の `UseWeakReference` の2件を、反射読み書き設定で
追加した。書き手はグラフとオブジェクト ID の対応を弱く保持し、グラフが
解放された後もシリアル化済みのスロット番号は再使用しない。読み手は復元した
ルートが所有するオブジェクトを弱く参照し、同じルートの再読込で実際の
オブジェクト同一性を維持する。自己循環を含む最後のルートを解放した後は、
失効した参照の読み込みを拒否する。上流の配列に対応する Rust のテストは、
3つの読み込み結果を保持して比較する。残る21件には生成読み書きが必要。
[公式 ID 別の結果](nunit-evidence/targeted-weak-stream-symbol.tsv) · [固定 C# の OpenStreamSerializationTests ログ](nunit-evidence/open-stream-official.log)

SerializationTests の反射読み書き・型スタンプ有無の2設定に14件を追加した。
多次元配列は次元長と行優先の要素順を記録し、復元前に形状を検証する。
型付きフィールドと文字列タプルでは、参照先の型と値を復元した。
[公式 ID 別の結果](nunit-evidence/targeted-cache-rectangular-records.tsv)

残りは9 fixture・2,049ケース。生成読み書き、コレクションと CLR 型の
コーデック、ジェネリック変換器の生成とキャッシュ、フック、バージョン許容を
伴う型変更の復元が必要。
