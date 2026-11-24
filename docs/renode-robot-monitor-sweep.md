# Robot 全件再実行とモニター対応

対象は Renode `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` の公開マニフェストを
Linux x86-64 で選択した 304 スイート、2,197 インスタンス。各段階で
同じ ID とパラメーターを投入した。上流 Robot ファイル、テストデータ、アサーションは
固定ツリーから読み取る。

```sh
tmux new-session -d -s penv-robot-sweep '
  target/renode-venv/bin/python scripts/upstream/run.py \
    --all --jobs 10 --out target/robot-sweep \
    >target/robot-sweep.log 2>&1
  echo $? >target/robot-sweep.exit
'
```

NUnit discovery を併走する場合は `--nunit-root` を明示する。
Robot 専用の `--all` は指定を省略できる。

## 修正した停止原因

- `logLevel` の数値と名前を上流のレベルに対応させ、記録されるログに適用した。
  CLIC 144件は `logLevel 0` を通過し、`machine UserState` に到達した。
- 文字列の `UserState`、複数行の変数、`echo` のモニター出力を扱った。
  CLIC の次の停止は `rvtest_code_end` に取り付ける Python コールバック。
  既存の停止用フックと一般の Python フックを区別し、後者の非対応を報告する。
- `Create Log Tester` と `Wait For Log Entry` を実際の penv ログと
  仮想時間に接続した。既存のログフックと STM32F4 のフラッシュ警告を記録する。
  STM32F4 の消去拒否ケースは penv と、別途実行した公式 Renode の両方で合格した。
- RISC-V の PC、レジスター、ステップ、実行モード、トラップベクターを
  既存の CPU 実装に接続した。対応する `AssembleBlock` は LLVM の出力を
  penv の RAM へ書く。ラベル・再配置を要するアセンブリーには対応が必要。
- 同期の `Add Symbol`、`Store`、アドレスとシンボルのメモリ比較は、
  指定の幅で実際に penv のバスを読み書きする。アサーション付きの
  `memory-assertions.robot` は8件合格。RV32 の残りは指定された
  `V` 拡張を penv が非対応と報告、または時間付きアサーションなどが未実装。

## 全件再実行の結果

| 実行 | penv SHA | PASS | FAIL | UNSUPPORTED | SKIP | NOT_RUN |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 再開前 `upstream-monitor-final` | `c43be89` | 18 | 0 | 1,965 | 133 | 81 |
| ログレベル `robot-loglevel` | `5a2766d` | 18 | 0 | 1,965 | 133 | 81 |
| UserState・レジスター `robot-userstate` | `772593d` | 22 | 12 | 1,949 | 133 | 81 |
| モニターログ・未登録アドレス `robot-logwait-retry` | `a9dc1b0` | 31 | 14 | 1,791 | 133 | 228 |
| 実行モード・LLVM `robot-scheduler` | `6660406` | 32 | 16 | 1,893 | 133 | 123 |
| メモリ・シンボル `robot-memory` | `0f9cec3` | 40 | 16 | 1,876 | 133 | 132 |
| ResetVector・アセンブラー引数 `robot-reset` | `c81f379` | 42 | 14 | 1,876 | 133 | 132 |

最終実行の全ケースは `target/robot-reset/results.json`、再開前との差分は
`target/robot-reset-delta.json`、CLIC 144件の停止点履歴は
`target/robot-resume-audit.json` に保存した。上流 Renode の
STM32F4 ログケースは `target/reference-flash-warning/robot_output.xml`。
モニター・ログの Rust チェックは合計14件合格。

## CLIC の Python フックと署名検証

固定上流の `AddHook` と同様に、指定した CPU の PC が ELF シンボルの
アドレスに達すると Python を同期実行する。各フックは独立した永続スコープを
持つ。今回接続した API は `cpu.Bus.GetSymbolAddress`、`cpu.Bus.ReadByte`、
`cpu.Log`、`machine.UserState`。実行環境は CPython で、IronPython の .NET
相互運用 API は対象外。アクセスログは標準 REPL が登録した32ビットアドレスの
メモリへのスカラーアクセスを対象にする。RV32 は既定で WFI が即時復帰する
従来動作を保ち、明示的な `WfiAsNop false` で割り込み待ちを有効にする。

再現には Python 3、`llvm-mc`、`ld.lld` が必要。下の診断は固定上流の
`end_hookscript` をそのまま抽出し、合成 RV32 ELF を走らせる。一致では元の
ログ述語が `MATCH`、署名不一致と割り込み待ちでは `FAIL` を返す。
診断は上流 Robot の件数に加算しない。

```sh
scripts/upstream/check-clic-hook.sh \
  target/robot-xmlrpc/penv ../renode target/clic-hook-final
```

同じ304スイートを最大10並列で各段階に投入した。各 `results.json` に
バイナリの SHA-256、呼び出し、ケースID、パラメーター、実測、分類と証跡がある。
`target/robot-clic-hook-audit.json` は CLIC とインライン REPL の停止理由を
ケースIDとパラメーターで照合した結果。

| penv コミット | 証跡のディレクトリ | PASS | FAIL | UNSUPPORTED | SKIP | NOT_RUN | CLIC の停止点 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| `973e5fe` | `target/robot-python-hook` | 42 | 15 | 1,882 | 133 | 125 | `LogPeripheralAccess` |
| `4deaa3e` | `target/robot-python-trace` | 42 | 15 | 1,871 | 133 | 136 | `WfiAsNop` |
| `280f115` | `target/robot-wfi` | 42 | 15 | 1,871 | 133 | 136 | CLIC コントローラー |
| `45160e6` | `target/robot-xmlrpc` | 42 | 14 | 1,872 | 133 | 136 | CLIC コントローラー |

CLIC arch 144件は全IDで両モニターコマンドを越えた。現行の停止理由は
元の REPL に必要な `IRQControllers.CoreLocalInterruptController` の未実装。
別の CLIC 8件も同じプラットフォームに依存する。上流 CLIC の署名アサーションは
起動前に止まっているため未評価。

引用符の修正では以前 `invalid inline platform quoting` だった24件が
全件そこを越えた。次の理由は penv 本体未対応12件、アダプター未対応12件。

UART の ANSI 色コードが XML-RPC を壊していた
`VEML7700.robot::Should Work From Zephyr Shell` は、上流と同じエスケープ除去を
通して Robot に応答できた。生の UART 応答は測定証跡に保持している。
ケースは次のセンサー操作 `sysbus.i2c1.als Illuminance 500` で
`adapter_missing` になった。

`cargo test --locked --bin penv -- --test-threads=4` の比較ログは
`target/robot-wfi-rust-tests.log` と `target/robot-rust-baseline-full.log`。
変更前の `ed298c1` と今回の Rust テストは両方547件合格、58件失敗で、
失敗したテスト名も一致した。

## RV32 CLIC の署名スイート

固定 Renode の `CoreLocalInterruptController.cs` と tlib の RISC-V
`helper.c`、`op_helper.c` に沿って、CLINT のソフトウェア／タイマー割り込みを
CLIC の入力3／7に接続した。RV32 M-mode のしきい値、優先度、直接・間接
ベクター、`mcause` の別名ビット、`mnxti`、条件付きスクラッチ交換を実装した。
ELF32 のロード区間が隣接する複数の RAM をまたぐ場合は、同じバイト列を
各マッピングに分けてロードする。

`clicdirect-01_default` の上流 ELF は `0x80002000` からのロード区間が
`0x80004000` の署名 RAM にまたがる。分割ロード後はゲストが署名比較に達し、
未対応のスクラッチ CSR によるトラップで9ワードが不一致だった。
CSR 実装後は72ワードすべて一致し、固定 Renode と penv の両方が元の述語
`All signatures correct` を満たした。

公式 Renode の実行は worktree 内の別コピー `target/renode-discovery` で行った。
RISC-V のネイティブ翻訳ライブラリを下記コマンドで作成する。
参照 XML は `target/clic-reference/official-check/robot_output.xml`。
ELF、署名と両エンジンの SHA-256 は `target/clic-reference/checksums.sha256`。

```sh
export PATH="$PWD/target/renode-dotnet:$PWD/target/renode-venv/bin:$PATH"
(
  cd target/renode-discovery
  ./build.sh --skip-fetch --no-gui --external-lib-arch riscv
  ./renode-test -T robot --skip-building -r ../clic-reference/official-check \
    -f "Should Pass clicdirect-01_default" \
    tests/peripherals/CLIC/CLIC-riscv-arch-tests.robot
)
target/renode-venv/bin/python scripts/upstream/run.py --jobs 4 \
  --suite tests/peripherals/CLIC/CLIC-riscv-arch-tests.robot \
  --suite tests/unit-tests/hex.robot --suite tests/platforms/STM32F072b.robot \
  --suite tests/unit-tests/memory-assertions.robot --out target/clic-arch-retest-new
```

今回の実装範囲は既定レイアウトの単一 RV32 CLIC と M-mode 割り込み。
S-mode 割り込みの配送と間接 CSR のペリフェラルアクセスは引き続き未実装。

`419ab97` の固定バイナリによる確定結果は CLIC 144件のうち76件 PASS、
68件 FAIL。68件はいずれも元のログアサーションに失敗した。
前回の144件はすべて CLIC プラットフォーム未実装で起動前停止していた。
HEX、STM32F072b、メモリアサーションの3スイートは両コミット間でテストIDごとの
結果が一致した。内訳は14件 PASS、53件 UNSUPPORTED。
`target/clic-arch-retest/results.json` と `progress.jsonl` に全ケースの証跡があり、
`target/clic-reference/case-audit.json` に比較結果がある。

残る共通原因の代表は `sclicdeleg-01_default`。ELF の `0x800002b8` が
未実装の `sintthresh` CSR（`0x147`）に書き込み、署名の `mcause` が
期待値 `0x10000009` に対して違法命令の `0x30000002` になった。
`sclic*` の64件を進めるには、S-mode の CLIC CSR と特権別の割り込み配送・
委譲が必要。CLINT の Rust 回帰テストは5件合格した。

もう一つの失敗群は `clicshvinhv-01` の4件。ゲストが `mcause.MINHV` を
設定して `mret` した際に、`mepc` のベクターテーブルエントリーを読むように修正した。
`3172275` では4件とも元の Robot ケースを個別に走らせて PASS を確認した。
この短い検証の証跡は `target/clic-reference/minhv-audit.json`。
144件の集計は `419ab97` の結果として保持する。

## RV32 CLIC の S-mode

`a1b85ca` は `stvt`、`sintthresh`、`sintstatus`、`snxti` と
S-mode のスクラッチ交換 CSR を実装する。割り込み先の特権は CLIC の
属性で選択し、同期例外の `medeleg` と分けて扱う。S-mode のトラップから
復帰すると以前の割り込みレベルを復元する。

`sclicdeleg-01_default` は固定 Renode と penv の元の Robot ケースで合格。
penv の診断ログでは104ワードの一致を確認した。全144件の penv 再計測では、
直前の全件測定に対して76件の既存 PASS を維持し、68件が FAIL から PASS に移った。
同時に実行した HEX、STM32F072b、メモリアサーションの67件は、
ケースIDごとに従来の14 PASS、53 UNSUPPORTED と一致した。

`target/clic-supervisor/official/robot_output.xml` が固定上流の代表ケースの証跡。
penv の SHA-256、コマンド、各ケースのパラメーターと実測は
`target/clic-supervisor-arch/results.json` に記録した。
差分は `target/clic-supervisor/case-delta.json` にある。
この144件とは別に、間接 CSR のペリフェラルアクセスを使う CLIC ケースがある。

## Robot の照度入力と UART 正規表現

`a6b7d70` は `Illuminance` と `FeedIlluminanceSamplesFromRESD` を
既存の照度モデルにつなぎ、Robot の `treatAsRegex=true` をテキスト UART
の実データに適用する。Rust の UART テストは18件合格した。
固定上流の VEML7700 スイートで照度設定、定周期 RESD、タイムスタンプ付き
RESD、Zephyr の旧・新ドライバーとシェル応答を測定した。

