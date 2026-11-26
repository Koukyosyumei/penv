# 固定 Renode の全件実行とトリアージ

対象 SHA と手元の依存準備は [公開テストの再現手順](renode-upstream-ci.md) を参照。
Linux x86-64 で公開マニフェストから Robot 304 スイートの 2,197 インスタンス、
NUnit 公式 6 プロジェクトの 3,413 ID を取得した。

## 全件実行

```sh
tmux new-session -d -s penv-upstream-batch '
  DOTNET_PROCESSOR_COUNT=4 target/renode-venv/bin/python scripts/upstream/run.py \
    --all --jobs 10 --out target/upstream-batch \
    --nunit-root target/renode-discovery \
    --dotnet "$PWD/target/renode-dotnet/dotnet" \
    >target/upstream-batch.log 2>&1
  echo $? >target/upstream-batch.exit
'
tail -f target/upstream-batch.log
```

各ワーカーはスイートと HotSpot バリアントを単位に、元の Robot ファイルと
共通リソースをまとめて実行する。`--jobs` は 1〜10。各完了は
`progress.jsonl` に書き、スイート単位の JSON、Robot XML、
ケースごとの `case.json`、モニター応答と標準エラーを保存する。
全件をまとめた `results.json` のキーはエンジン、上流 ID、パラメーター。
未対応や失敗を含む全件実行は終了コード 1。途中経過はスイート JSON とログを参照する。

`penv` は Robot から起動した CLI、`penv-rust` は NUnit ID と対応する Rust テスト。
NUnit の VSTest 一覧と discovery XML は ID、カテゴリ、状態を確定するための証跡。
上流の合否を penv の合否に転用しない。別途、固定上流 Renode の
`renode-test --fixture 'hex.Should Load HEX' --keep-renode-output -r <出力先> tests/unit-tests/hex.robot`
で HEX を実行し、上流自体の PASS を `target/upstream-reference-hex/robot_output.xml`
に保存した。ネイティブ参照実行の範囲は HEX 1 件。

## 判定と修正範囲

| verdict | category の例 | 根拠 |
| --- | --- | --- |
| PASS | `passed` | penv の観測と Robot 判定の成功、または Rust の個別テスト成功 |
| FAIL | `assertion_failed`, `penv_execution_failure` | 元のアサーション失敗、または稼働中の penv コマンド失敗 |
| UNSUPPORTED | `adapter_missing`, `penv_unimplemented` | アダプターの不足、または penv が操作・モデルの非対応を返した |
| SKIP | `skip`, `dependency_blocked` | 上流指定、または必要状態が提供されなかった |
| NOT_RUN | `environment`, `no_measurement`, `penv_execution_failure` | 準備・起動前の停止、または合否に使える観測がない |

共通原因の修正はアダプターとモニター入口に限定した。

- 固定ボード専用の一回限りの RESC 経路を、同じ penv プロセスに
  複数の命令を送るフレーム付き stdio に拡張。ファイルとインラインの
  プラットフォーム、`using sysbus`、UART の複数待機、エコー、LED、
  ADC と既存 CPU・バス操作を接続した。
- モニターの数値表示を上流の幅、大文字16進、Decimal / Both に合わせた。
  この差で中間実行時に失敗した STM32F4 質量消去・セクタ消去の
  アサーションをそのまま再実行した。
- `Provides ... Reexecution` は対応済み操作で成功したプロバイダーの
  記録を使用する。依存状態が得られなければ上流と同様に依存ケースを
  SKIP し、状態名を記録する。シリアライズ依存、Pause/Serialize
  HotSpot、およびホスト Sleep 中の非同期動作には対応が必要。
- ログテスターと複数機、デバッガ・ネイティブ統合を要するテストには
  未対応判定が残る。モデルの実装追加は今回の変更範囲外。
- NUnit は従来の整数 3 件に、
  `ParserPrimitivesTest.ShouldParseQuotedString` と
  `ShouldParseQuotedStringWithASemicolon` を追加した。
  既存 REPL の文字列パーサーで上流の引用符とセミコロンを検証する。

差分は ID とパラメーターで突合する。

```sh
python3 scripts/upstream/compare.py \
  target/upstream-full-first/results.json target/upstream-batch/results.json \
  >target/upstream-batch-delta.json
```

## `include` / `i` と `set` の比較

`target/upstream-full-triage/results.json` で最初の未対応モニター操作が
`include` / `i` だった231件と、`set` だった200件を基準にした。
比較はケース ID、パラメーターと元のコマンド文字列で行い、
次のリモート呼び出しまたは Robot XML の次の実行済みキーワードを証跡にする。

```sh
python3 scripts/upstream/monitor_delta.py \
  target/upstream-full-triage/results.json target/upstream-monitor-final/results.json \
  >target/upstream-monitor-final-delta.json
```

`set` は引用符、エスケープした空白、`$name`、`$name = ...` と
`?=`、グローバルと選択機の変数を扱う。Robot から受け取った
変数は RESC の前置きにも引き渡す。`include` / `i` はファイルの種類ごとに
既存のローダーへ接続する。`.repl` は上流と同じく未選択時に機械を作り、
ロードに使用するプラットフォームを選ぶ。`.resc` は元のファイルを
既存の penv RESC ローダーへ渡す。ネストした RESC の `$ORIGIN` と
引用符付きパスを保持する。C#／Python の include は対応するローダーの不足を返す。
REPL を選択した段階や penv のロード前停止は、ケースの PASS に加えない。

初回の比較では、`set` 200件と `include` / `i` 10件が次へ到達した。
その時点で止まった CLIC 144件の `Download File` を追加し、
元の全144件で上流が指定する署名ファイルを取得してサイズと SHA-1 を照合した。
144件すべてが元の RESC に進み、penv が `logLevel 0` を非対応と報告した。
署名比較のアサーションは未到達。

最終の全件実行は `c43be89` で実施した。penv 実行ファイルの SHA-256 は
`45166f340816e9d655f866119329e694c66a0cd10773f35efab93a9fc57bc45d`。
分類の根拠、コマンド、実測は `target/upstream-monitor-final/results.json`、
ケース別差分は `target/upstream-monitor-final-delta.json`、
全件の前後比較は `target/upstream-first-to-final.json` にある。

| 対象 | 件数 | 次の操作に到達 | 同じ操作を penv に投入 | パス解決後に同じ操作で停止 | 受付後に次の操作の証跡なし | その他の停止 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `set` | 200 | 200 | 0 | 0 | 0 | 0 |
| `include` / `i` | 231 | 39 | 123 | 62 | 3 | 4 |

| Robot 全件 | PASS | FAIL | UNSUPPORTED | SKIP | NOT_RUN |
| --- | ---: | ---: | ---: | ---: | ---: |
| 最初の全件実行 `8bca7e9` | 3 | 0 | 2,186 | 3 | 5 |
| 最終 `c43be89` | 18 | 0 | 1,965 | 133 | 81 |

最終の UNSUPPORTED はアダプター未対応が 1,196 件、penv 非対応が 769 件。
SKIP の128件は依存状態が未提供。NOT_RUN は環境24件、起動段階の
penv エラー39件、合否を判定できる観測なし18件。
`cargo test --locked --bin penv monitor::tests` は13件すべて合格した。
