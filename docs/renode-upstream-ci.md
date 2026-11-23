# 固定 Renode の公開テストを penv で実行する

参照は Renode `c38ac3babcd38a7144b9498e3ca4c2226b76aa37`。
`../renode` は読み取り専用で使用する。生成物、NUnit のビルド用クローン、
実行ログは penv worktree 内に置く。

## 公開ランナーの選択規則

- `renode-test` は `tests/run_tests.py` に `tests/tests.yaml` を渡す。
  マニフェストの文字列は単独スイート、マッピングは並列実行時のグループを表す。
  `run_tests.py` は Robot `.robot` / `.robot.rst` と NUnit `.csproj` を登録する。
- `tools/common.sh` は OS を `linux` / `osx` / `windows`、
  ホストを `aarch64`（`aarch64` または `arm64`）/ `i386` に分類する。
  除外タグは既定の `excluded`、`exclude_<os>`、`exclude_host_<arch>`。
  Robot の実行時スキップは `skipped` と `skip_<os>`。
- Robot の列挙とタグ選択は固定版の `RobotTestSuite.select_tests` を呼ぶ。
  ケース中にプロバイダーが `Handle Hot Spot` を検出すると、
  `None`、`Pause`、`Serialize` のインスタンスを作る。
  証跡では通常ケースの `hotspot_action` を JSON の `null`、
  HotSpot の既定アクションを文字列 `"None"` とする。
- 公開 NUnit プロバイダーは公式プロジェクトを `dotnet build` し、
  `dotnet test --no-build` の実行時に `TestCategory != ...` を AND で適用する。
  NUnit に Robot の `--skip` は渡さない。VSTest の `--list-tests` が
  `--filter` を無視することは、存在しない完全修飾名を渡して確認した。
  この基盤は VSTest の名前一覧を NUnit アダプターの discovery XML と照合し、
  XML のカテゴリとルートから継承したカテゴリでホスト選択を計算する。
  NUnit の `runstate` も記録する。カスタム名が重複する場合は
  discovery XML の NUnit ID を付けてインスタンスを区別する。
  表示名から引数値を回収できる範囲を超える NUnit ケースは、
  XML とテスト名を照合用に保存する。

公開の `renode-test --dry-run` はこの Linux x86-64 ホストで
**310 ファイル**を選んだ。内訳は Robot **304 スイート**と NUnit **6 プロジェクト**。
Robot はタグ選択後に **2,197 インスタンス**（実行時スキップ指定 **3**）を列挙した。
これらは列挙件数。旧担当別台帳の 3,247 行はこの集計の対象外。

## 再現

Python 3.9 以上、Rust/Cargo、および配布元のファームウェアを取得できる環境を使用する。
記録する penv のソースをコミットした状態で実行する。
出力ディレクトリは worktree 内の新規パスを毎回指定する。

```sh
python3 -m venv target/renode-venv
target/renode-venv/bin/pip install -r scripts/upstream/requirements.txt

target/renode-venv/bin/python scripts/upstream/run.py \
  --out target/hex-replay --suite tests/unit-tests/hex.robot \
  --case 'Should Load HEX' --negative-control

target/renode-venv/bin/python scripts/upstream/run.py \
  --out target/robot-inventory --inventory-only
```

Robot は上流ファイルをそのまま読み、固定プロバイダーと同じ共通リソース、
既定のセットアップと後処理を設定する。localhost の Robot Remote が
元のテストボディから受けた操作を RESC に書き、1 回の UART 待機を含む
単一機の起動を同じ penv バイナリで実行する。
`Create Terminal Tester` と `Start Emulation` は RESC に順に記録され、
待機で CLI を起動する。penv は対象ポートのテスターをエミュレーション前に装着する。
元の既定仮想タイムアウトは 8 秒。LF で完結した行にリテラル文字列が
含まれるかを元のケースと同じ条件で判定する。配布イメージは URL のサイズと
SHA-1 を照合した上でキャッシュし、SHA-256 を記録する。
上流の失敗時スナップショットと Renode プロセスログは無効化しており、
Robot XML、penv の標準出力・標準エラー、RESC を保存する。
この経路は複数回の UART 待機、実行中の入力やモニター操作、HotSpot、
状態依存およびプロファイリングを扱うケースでは完走しない。

NUnit discovery には .NET 10 SDK、固定サブモジュール、Renode の公開ビルドが
生成する `Directory.Build.targets` と `lib/resources` が必要。
独立したクローンを使用する。次の準備では `cmake`、C コンパイラー、
`make` も使用する。

```sh
git clone --no-hardlinks --no-checkout ../renode target/nunit-renode
git -C target/nunit-renode checkout --detach c38ac3babcd38a7144b9498e3ca4c2226b76aa37
git -C target/nunit-renode submodule update --init --recursive
(cd target/nunit-renode && ./build.sh --no-gui --external-lib-arch arm-m.le)
git -C target/nunit-renode/lib/resources checkout --detach 9d7824771f3db67663950cae06f5bb7f89fbf33b

target/renode-venv/bin/python scripts/upstream/run.py \
  --out target/upstream-replay --negative-control \
  --nunit-root target/nunit-renode
```