公式の CSV 変換ツールは `construct==2.10.68` に依存する。初回の2件では
変換用依存が欠け、空の RESD が渡された。固定上流の依存を venv に入れて
同じスイートを再実行した。

```sh
target/renode-venv/bin/python -m pip install -r scripts/upstream/requirements.txt
target/renode-venv/bin/python scripts/upstream/run.py --jobs 1 \
  --suite tests/peripherals/VEML7700.robot --out target/robot-veml7700-reproduce
```

再実行は元の5件すべて PASS。`target/robot-veml7700-resd/results.json` に
SHA、コマンド、ケースID、測定を記録した。HEX と STM32F072b の7件は
合計6 PASS、1 UNSUPPORTED で、UART アダプター変更前の結果と一致した。

### CLIC と UART アダプターの変更をまとめた全件実行

`495f119` の固定バイナリで304スイート、2,197件を最大10並列で実行した。
`target/robot-clic-veml-sweep/results.json` の確定値は192 PASS、14 FAIL、
1,716 UNSUPPORTED、133 SKIP、142 NOT_RUN。
前回の全件測定 `45160e6` に比べて150件が UNSUPPORTED から PASS に移った。
内訳は CLIC 署名144件、VEML7700 5件、UART の正規表現を使う
`STM32F4_Discovery.robot::Should Not Crash On RTC Writes` 1件。
Cortex-M TrustZone の6件は起動時に `enableTrustZone` プロパティーで
止まり、UNSUPPORTED から NOT_RUN に移った。

署名スイート以外の CLIC 8件には、TrivialUart の Robot テスター2件、
REPL の CLIC 上書き2件、アセンブリーの再配置4件が残る。
ケースIDとパラメーターを照合した差分は
`target/clic-supervisor/grouped-verdict-delta.json` に保存した。

## CLIC の残り8件と共通 Robot 操作

`7e9b7d8` は RISC-V の `AssembleBlock` で LLVM が再配置を出す命令を
ELF オブジェクトとして組み立て、`ld.lld` で指定アドレスに解決する。
元の `j .` とローカルラベルを扱い、未解決の外部シンボルはエラーにする。
CSR テストでは `MSTATUS` を読み、無効な `mcause.MPP` の書き込みで
以前の MPP を保持する規則も検証した。

REPL の既存エントリーへのプロパティー上書きは、対象名とプロパティー名で
マージした後、通常のモデル検証に渡す。CLIC では上流が指定する
`machineLevelBits` を使って割り込みのレベルと優先度を分ける。

実際の TrivialUart 出力を既存テスターに渡す。テキストの不在判定は
指定された仮想時間内に該当行が現れた場合に失敗する。
`LoggingUartAnalyzer` は接続した UART の行を Info ログに記録する。
CLIC の `OnGPIO` と間接 CSR を元のレジスターバンクに接続した。

対象の元の8件はすべて PASS。HEX とメモリアサーションはケースIDごとに
前回と同じ9 PASS、52 UNSUPPORTED。UART の Rust テスト18件と
対象のプラットフォームテスト2件も合格した。
`target/robot-clic-remaining/results.json` と `case-audit.json` に証跡がある。

まとめた全件実行は `--all --jobs 10` で304スイート、2,197件を完了した。
`target/robot-clic-final-sweep/results.json` の確定値は200 PASS、14 FAIL、
1,708 UNSUPPORTED、133 SKIP、142 NOT_RUN。前回と比較して、
今回の CLIC 8件だけが UNSUPPORTED から PASS に移った。
署名スイート144件と合わせ、CLIC の152件が PASS。
`target/robot-clic-final-sweep/clic-audit.json` に CLIC の全測定、
`verdict-delta.json` に全ケースの判定差分がある。
再配置を含むアセンブリーには `llvm-mc` と `ld.lld` が必要。

## 既存機種の Robot 縦断対応

基準は `target/robot-clic-final-sweep/results.json`。表示する母数は
スイートパスに機種名または代表ボード名が含まれるケース。
実行対象を絞るときも、元のスイート、入力、アサーションを使った。

| 系統 | 変更前に確認した主な停止点 |
| --- | --- |
| STM32F072 | GPIO ボタン操作と、ボタンを保持する間のホスト Sleep |
| STM32F407 | UART アナライザー、起動後の GPIO 配線、UART 正規表現のキャプチャグループ |
| STM32H7 | 単一マシンの名前指定、タイマー周波数と RCC クロックゲート・リセット |
| LiteX | UART テスターに既存の UART 出力が未接続 |
| BeagleV | 先頭に連続する `@` を含むパス指定、RV64 UART テスター |
| nRF52840 | UART テスター、経過時計の読み取り |

`e1d7e13` は複数の UART バックエンドの実出力を Robot テスターに接続する。
STM32 の GPIO 配線は既存の端子・信号を使い、正規表現のグループを
Robot の上流アサーションに返す。単一マシンの作成と選択を既存経路に通す。

`5448066` は H7 の既存 RCC 状態と実レジスター書き込みを公開し、
F072 スイートのボタン保持中にゲスト時間を進める。
`fecf28d` は RV64 の UART も同じテスターに接続する。
`91c8bb7` は nRF のタイミングと UART 不在判定に実際の機種別クロックを使う。
NUnit の discovery とケース実行は省略した。

F072 のボタンと LED 保持、F407 の PWM・直交デコーダー・RTC アラーム、
H7 の ADC DMA・暗号サンプル・RCC の3件、LiteX の Tock が元のアサーションに合格した。
F407 の4チャネル PWM は観測周波数が許容範囲を外れた。
BeagleV-StarLight は `SetGlobalSerialExecution` または外部 PMP の追加で止まった。
BeagleV-Fire は OpenSBI、U-Boot、ボード名を実測し、後続のログイン前に
Robot の制限時間に達した。証跡は
`target/robot-board-sweep/suite-0181/robot.xml` と `case-0000/monitor.jsonl`。

nRF52840 の集計ではタイムアウト中のリモート呼び出しに完了状態がなく、
そのケース以降の記録が集計エラーで落ちた。残った Robot XML と
個別モニターの実測から WFE、kernel.interrupt、CMSIS DSP の3件で
元のアサーションの合格を確認した。
`target/board-nrf-verified-pass.json` に別途記録し、元の全件結果を保存した。
この3件は先行の代表実行 `target/robot-board-probes/results.json` でも PASS。

最終の固定バイナリは `91c8bb7`。Robot を304スイート、2,197件、
最大10並列で実行した。機種別の元の集計を下に示す。

| 系統 | 件数 | 変更前 PASS | 実行報告の PASS | 代表的な測定 |
| --- | ---: | ---: | ---: | --- |
| STM32F072 | 6 | 5 | 6 | ボタンと LED 保持 |
| STM32F407 | 13 | 5 | 8 | PWM、直交デコーダー、RTC |
| STM32H7 | 40 | 11 | 16 | ADC DMA、暗号と RCC |
| LiteX | 26 | 0 | 1 | Tock の D1/D2 UART 出力 |
| BeagleV | 8 | 0 | 0 | Fire のブート途中まで |
| nRF52840 | 18 | 0 | 1 | シェル。別照合でさらに3件 |

全件の生の判定は213 PASS、20 FAIL、1,677 UNSUPPORTED、133 SKIP、
154 NOT_RUN。前回の生の判定では200 PASSだった。
差分と各機種のケースID別の停止点は
`target/robot-board-sweep/verdict-delta.json` および `board-audit.json` にある。
集計エラーを修正した `56acad4` は、未完了の時間切れと測定済みの合格を
分ける小テストで検証した。上記の全件結果は修正前の生データである。
UART の Rust テスト18件と H7 タイマーの Rust テスト3件も合格した。

## 起動バイナリーと既存のセンサーモデル

`c7df138` で、Robot から複数の `sysbus LoadBinary` を既存の
RESC バイナリーローダーへ渡せるようにした。LiteX の PHY
`VendorSpecific1` と LED 状態読出しを既存モデルに接続した。
nRF52840 の ADXL372 は SPI/I2C コントローラーとセンサーの名前で解決し、
軸の値を g からモデルが使う 0.1 g 刻みに変換する。
`9f3dfaf` は raw バイナリー起動で指定された PC を READY 前に適用する。

Arty LiteX の `Should Read Ethernet PHY Status` は
10 Mbps リンクダウンと100 Mbps リンクアップを元の UART アサーションで
確認した。結果は `target/common-phy/results.json`。
nRF52840 の SPI/I2C 代表ケースでは既存 ADXL372 の更新を受理した。
起動時 UART 出力待ちがホスト時間上限に達したため、
ここではセンサー測定アサーションを合格として数えていない。

`4a7672d` は固定上流のテキスト UART テスターに合わせて、照合前に ANSI の
色・装飾設定を除去する。バイナリーテスターと生の UART 出力はそのまま扱う。
CrossLink の `Should Run LiteX BIOS` は BIOS CRC と CPU 表示、コンソールの
`help`、LED の False から True への変化を元のアサーションで確認した。
結果は `target/common-crosslink-final/results.json`。Rust の UART テストは19件合格。

全件再実行は `--all --jobs 10` で、最終ペイロードは `4a7672d`。
NUnit は実行しない。

固定上流 `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` の304スイート、
2,197件を完了した。penv バイナリーの SHA-256 は
`b08b988112420ad797eb68d5ae8b45cc883edd879ac3a70cadcbc718d1c41726`。

| 判定 | 前回 | 今回 |
| --- | ---: | ---: |
| PASS | 213 | 220 |
| FAIL | 20 | 24 |
| UNSUPPORTED | 1,677 | 1,677 |
| SKIP | 133 | 133 |
| NOT_RUN | 154 | 143 |

合格の差分は Arty の PHY、CrossLink の BIOS、LiteX SMP の OpenSBI と
Linux の計4件、および `56acad4` により全件報告に反映された nRF52840 の
WFE、カーネル割り込み、CMSIS DSP の計3件。nRF52840 本体のスイートは
UART シェルを含む4 PASS、2 FAIL、6 UNSUPPORTED、1 SKIP、4 NOT_RUN。
別スイートの GPIO PullUp は UNSUPPORTED。Alarm、SPI、I2C、
タスクウォッチドッグはそれぞれ Robot の時間上限に達した。
I2C チップ ID とバス走査の2件は `0xe000ef34` への未マップアクセスで停止した。

全件実行の再現コマンドは以下。

```sh
target/renode-venv/bin/python scripts/upstream/run.py --all --jobs 10 --out target/robot-common-sweep
```

結果とケースIDごとの証跡は `target/robot-common-sweep/results.json`、
`board-audit.json`、`verdict-delta.json`、進捗は `progress.jsonl` にある。
上流 Renode 本体での実行はこの集計の対象外。

全件計測後に `c85951e` で、ADXL372 として解決できない
LSM330、LIS2DS12、LSM303 への加速度設定を UNSUPPORTED に分類した。
対象3スイートの再実行で3件が FAIL から UNSUPPORTED に変わったことを
確認した。ケースごとの差分は
`target/common-adxl-classification/classification-delta.json` にある。
表の値は全件計測時の固定バイナリーの結果を示す。

## Rust モニターへの移行

`scripts/upstream/run.py --rust-monitor` は起動前の `Execute Command` を
そのまま UTF-8 でまとめ、通常 CLI の `penv run --monitor-stdio --script -`
に送る。起動時は `PENV-STARTUP <バイト数>\n` と本文、起動後は
`PENV-COMMAND <バイト数>\n` と1コマンドの本文を送る。
`run --script PATH` も同じ Rust ローダーを使う。
Robot の証跡ディレクトリの `boot.resc` には送信した起動コマンドが残る。
Rust が使用する公開アセットを要求した場合は、Python が取得して同じ
起動コマンドを再送する。

```sh
target/renode-venv/bin/python scripts/upstream/run.py --rust-monitor \
  --suite tests/unit-tests/hex.robot --out target/monitor-hex
target/renode-venv/bin/python scripts/upstream/run.py --rust-monitor \
  --suite tests/platforms/STM32F072b.robot --out target/monitor-f072
```

