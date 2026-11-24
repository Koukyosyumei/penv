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