`--dotnet` で SDK 実行ファイルのパスを指定できる。
検証では公開 `build.sh` がリソース取得と targets 生成を完了した後、
ネイティブビルド開始時に `cmake` 不在で停止した。
以降、runner の managed ビルドで 6 件の公式プロジェクトを個別にビルドできた。
既定では HEX と STM32F072b の 2 Robot スイートを選択する。
`--suite` は複数指定でき、`--case` はケース名の完全一致で追加選択する。
既定選択には未対応ケースが含まれるので、その実行は結果を保存して終了コード 1 を返す。

## 実行済み NUnit 対応

公式プロジェクトは `tests/unit-tests/RenodeTests/RenodeTests.csproj`。
名前の共通接頭辞は
`Antmicro.Renode.UnitTests.PlatformDescription.ParserPrimitivesTest.ShouldParseHexadecimalLong`。
上流の `[Values]` が渡す数値文字列で penv の `parse_address64` を呼び、
変換に成功し同じ整数になることを Rust で検証する。
対応は discovery で一致した場合に限り、Rust の `--exact` で 1 ケースずつ実行する。

| 入力 | 期待値 | Rust テスト |
| --- | --- | --- |
| `0x1000` | 4096 | `upstream_nunit::hexadecimal_unsigned_long_1000` |
| `0x12AB` | 4779 | `upstream_nunit::hexadecimal_unsigned_long_12ab` |
| `0x0ff` | 255 | `upstream_nunit::hexadecimal_unsigned_long_0ff` |

`inventory.json` にホストと発見した Robot ケースを、`results.json` にケース ID、
パラメーター、ソース SHA、バイナリ SHA-256、起動コマンド、実測、判定、
各ケースの証跡パスを記録する。NUnit discovery と Rust 実行のコマンド、
生ログ、Rust テスト実行ファイルの SHA-256 も保存する。
合格数にはケースの完走と実際のアサーション成功を要する。

## 検証記録

実測時の penv は `3ed51ef`。CLI の SHA-256 は
`5f2b8e471bb4b30b548d1efd9153852156421687ddca55bbae7b19d260b8c64a`。
対応する Rust テスト実行ファイルの SHA-256 は
`9fa5d0d6b6575df9cadb01fc65ca0f507f4796c0d42c909412bfc6eaf2bf5849`。
ローカルの全証跡は `target/upstream-final/results.json` と、
そこに示す `robot-*/case.json`、`robot.xml`、`boot.resc`、
標準出力・標準エラーおよび `nunit/` にある。

| 上流ケース | 判定 | 観測または停止箇所 |
| --- | --- | --- |
| `hex.robot::Should Load HEX` | PASS | `Hello World! stm32f072b_disco`。8 秒の上流既定値、penv 終了コード 0 |
| 同ケースの診断 | FAIL（診断） | 同じ HEX・操作・期待行でタイムアウトを 1 µs に短縮。UART テスターが `Limit` を報告し、penv 終了コード 1、Robot XML は FAIL |
| `STM32F072b.robot::Should Run Zephyr Hello Sample` | PASS | `Hello World! stm32f072b_disco` |
| `STM32F072b.robot::Should Run stm32f0-crc-test Application` | PASS | `test result: ok. 840 passed; 0 failed` |
| `STM32F072b.robot::Should Run Zephyr Console Echo Sample` | 未対応 | `Write Line To Uart` |
| `STM32F072b.robot::Should Run Zephyr Blinky Sample` | 未対応 | `Create LED Tester` |
| `STM32F072b.robot::Should Run Zephyr Button Sample` | 未対応 | 実行中の `sysbus.gpioPortA.button Press` |
| `STM32F072b.robot::Should Read ADC` | 未対応 | 実行中の `sysbus.adc.adc-channel1 Volts 0.6` |

通常の Robot **3 ケース**と NUnit 対応 Rust **3 ケース**が完走し、
すべて合格した。診断の失敗はこの 6 件の外数。
NUnit の **3,413** discovery インスタンスのうち **3,399** が
このホストで runnable、**14** が NUnit 側の無視指定だった。
runnable のうち **3,396** は Rust 対応付けが未実装。

| 公式 NUnit プロジェクト | Discovery | Runnable |
| --- | ---: | ---: |
| MonitorTests | 85 | 85 |
| UnitTests | 627 | 621 |
| PeripheralsTests | 119 | 114 |
| RenodeTests | 282 | 279 |
| OptionsParserTests | 53 | 53 |
| Migrant Tests | 2,247 | 2,247 |

補助確認として `cargo test --locked --bin penv` を実行した。
変更後は **542 成功、58 失敗**。開始時の `a34cf6a` を別ディレクトリに
展開して同じコマンドを実行すると **539 成功、58 失敗**で、失敗名は完全に一致した。