この経路では、起動前コマンドの結果は最初の起動要求で確定する。
起動後の変数は Rust セッションに保持される。複数マシン、
対話中のプラットフォーム再読込、汎用スナップショット、
非同期の時間進行はこの移行の対象として残っている。

起動後の RESC の `include` は、同じマシンでサポート済みのコマンドを
実行する。起動時と同じ相対パス解決を使い、成功後は定義した変数も参照できる。
`emulation RunFor` とログの期限は Rust がゲストの仮想クロックから換算する。
待機中に仮想時間が進まなかった場合、UART とログの否定アサーションは
指定期間を観測済みとして扱わない。

単一 Cortex-M の実行開始前にライブモニターから `Save` すると、メモリーと
RAM の配置、ベクターテーブル、セッション変数を保存する。
同じマシンのライブ `Load` で復元できる。既存のメモリーのみの形式も
読み込める。実行中の CPU・周辺機器・イベントキューのスナップショットは
対象外。

固定 Renode と同じコマンド列で、起動後の `include` と変数による
読み出し（`0x08001331`）、`RunUntil` の絶対時刻、同期点の量、
保存後の RAM・変数・同期量の復帰を比較した。証跡は
`target/monitor-live-include/`、`target/monitor-until-proof/`、
`target/monitor-quantum-proof/`、`target/monitor-snapshot-proof/`、
`target/monitor-snapshot-quantum-proof/` にある。
`target/monitor-captures-proof/` には nRF ファームウェアの実際の起動行に
対して Rust と固定 Renode が同じ正規表現キャプチャを返した記録がある。

上流を変更せず、`--rust-monitor` で STM32F072b 6 件すべてと
nRF52840 の `Should Restart After Watchdog Timeout` が合格した。
`target/monitor-cortex-f072/results.json` と
`target/monitor-cortex-nrf/results.json` にケース ID、ビルドの SHA と
ハッシュ、実行コマンドおよび判定を記録した。LED 保持の期限を
仮想時計に統一した後、`Should Run Zephyr Button Sample` と
`Should Load HEX` を再実行し、両方が合格した
（`target/monitor-led-button/results.json` と
`target/monitor-led-hex/results.json`）。

数値表示、UART アイドル、Robot のマシン引数は同じ Rust モニター経路で確認した。
`target/monitor-namespace-proof/` には同一の RAM 読み出しについて
Renode、penv の RESC ファイル、penv のライブモニターの出力がある。
`target/monitor-idle-proof/` では nRF ウォッチドッグファームウェアを
使い、UART 0 の活動を検出して失敗し、UART 1 のアイドル確認に成功した。
`target/monitor-machine-context-proof/` では空白を含むマシン名で RAM を
読み書きした。存在しないマシンを指定した書き込みは失敗し、RAM の値は
変化しなかった。

`target/monitor-idle-resc-proof/` の通常 CLI 実行では、2つの UART
テスターを起動前に登録し、アイドル・活動あり・観測予算不足を区別した。
LiteX の同じ `RunFor` と時刻照会を固定 Renode、RESC ファイル、
ライブモニターで実行し、1 ms と 3 ms が一致した
（`target/monitor-rv-resc-proof/`）。
`pause` 後の `RunFor`、`RunUntil`、`RunToNearestSyncPoint` も固定
Renode と一致した（`target/monitor-paused-run-proof/`）。

未変更の Arty LiteX `Should Boot` は実 UART で `H2U` を観測し、
次の `Provides Serialization` で未対応となった
（`target/monitor-rv-arty/results.json`）。STM32F4 の FAULTMASK ケースは、
UART アイドル待機中にホスト上限に達して未実行となった
（`target/monitor-idle-stm/results.json`）。

RESC のログ待機はライブセッションと同じ Rust のログテスターを使う。
`target/monitor-log-resc-proof/` では固定 Renode と同じ過去時刻の警告を
両方の入口で取得し、ステップ予算で打ち切った不在確認は失敗した。
`target/monitor-log-pause-proof/` ではログ待機の停止オプションで双方が
仮想時間 1 ms に達した。`target/monitor-log-output-proof/` には待機中の
nRF の UART 出力が残る。

`target/monitor-snapshot-atomic-proof/` では起動前 RAM の読み戻しが
固定 Renode と一致し、penv が実行後の上書き保存を拒否した際にも
既存ファイルの SHA-256 が一致した。
`target/monitor-sysbus-before-repl-proof/` ではプラットフォーム定義前の
sysbus ポリシーが読み込み後も有効であり、未マップ読み出しが失敗した。
`target/monitor-rv-rate-proof/` では RISC-V の CPU 速度切り替えを含む
同じコマンド列を使い、固定 Renode と penv が仮想時間 3 ms を報告した。

未変更の RISC-V バス障害ケースは、プラットフォーム読み込み前の
sysbus 設定を通過し、次の未対応コマンド `cpu MaximumBlockSize 1` で
停止した。`target/monitor-sysbus-order-fault/results.json` に記録した。
同じビルドでの HEX ケースは合格した
（`target/monitor-sysbus-order-hex/results.json`）。

複数行の引用引数は RESC ファイルと Robot のコマンドフレームで共通の
Rust パーサーを通る。RISC-V の `AssembleBlock` は起動中と起動後の両方で
既存の LLVM アセンブラーを呼び出す。`cpu PC` でアドレスを取得する形式も
含め、固定 Renode と同じ 8 バイトが RAM に書かれた
（`target/monitor-assemble-proof/`、`target/monitor-assembly-address-proof/`）。
固定 Renode ローカルビルドの `platform-lib/linux-x64/` には、同ビルドの
`lib/resources/llvm/libllvm-disas.so` を参照するリンクを設定して比較した。

未変更の `Should Clear MTVAL On EBREAK And ECALL` は、固定 Renode と
penv の両方で合格した。`target/monitor-csr-write-proof/native-robot.xml`
と `target/monitor-csr-rv/results.json` が個別の結果であり、
`target/monitor-pc-assert-proof/` に PC 比較の成功・失敗の記録がある。
同じ penv ビルドの未変更 `Should Load HEX` も合格した
（`target/monitor-csr-hex/results.json`）。

起動前 Cortex-M スナップショットは、RAM とベクターテーブルに加え
R0–R14 と PC を保存する。旧 RAM 専用形式も読み込める。
`target/monitor-snapshot-numeric-proof/` と
`target/monitor-snapshot-clear-proof/` に、固定 Renode、通常 CLI、
ライブモニターで同じ値を復元した記録がある。
`Clear` 後は `Load` まで実行を止める。既存の nRF シェル検証では、
復帰後に実機種モデルの UART から `pong` を得た
（`target/monitor-snapshot-gpr-proof/nrf-clear.log`）。
実行後の保存と、周辺機器やイベントキューを含む状態復帰は未対応。

起動前スナップショットの読み込みは通常 RESC とライブモニターで
同じ Rust 実装を使う。`target/monitor-cross-snapshot-proof/replay.py` は
一方で保存し、もう一方の新しいプロセスで復帰する両方向のコマンドを実行する。
ライブで保存した PC、R0、同期量を通常 RESC から確認した。通常 RESC
からの保存は従来どおりメモリーと起動前 CPU レジスターを含む形式を使う。
同じ検証ディレクトリの `renode.log` には、固定 Renode が別プロセスで
保存ファイルを読み込んで PC と R0 を復元した記録がある。

`target/monitor-pause-include-proof/` の `timed.resc` を固定 Renode、
通常 CLI、ライブモニターで読み込み、`pause` 後に報告された仮想時間
1 ms、3 ms、3.1 ms が一致した。penv の通常 CLI と Robot のライブ経路は
`start` と `pause` を共通の Rust アクションで扱う。

UART の既定期限とテスター選択は Rust が処理する。
`target/monitor-uart-default-proof/` で固定 Renode と penv が
未受信を失敗と判定することを確認した。penv はテスター既定の 2 ms と
呼び出し指定の 3 ms を観測して計 5 ms に達した。固定 Renode は
バックグラウンドでも進むため、照会時刻は異なる。
`short.log` はステップ上限による打ち切りで不在確認が失敗した証跡。

`target/monitor-global-uart-proof/` では全体の既定値を変更しても
作成済みテスターの期限が保持され、新しいテスターには新しい値が
適用された。`target/monitor-failing-uart-proof/` には、既定テスターの
失敗文字列を登録して失敗し、解除後に同じファームウェアからの次の行を
受信して合格した固定 Renode と penv の RPC 結果がある。

`target/monitor-uart-symbol-proof/` では同じ ELF に対し
`cpu PC \`sysbus GetSymbolAddress "main"\`` を実行し、固定 Renode、
通常 CLI、ライブモニターで PC が `0x400027c0` になった。
未変更の `Should Respect The Default Tester Id` は固定 Renode で合格した。
penv は ELF シンボルを使う起動コマンドの解析を通過し、次の
`UART.SiFive_UART` のロードで未対応となった
（`target/monitor-uart-symbol-selected/results.json`）。

UART の送信先、送信後のエコー待機と既定のエコーポリシーは Rust の
テスターが処理する。`target/monitor-tester-send-proof/` には
同じゼファーのコンソールファームウェアで通常 RESC とライブモニターが
`Echo working?` を受信した記録と、未変更の上流ケースを固定 Renode で
実行した結果がある。`target/monitor-tester-echo-default-proof/` では
エコーを返さない UART に対し、既定の待機を無効にすると送信が成功し、
明示的に待機を指定すると両方で失敗した。

`target/monitor-uart-delimiter-proof/` では CR を行末に指定し、同じ nRF
ファームウェアの2行を固定 Renode と同じ内容で取得した。2行目の
先頭の LF も一致した。

UART の `matchNextLine` と `defaultMatchNextLine` は Rust が処理する。
`target/monitor-first-line-proof/` では nRF の実出力で、先頭行の不一致が
失敗し、後続の呼び出しで両方の行を受信できた。固定 Renode と penv は
次行照合で成功した正規表現の空のキャプチャ配列も一致した。
`target/monitor-next-line-default-proof/` では既定値を明示的に上書きした
呼び出しが両方で合格した。

`target/monitor-unfinished-uart-proof/` では nRF シェルの未完了プロンプトで
不在確認を行った。完成行だけを対象にすると合格し、未完了行を含めると
固定 Renode と penv の両方で失敗した。

未変更の STM32F072b コンソールエコーと nRF ウォッチドッグは合格した。
`Should Finish Instructions Before Pausing` は penv では
`CPU.ARMv7R` のロードで未対応となった
（`target/monitor-next-line-precise/results.json`）。

バイナリ UART の待機は通常 RESC と Robot が同じ Rust の履歴・照合処理を
使う。`target/monitor-binary-uart-proof/` の両実行では、nRF の出力から
CR/LF をまたぐバイト列と正規表現キャプチャを同じバイト値で得た。
`target/monitor-binary-empty-group-proof/` では未一致グループも固定
Renode と同じ空のバイト列として RPC から返った。

CPU を持たない `UART.TrivialUart` の REPL を実行できるようにした。
固定 Renode と penv の両方で上流の全256バイトのリテラル・正規表現
ケースを実行した。固定 Renode の結果は
`target/monitor-cpuless-uart-proof/native-robot.xml` と
`target/monitor-cpuless-uart-native-bytes/native-robot.xml`、
penv の結果は `target/monitor-cpuless-uart-regex/results.json` と
`target/monitor-cpuless-uart-bytes/results.json` にある。
通常の RESC の実行は `target/monitor-cpuless-uart-proof/all-bytes.resc`
と `file.log` に記録した。

`Wait For Next Line On Uart` は Rust で次の完成行を受信する。
`target/monitor-next-any-proof/` では、固定 Renode と penv が CPU なしの
`TrivialUart` から `A`、空行、`B` を順に受信し、4回目の待機で
失敗した。通常 RESC からも同じ3行を読み出した。

CPU なしの構成の仮想時計は 10 ns 刻みで動作する。通常 RESC、
ライブモニター、固定 Renode で 50 ns と 150 ns を確認した
（`target/monitor-cpuless-time-proof/`）。命令数の指定なしで
250 ms まで進めるコマンド列も一致した
（`target/monitor-cpuless-long-proof/`）。明示した
`--max-steps 1` は引き続き実行を打ち切った。

CPU を持たない `TrivialUart` 構成では、実行後の仮想時刻と同期量を
`Save` で保存し、`Load` で復帰できる。固定 Renode、通常の RESC、
ライブモニターで、時刻 10 ms と同期量 2 ms の復帰、および復帰後の
11 ms への進行が一致した（`target/monitor-cpuless-save-proof/`、
`target/monitor-cpuless-clear-proof/`）。後者は `Clear` を挟む。
破損データや異なる UART 配置を拒否した場合、時刻は変化しない。
この形式は未接続の UART と空のイベントキューを対象とする。

CPU なしの `TrivialUart` 構成では、Robot の `Provides Serialization`
が Rust のスナップショットを保存し、`Requires` が別プロセスで
プラットフォームを構成して読み込む。構成コマンドは Rust が現在の
マシンと UART 配置から生成する。`target/monitor-provides-snapshot-proof/`
に固定 Renode と penv の RPC 比較があり、復帰後の 10 ms と同期量 2 ms
が一致した。提供側の `RunFor` は復帰時の起動入力に含まれない。

新しい CPU なしスナップショットは UART の既定期限も保存する。
`target/monitor-provides-uart-policy-proof/` で、保存時の既定期限 2 ms
が復帰され、旧形式の読み込みでは現在の既定期限を維持することを確認した。
未変更の全256バイト正規表現ケースは合格した。
Arty LiteX の `Should Boot` は `H2U` を受信した後、
実行中のゲスト CPU を含む `Provides Serialization` で未対応となった
（`target/monitor-provides-arty/results.json`）。

`e470184` と `3054329` では、Robot が提供側と復帰側をそれぞれ
`SaveRestorableMonitorSnapshot` と `RestoreMonitorSnapshot` に委ねる。
Rust は保存時に対応する構成を検査し、復帰時にファイルのメタデータと
UART 配置からマシンを構成する。Python が依存状態に保持するのは保存パス。
通常 CLI からも、新規 RESC の `RestoreMonitorSnapshot @snapshot.save`
で同じ経路を使える。復帰用 RESC を保存ファイルと同じディレクトリに置き、
`emulation GetTimeSourceInfo` を続けて `penv run --script restore.resc`
を実行する。セッション付き保存と CPU なしの単体保存の両方で
仮想時刻 10 ms、同期量 2 ms を復帰した。既存マシンがある起動構成と
末尾を切り詰めた保存ファイルは終了コード 1 で拒否した。

固定上流 `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` の
`Provides` / `Requires` 比較と CLI の出力は
`target/monitor-provides-snapshot-proof/` にある。固定 Renode の
`renode.jsonl` と penv の `restorable-penv.jsonl` で復帰時刻と同期量が一致した。
比較に使用したデバッグバイナリの SHA-256 は
`022fcda50973496083b94ece8c0d33044d2bdb6250719bfbd18e16d7ede607b0`。
未変更の `Should Load HEX` と `Should Run Zephyr Console Echo Sample`
も合格した（`target/monitor-restorable-hex/results.json`、
`target/monitor-restorable-f072/results.json`）。
代表 Robot 3 件の固定バイナリの SHA-256 は
`10e779fde203ae303be52f7e93a72d8f41039f4b986ef403ed818e4d28639a4c`。
Arty LiteX の `Should Boot` はゲストの起動出力を受信し、
保存コマンドが CPU 付き構成を拒否したため未対応のまま
（`target/monitor-restorable-arty/suite-0000.json`）。

`7920259` は明示的な `$global.name` とマシンローカルの検索を
分離し、文字列の先頭の `@` とパストークンを区別する。
時刻照会などの実行コマンドに続く `echo` はその順序で実行する。
`8f914a2` は入れ子 RESC 内の `machine LoadPlatformDescription` で
`$ORIGIN` と `$CWD` を展開する。
`target/monitor-scope-proof/` の `compare.resc` と `include.resc` を
固定 Renode の `include` および `penv run --script` から実行した。
変数の値、パストークンの出力、仮想時刻 10 ms と同期量 2 ms、
時刻照会後の出力順、入れ子 RESC から読み込む REPL パスが一致した。
`penv-frames.jsonl` はライブモニターの変数読み出しと、
明示的なグローバル名で存在しない変数を拒否した応答を記録する。

`1dc723e` は `set definition $ORIGIN/board.repl` で、代入した RESC の
ディレクトリをパスに保持する。親 RESC に戻ってから変数で REPL を読み込む
`alias.resc` を固定 Renode と通常 CLI に渡し、展開結果と時刻が一致した。
`penv-alias-frames.jsonl` でストリーム起動と起動後の変数参照でも確認した。

`88b06b7` は `mach set 0` と `mach set "0"` を区別する。固定 Renode
の Robot RPC と penv のフレーム応答を `renode-mach.jsonl` と
`penv-mach.jsonl` に記録した。添字 0、16 進数の 0、実在する名前は成功。
存在しない名前と範囲外の添字はエラーを返し、直後のローカル変数は引き続き
選択済みマシンの値を返した。

未変更の HEX と STM32F072b の 7 件を `8f914a2` で実行した。
5 件が合格し、Blinky と Button は LED 待機中に 30 秒のホスト制限に達して
`NOT_RUN` になった（`target/monitor-anchored-robot/results.json`）。
`1dc723e` で 120 秒に延ばした再計測では両方とも合格した
（`target/monitor-frozen-blinky/results.json`、
`target/monitor-frozen-button/results.json`）。再計測バイナリの SHA-256 は
`11f8d8b9222fdbcc7c616571d3aa453b5800aeaf224b04264bf5465eea03a8a4`。

マシン選択変更後も HEX は合格した（`target/monitor-selection-88b06b7-hex/results.json`）。

`9ec62e1` では、起動前の `echo` を `PENV-INSPECT` で Rust に送り、
通常の RESC ローダーで変数を評価する。マシンや実行時操作が必要な場合は
通常の起動・ライブモニターに切り替える。照会後に投入した操作が
一度も Rust に評価されない場合、その照会だけでケースを合格にしない。

固定 Renode の Robot RPC と penv の同一コマンド列で、未定義時に
`?=` が 5678 を設定し、定義済みの 1234 は維持されることを確認した。
末尾改行を除いた結果が一致した
（`target/monitor-inspect-proof/{renode,penv}.jsonl`）。
未変更の `Should Call Conditional Set`、
`Should Not Call Conditional Set`、`Should Load HEX` はすべて合格
（`target/monitor-inspect-9ec62e1-{positive,negative,hex}/results.json`）。

`a378f33` と `b9600b7` は、`log` と `lastLog` を Rust の履歴で処理する。
通常 CLI と起動前の Robot 照会が同じ記録・読み出し処理を使う。
履歴は既定で 1,000 件を保持し、上限で古い 1 件を捨てる。
Robot で起動前に投入されたログは照会時に順に評価するため、
そのタイムスタンプは照会時の評価時刻となる。

`target/monitor-lastlog-proof/history.resc` を両 CLI で実行し、
時刻を除いた INFO と WARNING の履歴を比較した。
固定 Renode と penv の 1,003 RPC コマンドも比較し、1,001 件目を
追加する前後で、それぞれ 1,000 件すべてのメッセージと順序が一致した。
`renode-rpc.jsonl` と `penv-rpc-summary.jsonl` に結果を記録した。
STM32F072b の未変更の Console Echo は合格。
MiV の `Should Print Last Logs` は `UART.MiV_CoreUART` の未対応で
起動前に停止した（`target/monitor-lastlog-a378f33-miv/results.json`）。
未変更の `Should Overflow Buffer` は `b9600b7` で合格した
（`target/monitor-lastlog-b9600b7-overflow/results.json`）。

`8976c4a` では、プラットフォームを指定しない通常の `penv run --script`
で、Robot の起動前照会と共通の Rust 評価処理を使う。未変更の
`Should Call Conditional Set`、`Should Overflow Buffer`、`Should Load HEX`
は合格した（`target/monitor-consoleonly-8976c4a-*/results.json`）。
`target/monitor-consoleonly-proof/console.resc` を両 CLI で実行し、条件付き
代入の出力、INFO と WARNING の履歴、末尾の出力順を比較した。

`a54f076` は空白をエスケープした `$ORIGIN` パスを `include`、`echo`、
`Save`、`Load` と `RestoreMonitorSnapshot` で扱う。
比較対象は `target/monitor-consoleonly-proof/escaped.resc` と
`{native-save,penv-save}/snapshot.resc`。両 CLI で同じ子 RESC が実行され、
保存・復帰後の仮想時刻 10 ms と同期量 2 ms が一致した。
固定バイナリの STM32F072b Console Echo も合格した
（`target/monitor-anchor-a54f076-uart/results.json`）。

`e403ea0` と `5c04013` は、`mach create` だけで作成したマシンを
CPU と RAM のない共通 Rust マシンとして起動する。未マップのモニター
アクセスは実際のバス処理で WARNING を記録する。
`target/monitor-consoleonly-proof/{renode,penv}-empty.normalized` で、
同じ RESC の読み出し値、読み書きの警告、仮想時刻と同期量を比較した。
機種付き RISC-V の既存アクセス経路は維持している。空マシンの
ライブフレームでは、仮想時間を進めずにログ待機が両方の警告を検出した
（`target/monitor-consoleonly-proof/empty-frames.jsonl`）。
Rust の `unhandled_access` テストは 5 件合格、6 件失敗。
変更前 `a54f076` の独立ビルドでも同じ 6 件が失敗した
（`target/monitor-empty-bus-{tests,baseline}.log`）。
未変更の `Should Log Unhandled Read From Monitor` と
`Should Log Unhandled Write From Monitor` は `5c04013` で合格した
（`target/monitor-empty-5c04013-{Read,Write}/results.json`）。
それ以前の `e403ea0` のライブ経路では未マップをゼロとして処理する
既存の短絡経路が警告を飛ばしたため、両ケースのログ待ちは失敗した。

`4b77168` は空マシンの保存・復帰を既存の Rust スナップショット経路に追加した。
通常 CLI の同じ `Save` / `Load` コマンド列と、別プロセスへの
Robot RPC `Provides` / `Requires` で固定 Renode と比較した。
両方とも復帰時刻 10 ms と同期量 2 ms が一致した。
`target/monitor-consoleonly-proof/{native-save,penv-save}/empty-output.log`
と `target/monitor-empty-snapshot-proof/{renode,penv}.jsonl` に証跡がある。
空マシン用の保存を UART 付きマシンへ読み込む要求は拒否した。
空マシンの保存は変更した未マップポリシーと保留中のイベントを拒否する。
この変更後も未変更の HEX と未マップ読み出しのログ待機は合格した
（`target/monitor-empty-snapshot-4b77168-{hex,read}/results.json`）。

`2661ccf` では、起動スクリプト中の時刻照会、バス読み出し、`echo` と
`lastLog` の応答を、ゲスト出力と別に `READY` フレームへ返す。
`target/monitor-startup-outputs-proof/commands.resc` を固定 Renode、通常
CLI、ストリーム起動の各経路で実行し、仮想時刻、同期量、読み出し値、
出力順が一致した（`renode.normalized`、`penv-cli.log`、`frames.log`）。

`9d662d9` では、開始を要求しないログ待機も Rust の履歴を調べる。
停止中に既存のログに一致した場合は仮想時間を進めず、そのログを返す。
停止中のログに一致しないまま仮想時間が必要な場合は `STOP Paused`
を返す。外部からの非同期ログ待機は今後の対応となる。
`351bbac` は Robot のバス読み出しを即時照会に送る。
固定 Renode と同じ RPC コマンド列で読み出し `0x00000000`、
警告文および時刻 0 ns が一致した
（`target/monitor-buffered-log-proof/{renode,penv}.jsonl`）。
未変更の未マップ読み書きログ待機、HEX、STM32F072b Console Echo は
すべて合格した（`target/monitor-read-351bbac-*/results.json`）。
nRF52840 の `Should Run Alarm Sample` は、時刻アサーションに到達する前の
UART 待機中に 40 秒のホスト制限に達した
（`target/monitor-startup-2661ccf-alarm/results.json`）。

`7b13622` は `s` / `p` を Rust の開始・一時停止アクションにつなぐ。
`pause.resc` を両 CLI で実行し、一時停止を挟んだ実行の仮想時刻 10 ms
と同期量 2 ms が一致した。
この変更後も未変更の HEX と STM32F072b Console Echo は合格した
（`target/monitor-pause-7b13622-{hex,uart}/results.json`）。

`9e6be15` では、UART テスターの既定値と各行・プロンプト・バイト待機の
`pauseEmulation` を Rust へ渡す。Rust は一致時に停止し、後続の待機に
進行が必要な場合は再開する。バッファ内で既に一致している場合と、その後の
仮想時間の再開を Rust テストで確認した。`313f9d7` では
`Write Line To Uart` のエコー待機も同じ処理を使い、個別指定を受け付ける。

固定 Renode `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` と同じ
Robot RPC コマンド列で、`TrivialUart` へ出力した行 `A` を
`pauseEmulation=true` で取得した。両方とも取得後の仮想時刻は 0 ns、
同期量は 100 µs だった。RPC の記録は
`target/monitor-uart-pause-proof/{renode,penv}.jsonl`。
未変更の全バイト値の正規表現、STM32F072b Console Echo、HEX は
`313f9d7` で合格した（`target/monitor-uart-pause-313f9d7-*/results.json`）。

`5804a17` は UART の無通信確認を、仮想期限に達した場合のみ
指定に従って停止する処理へつなぐ。固定 Renode と penv は
`Test If Uart Is Idle` の 20 ms 待機で同じ仮想時刻を返した。
通常 CLI の同じ Rust アクションでも 20 ms と同期量 100 µs を確認した
（`target/monitor-uart-idle-proof/`）。このビルドでは全バイト値の
正規表現と STM32F072b Console Echo が合格した。未変更の
`Terminal Tester Idle Assert Should Start Emulation` は、初期化時の
同一行に複数宣言を含む REPL が拒否されたため `NOT_RUN`
（`target/monitor-uart-idle-5804a17-idle/results.json`）。

`9a564d2` は Rust のモニターで開始・停止を保持し、`emulation IsStarted`
と開始を要求しないログ待機の判断に使う。固定 Renode と同じ
RPC コマンド列で、初期状態、停止指定なしの無通信待機後、`p` 後、
停止指定ありの無通信待機後が `False, True, False, False` で一致した
（`target/monitor-active-proof/{renode,penv}.jsonl`）。
Rust のモニター 21 件とログ 8 件は合格。
広い `monitor` フィルターで失敗した CPU ステップ 2 件は、
変更前 `5804a17` でも再現した（`target/monitor-active-baseline.log`）。
通常 CLI でも `s` と `p` の前後は両方とも `False, True, False`
（`target/monitor-active-proof/activity.resc`）。
未変更の HEX、未マップ読み出しのログ待機、STM32F072b Console Echo は
`9a564d2` で合格した（`target/monitor-active-9a564d2-*/results.json`）。
`b71cbd6` は、トップレベルのセミコロンで同一行の REPL 宣言を分ける。
プロパティの括弧内や引用符内は宣言の境界に含めず、エラーの行番号は物理行を
指す。CPU と RAM の宣言を同一行に書いた
`target/monitor-repl-decl-proof/declarations.resc` を同じ入力で比較し、
固定 Renode、penv の通常 CLI、起動フレームで RAM の読み出し値
`0x00001234` が一致した。
未変更の HEX は合格した。precise-pause の無通信ケースは新しい構文解析を
通過し、STM32L071 の `Analog.STM32_ADC` 未対応へ進んだ
（`target/monitor-repl-decl-b71cbd6-{hex,idle}/results.json`）。

`0cda938` と `3ef06f9` では、起動前の `Execute Script` と
`include` のパスを Rust モニターが解決する。存在しないファイルは
その場で拒否される。拒否応答は証跡として記録し、上流 Robot の
期待エラーのアサーションが通った場合に限りケースを合格とする。
コンソールだけのスクリプトは、起動前に出力を返し、その代入を
後続コマンドの入力にも反映する。機種を起動するスクリプトは
既存の起動キューを通る。

`target/monitor-inspect-include-proof/` の RPC 比較で、欠損ファイル 2 件の
拒否と診断、空白を含むファイルからの出力 `from script`、
その後の変数値 `42` が一致した。通常 CLI でも同じ二つの出力を確認した
（`renode-cli.normalized` と `cli.log`）。
`3ef06f9` で HEX と STM32F072b Console Echo が合格した。
欠損ファイルの二つのケースは Robot 上では合格したが、集計では当初、
捕捉された実行エラーとして `NOT_RUN` だった。
`2f4d095` で Rust が返した `REJECTED` を一般の実行失敗と区別した。
Rust モニターテスト 21 件が合格した。
`Should Fail On Loading Nonexisting Script` と `Should Fail On Command Error`
を未変更のまま再実行し、両方とも合格した
（`target/monitor-inspect-include-2f4d095-{script,include}/results.json`）。

`ee1e628` では、起動前とライブの双方で、不正な `log` および
`Save` / `Load` のシグネチャを Rust が拒否する。固定 Renode と同じ
`log invalid_commmand` と `Save invalid_value` の RPC コマンド列で、
拒否と診断を比較した。後続の正常な `log` と `lastLog` も成功した
（`target/monitor-param-proof/{renode,penv}.jsonl`）。
未変更の不正組み込み引数、不正保存引数、履歴上限、HEX の 4 ケースは
合格した（`target/monitor-params-ee1e628-*/results.json`）。
正常な保存と復帰も通常 CLI の既存コマンド列で再確認し、
仮想時刻 10 ms と同期量 2 ms が一致した。

`d7d2cc3` では、`echo` と `log` の引用符とエスケープを Rust の
共通処理でデコードする。ログレベルは固定 Renode と同じく、
整数または引用符付きのレベル名を受け付ける。
`target/monitor-string-proof/strings.resc` を通常 CLI と Robot RPC で
比較し、引用符、バックスラッシュ、INFO と WARNING の結果が一致した。
ログのホスト時刻は比較から除外した。Rust モニターテスト 21 件が合格した。
同じビルドで未変更の `Should Call Conditional Set`、`Should Overflow Buffer`、
`Should Load HEX` が合格した（`target/monitor-strings-d7d2cc3-*/results.json`）。

`db59cc5` と `7b00750` は UART テキスト応答の `Timestamp` を
Rust の仮想時計から返す。未完成行は一致を確認した時刻を使い、
完成行は UART が終端文字を受信した時刻を履歴に保持する。
Python はフレーム内の ns をミリ秒に変換する。
固定 Renode のバイナリ応答はコンストラクターで `Timestamp` を
自身に代入するため、同じ 0 の互換動作を維持した。

固定 Renode と同じ RPC コマンド列で、20 ms の時点で照会した
未完成行の `Timestamp` は両方とも 20.0 ms だった。
完成行は 20 ms で受信し、30 ms になってから取得しても
両方とも `Timestamp` は 20.0 ms だった。
証跡は `target/monitor-uart-{timestamp,completed}-proof/{renode,penv}.jsonl`。
Rust モニター 21 件と UART 関連 34 件は合格した。
仮想時計を共有した後もスナップショットの Rust テスト 3 件が合格した。
未変更の HEX、STM32F072b Console Echo、全バイト値の正規表現は
`7b00750` で合格した（`target/monitor-uart-completed-7b00750-*/results.json`）。

`56d8c4d` は単一マシンの Rust モニターで `machine GetTimeSourceInfo` と
`machine SetQuantum` を処理する。固定 Renode と同じ空マシンの
コマンド列で、`machine` と `emulation` の両方が、同期量 2 ms、
4 ms、1 ms の順で変更を反映した。
`c253a34` は `mm:ss` と `hh:mm:ss` を共通の時間引数として扱う。
`SetGlobalQuantum "00:00:00.000000050"` に続けて
`RunFor "00:00:00.02"` と `RunUntil "00:00.03"` を実行し、
通常 CLI と Robot RPC で固定 Renode と同じ 30 ms と同期量 50 ns を確認した。
証跡は `target/monitor-quantum-proof/`。Rust モニターテスト 21 件が合格した。
空マシンの保存・復帰でも両方の時刻照会が 10 ms と同期量 2 ms を返した
（同ディレクトリの `{native,penv}/clock.log`）。
未変更の HEX と STM32F072b Console Echo は合格した。
`Should Step Over Time Quantum With Multiple CPUs Active` は起動時の
ブート引数付き `sysbus LoadFdt` が未対応で時刻アサーションに到達しなかった
（`target/monitor-interval-c253a34-*/results.json`）。

`8203f4a` は、Rust 側の `sysbus LoadFdt` で v17 DTB の
`/chosen/bootargs` を更新し、更新したバイト列をゲスト RAM へ配置する。
既定では既存文字列にそのまま追記し、`false` 指定では置換する。
`chosen` が存在しない場合は作成する。disabledNodes と CPU context の
追加引数は対象外。入力ファイルは変更しない。

固定 Renode と同じコマンド列の RAM 読み出しから DTB を再構成し、
追記、置換、`chosen` 作成の値と既存の `model` が一致した。
通常 CLI と Robot 用起動フレームの読み出しも一致した
（`target/monitor-fdt-proof/`）。切り詰めた DTB は拒否した。

未変更の複数 CPU 時刻ケースは、FDT コマンドの構文解析を通過し、
`emulation SingleStepBlocking True` が未対応のため起動前に停止した。
HEX と STM32F072b Console Echo は合格した
（`target/monitor-fdt-8203f4a-*/results.json`）。

`3e4c108` は起動時に配置するバイナリのアドレスを 64 ビットで保持する。
固定 Renode の比較用ビルドには RV64 トランスレーターがなかったため、
両実行で同じ RAM アドレス `0x100000000` と FDT コマンドを使い、
Renode 側は CPU を省略した。双方から読み戻した DTB の `bootargs` と
`model` が一致した（`target/monitor-fdt-proof/{high,native-high}.resc`）。
Rust モニターテスト 21 件が合格した。
この変更後の HEX と STM32F072b Console Echo も合格した
（`target/monitor-fdt-high-3e4c108-*/results.json`）。

`7f8b5d5` は `emulation SingleStepBlocking` を共通 Rust モニターで
読み書きする。通常の自動進行は、ブロックするシングルステップのコアが
ある間停止する。明示的なステップは指定コアを進め、ブロックを解除した
場合は他コアが進んだ。2 コアの PC と仮想時間を Rust テストで確認した。
同期境界での他コアの最初の進行など、固定 Renode の非同期スケジューリング
全体はこの実装の対象外。

固定 Renode と同じコマンド列で、既定値、`False` 設定後、空マシンの
保存・復帰後が `True, False, True` と一致した。通常 CLI と
起動フレームの出力は `target/monitor-step-block-proof/` にある。
元の複数 CPU ケースは `SingleStepBlocking` を通過し、当初は
FU540 の Cadence GEM の読み込みで停止した。

`2582bdf` は、Cadence GEM の 16 ビット構成値を受け付け、
既存モデルでレジスターの 12 ビット幅に合わせる。通常 CLI と Robot が
共有する sysbus モニターは既存の GEM レジスター処理を呼ぶ。
同じ GEM 設定の `moduleId=0x1007`、`moduleRevision=0x0109` に対して、
固定 Renode と penv はレジスターの読み出しで `0x00070109` を返した。
固定 Renode の比較用ビルドには RV64 トランスレーターがないため、
レジスターの比較では Renode 側の CPU を省略した。
FU540 の元の起動スクリプトを使った penv の確認では
`READY` と CPU の実際の状態を取得した
（`target/monitor-step-block-proof/fu540-{debug,queries}.log`）。

`8aa85da` は逐次スケジューラーで `SetGlobalSerialExecution true` を
受理し、並列実行の要求は未対応とする。固定 Renode と同じ空マシンの
コマンド列で、実行後の仮想時刻 1 ms と同期量が一致した。
未変更の複数 CPU ケースは起動と設定を通過し、ゲストから
`smp: Brought up 1 node, 4 CPUs` が期限内に届かず `FAIL` となった。
時刻アサーションには未到達。FU540 の RAM から読み戻した DTB には
`earlyconsole mem=256M@0x80000000` が入っていた。
HEX と STM32F072b Console Echo は合格し、Rust モニターテスト 21 件、
シングルステップ制御 1 件、GEM 1 件も合格した
（`target/monitor-step-{gem-2582bdf,serial-8aa85da}-*/results.json`）。

`815c1a6` は、テキスト UART 待機が期限に達した場合に、Rust が計測した
開始・終了の仮想時刻と、未消費 UART バッファの末尾 512 バイトを返す。
固定 Renode と同じ RPC コマンド列で、UART に行 `A` が存在していても、
別の文字列を期限 0 で待つと失敗し、仮想時刻が進まないことを確認した
（`target/monitor-uart-tail-proof/{renode,penv}.jsonl`）。

FU540 の未変更ケースは同じ UART アサーションで `FAIL`。
実測の待機区間は 0–8 秒、UART 履歴は空だった。
期限後の e51 は `PC=0x80000004`、`MCAUSE=0x2`、`MEPC=0x80000008`
を返した。これは期限後に観測した状態で、最初のトラップを特定した値ではない
（`target/monitor-uart-tail-proof/fu540.log`）。
`target/monitor-reg-d0b5e15-*/results.json` の HEX と
STM32F072b Console Echo は合格した。

`d0b5e15` と `898d0e2` は、モニターの PC、CSR、実行モード照会を
起動時の応答に含め、ファームウェアがない構成でも実行モードを設定できる
ようにした。固定 Renode の RV32 マシンと同じ RPC コマンド列で
PC、MCAUSE、MEPC、MTVAL の数値と、`Continuous` から `SingleStep` への遷移が
一致した。固定 Renode が実行モードに付加するヘルプ文は比較に含めていない。
通常 CLI と起動フレームの結果も一致した
（`target/monitor-startup-reg-proof/`）。
Rust の UART 関連テスト 34 件とモニター関連テスト 21 件が合格した。
最終ビルドの HEX と STM32F072b Console Echo も合格した
（`target/monitor-execmode-898d0e2-*/results.json`）。

### RESC マクロのライブ実行と保存

`16c88a3` は起動スクリプト、ライブの include、ライブの宣言で
定義したマクロを同じ Rust モニターセッションに保持する。
固定 Renode と penv に同じコマンド列を送り、起動後のマクロが
変数の初期値と更新値を返すことを通常 CLI と Robot RPC で確認した
（`target/monitor-live-macro-proof/`）。

`8216b11` は復元可能なモニタースナップショットにマクロを保存する。
マクロがある場合に v4 を書き、v1–v3 も読み込む。固定 Renode と同じ
RPC コマンド列で、`Provides` の後に変数を更新し、`Requires` の後に
`runMacro $hello` が保存時の `saved value` を返すことを確認した。
penv は依存ケースを新しいセッションで開始し、スナップショットから
復元した（`target/monitor-macro-snapshot-proof/{renode,penv}.jsonl`）。
スナップショットの Rust テストは3件合格した。

同ビルドで未変更の Robot ケース `Should Load HEX` と
`Should Run Zephyr Console Echo Sample` は各1件合格した。
`Should Serialize Monitor State` は `alias run test` で
`UNSUPPORTED`。各実行は該当1件を選択し、他の2,196件は未実行。
SHA、バイナリハッシュ、実行コマンド、個別判定は
`target/monitor-macro-{hex,uart,state}/results.json` を参照。

続く起動中のマクロ対応では、マシンなしで実行できるコンソールマクロの
応答を Rust の起動照会から返し、マシンを必要とする操作は起動列に残す。
固定 Renode と同じ5コマンドで、マクロから `initial`、変数更新後に
`changed` を得た。比較は末尾の改行を除いて行った
（`target/monitor-startup-macro-proof/{renode,penv}.jsonl`）。
モニター関連の Rust テストは21件合格した。

### エイリアスと環境の復元

`2a82794` で Rust モニターにエイリアス定義、引数付き展開、
スコープ照会、再帰検出を追加した。エイリアスを含むスナップショットは
v5 を書き、v1–v4 の読み込みも維持する。固定 Renode と同じ RPC
コマンド列で、マシンを作らずに `alias say echo` と変数を保存し、
エイリアスを上書きしてから復元すると `say $name` が
`saved value` を返した。通常 CLI の RESC でも同じ出力を確認した
（`target/monitor-alias-proof/`）。

`a42b0f7` と `7351e36` は `printenv` を Rust に実装し、
実際に成功した保存とライブ照会を Robot の観測として記録する。
固定 Renode との `printenv` 比較では作業ディレクトリ、行の順序、
空行を除き、変数・マクロ・エイリアスの内容が一致した
（`target/monitor-printenv-proof/{renode,penv}.jsonl`）。
URL の表示には差があり、固定 Renode は取得済みのローカルパスを表示し、
penv は保持している URI を表示する。

固定上流の未変更 `Should Serialize Monitor State` と
`Should Deserialize Monitor State` は同じスイート実行で両方合格
（`target/monitor-env-results/results.json`）。他の2,195件は未実行。
代表ケースの HEX と STM32F072b Console Echo も個別に合格
（`target/monitor-alias-hex/results.json` と
`target/monitor-env-uart/results.json`）。Rust モニターテストは22件合格。

再現コマンド

```sh
target/renode-venv/bin/python scripts/upstream/run.py --rust-monitor \
  --suite tests/unit-tests/monitor-tests.robot \
  --case 'Should Serialize Monitor State' \
  --case 'Should Deserialize Monitor State' \
  --out target/monitor-env-repro
```

### 通常 CLI とライブモニターの Save / Load

`8261b48` は通常の RESC、フレーム起動、ライブモニターからの
`Save` を同じ Rust スナップショット書き込みに統一する。
従来の非ラップ形式は読み込み可能。名前付きの空マシンで
固定 Renode と同じ `Save`・変数更新・`Load` の操作を行い、
エイリアスが保存時の `saved value` を返すことを確認した
（`target/monitor-shared-save-proof/`）。

`2850809` は通常 CLI でもマシンを作成せずにモニター環境を保存・復元する。
`bb0d243` は RESC の Save / Load 後の代入、`echo`、`printenv` を
順に評価する。固定 Renode と同じマシンなしのコマンド列で、
更新直後に `changed`、復元後に `saved value` を出力した
（`target/monitor-save-order-proof/`）。CLI の回帰テストと既存の
起動前スナップショットテスト2件、Rust モニターテスト22件が合格。
広めの CLI 実行は15件合格、6件失敗。失敗6件は変更前の `7351e36` でも再現した
（`target/monitor-save-order-prechange.log`）。

同じ仕組みで実行後の任意の CPU・デバイス状態まで保存できる範囲は
広がっていない。RESC 内の include・マクロ・エイリアスまで
一律に遅延実行する変更も含めていない。

`bb0d243` の未変更 Robot 実行は、モニター保存・復元の2件、
HEX、STM32F072b Console Echo が合格。残り2,193件は未実行
（`target/monitor-ordered-save-robot/results.json`）。

`f7547a7` は RESC の保存境界以降にあるエイリアス宣言とマクロ宣言を
Rust の実行順に組み込む。固定 Renode と通常 CLI で、再定義後の
`printenv` は `say = log` と `echo 456` のマクロを、復元後は
`say = echo` と `echo 123` のマクロを表示した
（`target/monitor-declaration-order-proof/`）。同じ順序を CLI 回帰テストで
確認した。Rust モニターテスト22件と、未変更の保存・復元・HEX の3件が合格。
残り2,194件は未実行（`target/monitor-declaration-order-robot/results.json`）。

変数をファイル引数とする `Save $snapshot` と `Load $snapshot` は
固定 Renode が拒否したため、このバッチで動作を拡張しなかった
（`target/monitor-dynamic-path-proof/`）。
復元後の RESC でエイリアスやマクロを呼び出す箇所の動的解決は残っている。

### 復元した環境での RESC コマンド呼び出し

`7d7c550` はスナップショット境界以降の `runMacro` を実行時に
解決する。`01e9c7b` はエイリアス呼び出しも現在の Rust モニター状態で
解決する。`9e4cd90` は変数を参照する `include` / `i` /
`ExecuteScript` のパスを、実行時のモニター状態から解決する。

固定 Renode と通常 CLI に同じコマンド列を送り、保存後に上書きした
マクロ・エイリアスが `changed macro`、復元後に `saved macro` を
返すことを確認した。変数経由の include は同じ順序で
`changed include` と `saved include` を返した。
各比較はスナップショットファイル名と出力の空行を除いた
（`target/monitor-runtime-{macro,alias,include}-proof/`）。
CLI 回帰テストは相対パスの include も検証する。

最終ビルドの未変更 Robot はモニター状態の保存・復元、HEX、
STM32F072b UART の4件が合格。他の2,193件は未実行
（`target/monitor-runtime-include-robot/results.json`）。
Rust のモニターテストは22件合格した。

遅延呼び出し内の再度の Save / Load、起動後の任意のゲスト状態の復元は
今回の対応範囲外。ログ待機のサブミリ秒試行では、固定 Renode の
`LogTester.WaitForEntry` が仮想期限を整数ミリ秒へ変換することを確認した。
0.125 ms と 0.5 ms の観測で指定時刻への停止は確認できなかったため、
秒単位の精度を追加する試作差分は取り下げた
（`target/monitor-subms-log-proof/`）。

### ログ待機の期限と捕捉の開始・消去

`e46cfe7` は Robot から秒値を受け取り、固定 Renode の
`LogTester.WaitForEntry` と同じ単精度の秒から整数ミリ秒への変換を Rust
で行う。固定 Renode と penv に同じ STM32F072b のコマンド列を送り、
1.9 ms の不在判定後に仮想時刻 1 ms、`IsStarted = False` を確認した
（`target/monitor-log-rounding-proof/{renode,penv}.jsonl`）。
1 ms 未満の正の期限で Renode が開始する場合の挙動は残っている。

`a0ad88f` はログテスターの作成と履歴消去を Rust のコマンド列に置く。
固定 Renode と penv に同じ RPC を送り、作成前のログと消去したログが
見つからず、作成後のログは見つかった。通常 CLI でも同じ Rust
コマンドによる消去順序を確認した（`target/monitor-log-lifecycle-proof/`）。

未変更 Robot はログ読み出し・書き込みの2件、モニター保存・復元の2件、
HEX、STM32F072b UART の計6件合格。他の2,191件は未実行
（`target/monitor-log-lifecycle-robot/results.json`）。
Rust モニター22件とログ8件が合格。広い `monitor` フィルターでは
33件合格、2件失敗で、失敗は従来からの BusFault ステップ時のフック出力差
（`target/monitor-log-lifecycle-all-monitor.log`、
`target/monitor-snapshot-proof/baseline-step.log`）。

### UART テスターの履歴消去と結果変換

`960b96e` は `Clear Terminal Tester Report` を Rust の UART テスターへ
渡し、完了済みのテキスト行を消去する。固定 Renode と同じ TrivialUart
操作で、完了行 `A` は消去され、途中の `B` は改行を追加すると
時刻 0 ms の行として受信できた。通常 CLI でも同じ操作が成功した
（`target/monitor-clear-uart-proof/`）。バイナリモードでは
消去後も `41 0a 42` がバッファに残ることを両方で確認した。
この比較ではテスター作成の位置引数を penv 側の名前付き引数に変換した
（`target/monitor-clear-uart-binary-proof/`）。

`ba42394` は Rust 経路で UART テスター ID と既定期限を Python に
重複保持せず、成功したテキスト待機の `IsFailingString` も返す。
固定 Renode と `B` の結果オブジェクト全体が一致した。

Rust の UART テストは35件合格。未変更の最終 Robot 実行では
全256バイトのテスターケース、STM32F072b Console Echo、モニター保存・復元
の計4件が合格。他の2,193件は未実行
（`target/monitor-uart-results-robot/results.json`）。
別実行の SiFive FE310 を使う UART 2件は
`UART.SiFive_UART` で未対応（`target/monitor-clear-uart-robot/results.json`）。

### UART テスターのマシン指定と生入力

`0d00fd1` は Robot のテスター作成引数を Rust に渡し、名前付き
マシンを共通モニターで検証する。固定 Renode と同じコマンド列で
`missing` を拒否し、空白を含む `uart bench` を受理して ID 0 を返した。
停止中の `Write Char On Uart` も受理され、結果オブジェクトと実行状態
`IsStarted = False` が一致した。通常 CLI でも同じ Rust 書き込みを確認した
（`target/monitor-uart-selection-proof/`）。

`772a574` は `Write To Uart` と `Send Key To Uart` を同じ入力処理へ
渡す。Rust は固定 Renode の `TerminalTester.Write` に沿って、UTF-16
コード単位から下位8ビットをUARTに渡す。固定 Renode と同じ入力で
両方の RPC が成功し、返却オブジェクトが一致した。penv の通常 CLI
とRPCで `é` とキー255をレジスターから `0xE9`、`0xFF` と読み戻した
（`target/monitor-uart-raw-proof/`）。

停止中の配送時刻には差がある。固定 Renode の `STM32F7_USART.WriteChar`
は受信スレッドへキューし、停止中の即時読み出しでは `0x00` を返した。
penv は同条件で入力を直ちに読み出せた。

Rust UART テスト35件とモニターテスト22件が合格。
マシン指定変更時の未変更 Robot は全256バイト、STM32F072b Console Echo、
モニター保存・復元の4件合格
（`target/monitor-uart-selection-robot/results.json`）。
生入力変更後も全256バイトと Console Echo は合格した。
`Should Passthrough Characters Via Uart Hub` は
`uart_hub_nucleo_h753.resc:8` の2台目の作成で起動前停止し、未実行。
最終実行は2件合格、2,195件未実行
（`target/monitor-uart-raw-robot/results.json`）。

### 停止中の STM32F7 USART 入力の配送

`71273a5` は、クロック周波数が分かる F7 レイアウトの USART で
ボーレート分周値が 0 の場合にも、テスター入力を仮想時間で配送する。
固定 Renode の `STM32F7_USART.BaudRate` は分周値を最低 1 として扱う。
既存のフレーム間隔を優先し、未設定時はこの下限を使う。
テスター以外の入力経路の分周値 0 の処理は維持する。

固定 Renode と penv で同じ STM32F072b のコマンド列を実行した。
CPU を停止し、USART に `X` を送信した直後の RDR は両方 `0x00`。
仮想時間を 100 µs 進めた後は両方 `0x58` で、`IsStarted` は
`False` だった。通常 CLI の RESC でも同じ値を確認した。
配送の最初のクロック刻みはこの比較では計測していない
（`target/monitor-uart-rx-schedule-proof/`）。

Rust の UART テスト36件が合格。未変更の Robot は全256バイト、
STM32F072b Console Echo、モニター保存・復元の4件合格。
残り2,193件は未実行（`target/monitor-uart-rx-schedule-robot/results.json`）。

### マシン選択の起動前検証と保存・復元

`6f6b589` は `mach set` と Robot の `Execute Command` のマシン引数を
起動前の Rust セッションで検証する。無効な選択はその場で拒否し、
起動コマンド列に残さない。固定 Renode と penv に同じコマンドを送り、
無効な番号・名前のあとも正しい名前・番号を選択し、変数 `ready` を
読み出せた。通常 CLI の正しい番号・名前による選択も確認した
（`target/monitor-mach-selection-proof/`）。

`c8dab66` は単一マシンセッションで登録と選択を区別する。
`mach clear` 後はグローバル変数へ戻り、`mach set 0` でローカル変数へ
戻る。ライブ状態で選択解除中の `sysbus` は固定 Renode と同様に拒否した。
選択解除を保存すると v6 を書き、v1–v5 の読み込みも維持する。
固定 Renode と penv の保存・復元列では変数の表示が
`local`、`global`、`global`、`local` の順で一致し、通常 CLI でも
同じ順序だった（`target/monitor-mach-clear-reference/`）。
Rust モニターテスト22件と CLI スナップショット回帰テストが合格。

複数マシンの実行と、すべてのデバイスコマンドを対象にした
選択解除時の検証は残っている。

未変更の最終 Robot 実行はモニター保存・復元、HEX、STM32F072b Console Echo
の4件合格。残り2,193件は未実行
（`target/monitor-mach-clear-robot/results.json`）。
起動前検証の変更単体では全256バイトのUARTケースも合格した
（`target/monitor-mach-selection-robot/results.json`）。

`c28d7ce` は RESC のスナップショット操作以降にある `mach set` と
`sysbus` の操作を実行時の選択状態と変数で解決する。
固定 Renode と通常 CLI で、選択を解除して保存し、再選択した場合と、
復元後に再選択した場合の両方で同じ `0x00000000` を読み出した。
penv の負例は、選択を解除したスナップショットを復元し、再選択なしで
`sysbus` を呼ぶと終了コード 1 になることを確認した
（`target/monitor-selection-ordered-proof/`）。
Rust モニターテスト22件と起動前メモリ保存・復元の CLI テストが合格。

固定 Renode でも同じ解除状態を Load した後、再選択なしの `sysbus`
を拒否した（`target/monitor-selection-cleared-negative/renode.jsonl`）。
未変更の Robot は保存・復元、HEX、STM32F072b Console Echo の4件合格。
残り2,193件は未実行（`target/monitor-selection-ordered-robot/results.json`）。

### 復元後の CPU と machine コマンド、UserState

`5827a3e` と `0262433` は、スナップショット操作以降の CPU コマンドと
`machine UserState` などの操作を実行時に解決する。固定 Renode と
通常 CLI で、選択解除状態の復元直後に `cpu IsHalted` と
`machine UserState` が拒否され、`mach set 0` 後は CPU の停止状態と
設定した UserState を読み出せることを確認した
（`target/monitor-cpu-selection-proof/`、
`target/monitor-machine-properties-proof/`）。
この段階の未変更 Robot は保存・復元、HEX、STM32F072b Console Echo が
4件合格（`target/monitor-machine-properties-robot/results.json`）。

`8cf2555` は空でない `machine UserState` を Rust のスナップショット
v7 に保存し、v1–v6 も読み込む。固定 Renode、通常 CLI、Robot RPC に
同じコマンド列を送り、上書き後の `changed value` が `Load` 後に
`saved value` に戻ることを確認した
（`target/monitor-user-state-snapshot-proof/`）。
Rust モニターテスト22件と CLI のスナップショット回帰テスト3件が合格。

既存 v6 の成果物を通常 CLI で復元し、マシン選択と変数が維持されることも
確認した（`target/monitor-user-state-snapshot-proof/legacy-v6.log`）。
最終ビルドの未変更 Robot は同じ4件が合格、残り2,193件は未実行
（`target/monitor-user-state-snapshot-robot/results.json`）。
検証は単一マシンと対応済みスナップショット形式の範囲。

### 数値表示形式と単一ステップ設定の復元

`72c9c75` は Rust のモニタースナップショットに数値表示形式を保存する。
十進と併記は v8、既定の十六進は従来の形式を使い、読み込みは v1–v8
に対応する。固定 Renode、通常 CLI、Robot RPC で共通のコマンド列を実行し、
十進で保存してから十六進に変更すると `0x00000000`、復元後は `0` に
戻ることを確認した（`target/monitor-number-mode-proof/`）。

続けて `emulation SingleStepBlocking` を両方向で比較した。
保存時と読み込み直前をそれぞれ True / False と False / True にし、
どちらの組み合わせでも固定 Renode と penv の通常 CLI は読み込み後に
True を返した。この動作は既存の Rust 実装と一致した
（`target/monitor-single-step-snapshot-reference/`）。

Rust モニターテスト22件と CLI のスナップショット回帰テストが合格。
未変更の Robot は保存・復元、HEX、STM32F072b Console Echo の4件合格。
残り2,193件は未実行（`target/monitor-number-mode-robot/results.json`）。

### ログレベルの起動前検証とライブ照会

`55bdcc4` は単一引数の `logLevel` を起動前に Rust で検証し、
無効な変更を起動列へ入れない。ライブの `lastLog` 応答は観測として記録し、
合否は元の Robot アサーションに従う。

固定 Renode と penv の RPC では、`logLevel 4` が拒否された後も
有効な設定を行えた。保存時と復元直前のレベルを 0 と 2 で入れ替えた
両方向の比較で、`Load` 後も直前のレベルが効いた。直前が 2 なら
警告だけが表示され、0 なら情報も表示された。
通常 CLI でも警告だけが表示される経路を確認した
（`target/monitor-log-level-restore-proof/`）。
Rust モニターテスト22件が合格。

未変更 Robot はログバッファの上限、未処理のバス読み出し・書き込みの
ログ、および STM32F072b Console Echo の4件が合格した。
残り2,193件は未実行（`target/monitor-log-level-validation-robot/results.json`）。

### Save / Load をまたぐ RESC include

`fe7326b` は、スナップショット操作の後にあるリテラルの RESC
`include` も実行順に評価する。子スクリプトの変数変更を外側の
`Save` より先に反映する問題を解消し、子スクリプト内での
`Save` / `Load` を同じ Rust セッションで扱う。
`2caee58` はライブモニターの `include` にも同じ経路を使う。

固定 Renode と通常 CLI の比較では、空白をエスケープしたパスを使い、
外側と子スクリプトのスナップショットで `changed`、`saved`、
`changed` の順に値を得た。ライブ Robot RPC でも同じ結果。
通常 CLI では相対パスでも同じ順を確認した。相互 include が保存境界を
またぐ負例は、再帰上限のエラーで停止した
（`target/monitor-literal-include-restore-proof/`、
`target/monitor-literal-include-nested-proof/`）。
Rust モニターテスト22件と CLI スナップショット回帰テストが合格。

子スクリプト内で上書きしてから子のスナップショットを読み込む場合も
同じ結果になった（`nested-load-*.jsonl` と `nested-load-cli.log`）。
未変更 Robot は保存・復元、HEX、STM32F072b Console Echo の4件合格。
残り2,193件は未実行（`target/monitor-literal-include-nested-robot/results.json`）。
検証対象は RESC と対応済みの単一マシンのスナップショット。

### 共通 Rust パーサーでの runMacro

`153eada` はライブおよび遅延実行のマクロ本体を Rust の RESC
パーサーで処理し、本体内の変数変更と `Save` / `Load` を扱う。
固定 Renode、通常 CLI、Robot RPC に同じコマンド列を送り、
外側とマクロ内の別々の保存・復元で `changed`、`saved`、`changed`
の順に値を得た（`target/monitor-macro-save-proof/`）。

`f74fda9` は起動時のマクロも同じパーサーに通し、呼び出し先の
ソース行と基準ディレクトリを維持する。固定 Renode、通常 CLI、
Robot RPC で、ネストした起動時マクロによる変数変更を確認した
（`target/monitor-macro-unified-proof/`）。
再帰エラーを含む Rust モニターテスト22件が合格。
比較したのは行単位のマクロと対応済みスナップショットの範囲。

未変更 Robot は保存・復元、HEX、STM32F072b Console Echo の4件が合格。
残り2,193件は未実行（`target/monitor-macro-unified-robot/results.json`）。
CLI のスナップショット回帰テストも合格した。

### セミコロン区切りのモニターコマンド

`93cf4fb` と `808778f` は、マクロ本体、RESC、ライブモニターの
コマンドを引用符外のセミコロンで分割する。同じコマンド列を固定 Renode、
通常 CLI、Robot RPC に送り、`alpha;beta` が1つの文字列として
出力され、続く変数更新の結果 `done` も得られることを確認した
（`target/monitor-{macro-semicolon,command-chain}-proof/`）。
同じ行に `Save`、変数の上書き、`Load` を置いた比較でも、上書き後と
復元後の値は `after`、`before` の順で一致した
（`target/monitor-chain-snapshot-proof/`）。

`324c751` は、マシンを必要としないコマンド列の応答を起動照会から
返す。固定 Renode と penv の RPC の出力順が一致し、前の呼び出しの
出力を再送せず、新しい変数値を次の呼び出しで参照できた
（`target/monitor-preboot-chain-proof/`）。
マシンを必要とする起動前のコマンド列は通常の起動列に残る。

Rust モニターテスト23件が合格。未変更の Robot は保存・復元、HEX、
STM32F072b Console Echo の4件合格。残り2,193件は未実行
（`target/monitor-preboot-chain-robot/results.json`）。

### 復元した変数による仮想時間の制御

`8712a74` は、RESC の保存境界以降にある仮想時間コマンド
`SetGlobalQuantum` / `SetQuantum`、`RunFor`、`RunUntil` の引数を
実行時に解決する。固定 Renode、通常 CLI、Robot RPC で、
保存後に上書きした変数を使うと仮想時刻と同期量が 3 ms、復元後の変数を
使うと 1 ms になることを確認した。相対時間の `RunFor` と絶対時刻の
`RunUntil` の両方で一致した。比較対象は仮想時刻と同期量
（`target/monitor-restored-time-proof/`、
`target/monitor-restored-run-until-proof/`）。

Rust モニターテスト24件が合格。未変更の Robot は保存・復元、HEX、
STM32F072b Console Echo の4件合格。残り2,193件は未実行
（`target/monitor-restored-time-robot/results.json`）。

### 復元した変数を使う UART 待機

`81896a8` は、保存境界以降の UART テスター作成、既定の期限設定、
各 UART 待機の引数を実行時の Rust セッションで解決する。
TrivialUart で期限の変数を 2 ms から 6 ms に上書きして `Load` し、
復元後の変数を使ったアイドル判定が仮想時刻 2 ms で完了することを
固定 Renode、通常 CLI、Robot RPC で確認した。固定 Renode と Robot RPC には、
復元後の変数から取得した値を UART キーワードの引数として渡した。
通常 CLI では、明示的な期限とテスターの既定期限を使った不在確認が
いずれも 2 ms で完了した（`target/monitor-restored-uart-proof/`）。

Rust モニターテスト24件と UART テスト36件が合格。
未変更の Robot は全256バイトの UART ケース、STM32F072b Console Echo、
保存・復元の4件が合格し、残り2,193件は未実行
（`target/monitor-restored-uart-robot/results.json`）。

### 復元した変数を使うログ待機とメッセージ

`3da35dc` は保存境界以降のログ待機を実行時に解決し、
`WaitForMonitorLog` と `AssertAbsentMonitorLog` の期限に変数を使える
ようにする。固定 Renode、通常 CLI、Robot RPC で、不在判定が変数の
上書き後は 6 ms、復元後は 2 ms で完了することを確認した。
固定 Renode と Robot RPC には読み出した変数値を期限引数として渡した
（`target/monitor-restored-log-proof/`）。

`afab9d5` は `log`、`logLevel`、`lastLog` も保存境界以降に
順に評価する。同じコマンド列を実行し、上書き後には
`[WARNING] Script: changed marker`、復元後には
`[INFO] Script: original marker` が出ることを固定 Renode、
通常 CLI、Robot RPC で確認した。比較はスクリプトのログ行の本文と重要度
（`target/monitor-restored-log-message-proof/`）。

Rust モニターテスト24件とログテスト8件が合格。
未変更の Robot はバス読み出し・書き込みのログ、保存・復元、
STM32F072b Console Echo の5件合格、残り2,192件は未実行
（`target/monitor-restored-log-robot/results.json`）。

### 復元した文字列による UART 入力

`d86519d` は、保存境界以降の `WriteTesterUart` を実行時の
Rust セッションで評価し、復元した文字列変数を入力に使えるようにする。
STM32F072b で `X` を保存して `Y` に上書きし、復元した `X` を送った。
固定 Renode、通常 CLI、Robot RPC のすべてで受信レジスターは直後に
`0x00`、仮想時間を 100 µs 進めると `0x58` を返した。
固定 Renode と Robot RPC には復元後に読み出した文字を引数として渡し、
通常 CLI では `WriteTesterUart $character` を使った
（`target/monitor-restored-uart-input-proof/`）。

Rust モニターテスト24件と UART テスト36件が合格。
未変更の Robot は全256バイトの UART ケース、STM32F072b Console Echo、
保存・復元の4件合格、残り2,193件は未実行
（`target/monitor-restored-uart-input-robot/results.json`）。

### 復元した変数による UART の照合

`0566e64` は UART のテキスト・バイナリ入力と待機パターンに同じ
Rust の文字列変数解決を使う。失敗文字列の登録・解除も保存境界以降は
実行時に解決する。

固定 Renode、通常 CLI、Robot RPC で、上書きしてから復元したパターンで
同じ受信データを照合した。テキスト `X` の結果オブジェクトと、
バイナリ `58 0a` の結果が一致した。復元した `X` を失敗文字列として
登録した場合は、別の行を待つ操作が `X` の受信で失敗した。
固定 Renode と Robot RPC には読み出した値をキーワードの引数で渡し、
通常 CLI は `$expected` を直接使った
（`target/monitor-restored-uart-{pattern,byte-pattern,failing-pattern}-proof/`）。

Rust モニターテスト25件と UART テスト36件が合格。
未変更の Robot は全256バイトの UART ケース、STM32F072b Console Echo、
保存・復元の4件合格、残り2,193件は未実行
（`target/monitor-restored-uart-pattern-robot/results.json`）。

### 復元した変数によるログ照合

`b004e09` はログ待機と失敗文字列の登録・解除で文字列変数を使える
ようにした。通常 CLI とライブモニターは同じ Rust 処理で UTF-8 の
パターンをログテスターへ渡す。

固定 Renode、通常 CLI、Robot RPC で、上書きして復元した
`µ original` と正規表現 `µ .+` が受信ログに一致した。
復元値を失敗文字列として登録すると、別のログを待つ操作は失敗した。
固定 Renode の例外表示では `µ` が省かれた。待機の合否と照合結果を
比較し、Robot RPC では復元後に取得した文字列をキーワードに渡した。
通常 CLI と penv のライブモニターは変数を直接指定した
（`target/monitor-restored-log-{pattern,regex}-proof/`）。

Rust モニターテスト26件とログテスト8件が合格。
未変更の Robot はログ2件、保存・復元2件、STM32F072b Console Echo の
計5件が合格。残り2,192件は未実行
（`target/monitor-restored-log-pattern-robot/results.json`）。

### 引用符付きスナップショットパス

`516f604` は共通 Rust モニターの `Save` / `Load` で引用符付き
パスを受理する。固定 Renode、通常 CLI、Robot RPC で空白を含むパスを
使用し、上書き後の `changed` が復元後に `saved` に戻ることを確認した。
固定 Renode では通常の `$snapshot` は文字列を設定しても拒否された
（`target/monitor-snapshot-path-forms/`）。
既存の `$CWD/...` 形式は固定 Renode と通常 CLI で同じ結果だった
（`target/monitor-snapshot-anchor-proof/`）。

Rust モニターテスト26件、引用符付き相対パスを使う CLI 回帰と
既存のメモリ保存・復元 CLI テストが合格。
未変更の Robot は保存・復元、HEX、STM32F072b Console Echo の4件合格。
残り2,193件は未実行（`target/monitor-snapshot-quoted-robot/results.json`）。

### 保存・復元時の作業ディレクトリ

`dda5afc` は `Save` / `Load` の `$CWD/...` を、共通 Rust モニターが
実行する時点のディレクトリで解決する。二つのディレクトリに同じ名前で
別の状態を保存し、ディレクトリを切り替えて読み込むと、固定 Renode、
通常 CLI、Robot RPC のすべてで値が `second`、`first` の順に戻った。
旧ビルドの通常 CLI では `second` が二度返ることを確認した
（`target/monitor-anchor-runtime-proof/`）。

Rust モニターテスト26件と CLI 回帰テスト2件が合格。
未変更の Robot は保存・復元、HEX、STM32F072b Console Echo の4件合格。
残り2,193件は未実行（`target/monitor-anchor-runtime-robot/results.json`）。

### 無効な Load の起動前検証

`e946746` は、保留中の実行がなければ、存在しないスナップショットを
Rust が起動前に拒否する。`a994a64` は同じ経路でセッションのメタデータと
マシンのヘッダーを確認する。読み込み時のペイロード検証は従来どおり
実際のロードが担当する。

固定 Renode と penv の Robot RPC で、存在しないファイルと不明なヘッダーの
両方を拒否し、後続の `echo` は `ready`、`IsStarted` は `False` を
返した。通常 CLI は壊れたファイルを終了コード 1 で拒否した。
保留中の `Save` が作るファイルを `Load` する正常例も、
固定 Renode と penv の RPC で `saved` に復元できた
（`target/monitor-{missing-load,invalid-load,pending-save-load}-proof/`）。

Rust モニターテスト26件と作業ディレクトリを切り替える
CLI スナップショット回帰テストが合格。
最終ビルドの未変更 Robot は保存・復元、HEX、STM32F072b Console Echo の
4件合格、残り2,193件は未実行
（`target/monitor-invalid-load-robot/results.json`）。

### 無効な Save の起動前検証

`6497675` は、保存先の親ディレクトリが存在しない場合を起動前に
Rust が拒否し、実際の保存処理でも同じ検査を使う。
固定 Renode と penv の Robot RPC で、起動前・ライブの両方で
`Save` が失敗し、続く `echo` は `ready`、`IsStarted` は `False` を返した。
通常 CLI はディレクトリを作成せず終了コード 1 を返した
（`target/monitor-invalid-save-proof/`）。

`15ca480` は保存先そのものがディレクトリの場合も、書き込み前に拒否する。
固定 Renode と penv の RPC では拒否後も `ready` を読み出せた。
通常 CLI でもエラーを確認し、ディレクトリ内の確認ファイルは保持された
（`target/monitor-directory-save-proof/`）。

Rust モニターテスト26件と作業ディレクトリを切り替える
CLI スナップショット回帰テストが合格。
未変更の最終 Robot は保存・復元、HEX、STM32F072b Console Echo の4件合格。
残り2,193件は未実行（`target/monitor-directory-save-robot/results.json`）。

### 通常ファイル以外を指す Load の拒否

`399e448` は、通常ファイル以外のロード先を起動前に Rust が拒否する。
ライブのロードと `RestoreMonitorSnapshot` の起動経路も通常ファイルを
確認する。固定 Renode と penv の RPC で、ディレクトリを指定した
`Load` が拒否され、続く `echo` は `ready` を返した。
通常 CLI ではディレクトリと FIFO を終了コード 1 で拒否した。
FIFO については penv の挙動をタイムアウト付きで確認した
（`target/monitor-directory-load-proof/`）。

Rust モニターテスト26件と CLI スナップショット回帰テスト2件が合格。
通常ファイルへのシンボリックリンクを使う正常例では、固定 Renode と
penv の RPC の両方で `saved` に復元できた
（`target/monitor-symlink-load-proof/`）。
未変更の Robot は保存・復元、HEX、STM32F072b Console Echo の4件合格。
残り2,193件は未実行（`target/monitor-directory-load-robot/results.json`）。
