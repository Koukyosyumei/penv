# ARM 上流回帰の調査

対象は STM32F072B Discovery、Armv6-M の Cortex-M0/M0+、および使用する STM32 の周辺機器。既存受入の対象と結果は README の「固定した上流 ARM 回帰の対応表」に記録している。

| 参照ツリー | 固定コミット |
| --- | --- |
| Renode | `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` |
| `src/Infrastructure` | `09ac7c563095c5410258ecc1b1a6b841a013db85` |
| Infrastructure の `src/Emulator/Cores/tlib` | `c04522a9c886ba743c77a86b0c879103a2ef46d0` |

## 適用範囲と集計

[ケース台帳](arm-upstream-cases.tsv)の候補を固定上流の Robot/NUnit フィクスチャで突合した。STM32F072B Discovery の実イメージ、Cortex-M0/M0+ に移せる命令・例外シーケンス、CPU 非依存の共有周辺機器・バス・端末・時間制御の契約を対象とする。現在の実装の有無は適用判定に使わない。別ボードの ELF、固定の周辺機器アドレス、ISA 固有のレジスタや複数コア構成に依存する場合は、必要な構成を各行に記録した。ISA の根拠は Armv6-M の命令と例外モデル、および固定 tlib の実装を参照した。[Armv6-M Architecture Reference Manual](https://developer.arm.com/documentation/ddi0419/latest/)

| 出典 | 対象の分母 | 上流等価合格 | 部分移植 | 未達 | 対象外の台帳行 |
| --- | ---: | ---: | ---: | ---: | ---: |
| Renode Robot | 113 | 61 | 41 | 11 | 239 |
| Renode Infrastructure NUnit | 134 | 89 | 23 | 22 | 9 |
| 合計 | **247** | **150** | **64** | **33** | **248** |

- 分母は対象の実行構成。例外復帰のキーワード引数、CPU の停止・リセット順序、NUnit の `Values` と `Range`、個別に初期化する CRC 入力などを展開した。同じ構成の `Repeat(5)` や一連の操作中の繰り返し観測は1行とした。対象外の ISA 専用スイートの内部ループは、必要構成を記した親ケースの行にまとめた。
- 重複のキーは出典・ソースパス・ケース名・固定パラメータ・構成。上流の Hello World ELF と HEX はローダと刺激が違うので独立に数える。ローカルの Console Echo プロンプトと Blinky のクロック操作は補助回帰で、対応する上流ケースの数を増やさない。F072 の同じ HEX を使う CPU1/CPU2 選択は別の Cortex-M4 2コア構成。
- `equivalent pass` は適用構成で上流と同じ主要刺激・観測を実行できたケース。Button はホスト側待ち時間の差を残しているため `partial` とした。Console Echo は CR と未完了行の待機を揃えた。M55 の Fault 状態レジスタ、F4 のフラッシュ消去範囲や TIM2 のファームウェア観測を省いた移植も部分扱い。
- `unmet` は対象のまま、等価な回帰がない項目。独立した BusFault の優先度や置換不能な前提を必要とするケースは `out of scope`、同じ `LDR`/`STR` と保存フレームを HardFault で再現できるケースは未達に残した。上流の `Ignore` 属性付き NUnit ケースは対象外。
- 集計はこの台帳に含まれる固定リビジョンの候補に対するもの。NUnit/Robot ランナーの上流全件実行ではなく、penv の Rust・CLI テストと実イメージによる等価性監査の結果。

今回の NMI 回帰では、上流のリセットループ `0x20`、NMI ハンドラー `0x100`、`NOP` 後の `0x102`、`BX LR` 後の `0x20` と初期 SP `0x800` を Armv6-M で検査する。上流と同じ `nvic SetPendingIRQ 2` で保留を設定し、例外受理時に SCB ICSR の NMIPENDSET がクリアされることも確認する。TrustZone 行の `BXNS` は Armv8-M Security Extension に依存する。

## 再現

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
cargo test --locked
cargo test --locked renode_nmi_bx_lr_restores_interrupted_thread
cargo test --locked renode_resc_set_pending_irq_sets_nmi
scripts/verify-stm32f072.sh ../renode /tmp/新規証跡ディレクトリ
```

上記スクリプトは固定 Renode ツリーの公開イメージ7点を取得し、サイズ・SHA-1 と照合してから STM32F072 の ELF・HEX 回帰を実行する。

固定 Infrastructure のチェックアウトまたは固定コミットのアーカイブを展開し、次のコマンドで再生成と集計を照合する。

```sh
python3 scripts/inventory-arm-upstream.py ../renode /path/to/infrastructure |
    cmp - docs/arm-upstream-cases.tsv
awk -F '\t' 'NR>1 { total[$1,$6]++ } END { for (key in total) print key, total[key] }' \
    docs/arm-upstream-cases.tsv
```

## 残件

対象のうち部分移植38件と未達127件が残る。MPU 起因のアンスタック Fault、HardFault のベクター取得中の割り込み到着、CPU pause/halt とリセットマクロの順序、Robot UART テスター、GPIO とタイマーの NUnit 条件が未完了。既存の公開イメージ回帰と部分移植は個別の証拠として維持する。

## HardFault・ロックアップ

出典は固定 Renode の `tests/unit-tests/arm-cortex-m-bus-fault.robot` と、同ディレクトリの `BusFaultingPeripheral.cs`。各対応は [ケース台帳](arm-upstream-cases.tsv) に記録した。上流は Cortex-M55 で実行するため、以下は Armv6-M に共通する刺激と観測条件の移植。

| 上流の刺激 | Armv6-M の観測 |
| --- | --- |
| ゼロ初期化されたベクターで2ステップ | HardFault の入口を経て PC が `0xEFFFFFFE` に固定される |
| `0x200` で `ldr r2, [r0]`、`r0=0x100000` | `0x340` の HardFault へ遷移。r1/r2 と Fault 命令のスタック PC を保持 |
| HardFault 中に同じアドレスへ再アクセス、さらに10ステップ、NMI 保留 | 命令は進行しない。NMI が `0x380` に入り、`0xFD8` にロックアップ PC を積む。リセットで初期状態を復元 |
| ベクターベース `0x100000` に対して `SetPendingIRQ 2` / `3` | ベクター読み出しでロックアップし、SP `0xFE0` と対象の例外番号を保持する。LR は Armv6-M の `0xFFFFFFF9` |
| HardFault 中の NMI で `movs r0, #0; str r0, [sp, #28]; bx lr` | 破損した復帰フレームでロックアップし、背景の HardFault と SP `0xFE0` を保持 |
| リセット用テーブルのオフセット `4` または `0` だけで読み出し Fault | PC `0xEFFFFFFE`、SP・IPSR は0、LR は `0xFFFFFFFF` |
| 補助検証として HardFault ハンドラーがスタック PC を修正して `BX LR` | 次命令を実行し、スレッドの SP と例外番号を復元 |

テスト用バスは上流モックと同じ RAM アドレス範囲と失敗オフセットを指定する。本体ではバスの未割り当て・アラインメント異常と未定義 `UDF` をゲスト Fault として扱い、ホスト I/O エラーと未実装の命令はエラーとして返す。CLI はロックアップ中も仮想実行上限まで進み、終了時にロックアップ PC を報告する。

```sh
cargo test --locked renode_invalid_reset_state
cargo test --locked hardfault
cargo test --locked renode_fault_on_initial_msp
cargo test --locked runs_cortex_m0_from_vector_table
```

Cortex-M55 の CFSR/HFSR/BFAR、BusFault 有効化、ITSTATE、TrustZone、FP 条件は Armv6-M の結果には含めない。実行中のベクター遅延到着は未移植。

## 例外スタック書き込み・アンスタック時の Fault

固定上流の `tests/unit-tests/arm-cortex-m-bus-fault.robot` から、`0x100000` の Fault 領域と MSP `0x100020` を使う6ケースを Armv6-M に移植した。ケース名と Rust テストの対応は[台帳](arm-upstream-cases.tsv)を参照。

| 刺激 | 確認した結果 |
| --- | --- |
| スレッドから NMI または IRQ 16 に入る際にスタック書き込みが失敗 | SP は `0x100000`。NMI は起動して HardFault を保留。IRQ 16 の場合は HardFault が先に起動し、IRQ 16 は保留のまま |
| HardFault 中の NMI でスタック書き込みが失敗。ベクターも失敗する組み合わせを含む | NMI と HardFault がともにアクティブになり、SP `0x100000`、PC `0xEFFFFFFE` でロックアップ |
| 同じ NMI でスタックのオフセット `0x14` だけが失敗する補助ケース | 上流 tlib の `v7m_push` の順序で先に書いた xPSR と PC が残る。Fault を起こしたワードと下位アドレスのワードは初期値を保持 |
| HardFault 中の NMI が `ldr r0, =0x100000; msr msp, r0; bx lr` を実行 | 復帰時の読み出しが失敗して NMI を終了。SP `0x100020` までフレームを消費し、HardFault の状態でロックアップ |
| 読み出しだけが失敗するスタックで、スレッドから起動した NMI が `nop; bx lr` を実行 | SP `0x100000`、EXC_RETURN `0xFFFFFFF9` と保存フレームの8ワードを保持して HardFault へテールチェーン。Fault を解除すると同じフレームでスレッドに復帰 |

上流の Cortex-M55 は一部のケースで BusFault を有効化する。Armv6-M では HardFault に昇格するため、その例外番号とベクターを検証した。CFSR、HFSR、SHCSR、ITSTATE、FP、TrustZone の観測は対象外。通常の例外復帰で保留済みの割り込みへ進むテールチェーンは以下で検証した。ベクター取得中に新たに到着する割り込みの優先順位評価は残件。Fault 注入は Rust テスト内で行い、公開 STM32F072 イメージは既存動作の回帰に用いる。

```sh
cargo test --locked stacking_fault
cargo test --locked unstack_fault
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-stack-fault-evidence
```


## Console Echo の等価性

固定上流の `tests/platforms/STM32F072b.robot` の `Should Run Zephyr Console Echo Sample`、`src/Renode/RobotFrameworkEngine/UartKeywords.cs`、Infrastructure の `TerminalTester.cs` を照合した。公開 ELF は `stm32f072b_disco--zephyr-echo.elf-s_544096-541b7b153ff0a9b30489bd1cc34e693c0ac8b9ea`。

| 上流の操作 | penv の RESC |
| --- | --- |
| USART1 の入力案内を完了行の部分一致で待つ | `waitForUart sysbus.usart1 "Start typing characters to see them echoed back" timeout=8s` |
| `Write Line To Uart` が送る `Echo working?` + CR | `sysbus.usart1 WriteLine "Echo working?"`。末尾は既定の `0d` |
| `waitForEcho=true` が完了行または未完了行を部分一致で待つ | `waitForUartPrompt sysbus.usart1 "Echo working?" timeout=8s`。出力が `?` で終わり、後続の CRLF まで進めないことを公開 ELF 回帰で検査 |

上流と同様に、LF 終端のテキスト照合では CR と NUL を無視する。生の標準出力には両方を残す。8秒は上流の既定仮想時間で、penv では待機ごとの上限と全体の `--max-steps` の残量の小さい方を使う。RESC の各待機は同期的に進むため、Robot 側のホスト並行実行のスケジューリングは再現対象に含めていない。補助の CRLF・完了行回帰と ADC 回帰も維持した。

追加プローブで見つかった受信フレーム時間と RE 解除の差は、以下の USART 時間制御で対応した。上流の複数 TerminalTester ID、バイナリモード、正規表現は別の残件。

```sh
# verify-stm32f072.sh で取得済みの assets を使用
PENV_RENODE_ROOT="$(realpath ../renode)" PENV_RENODE_ASSETS=/path/to/evidence/assets \
    cargo test --locked --test cli upstream_stm32f072_console_echo -- --ignored
cargo test --locked text_waits_ignore_nul
cargo test --locked renode_stm32_usart
```


## USART の受信フレームと IDLE

出典は固定 Renode の `tests/peripherals/STM32_UART.robot` の6ケースと、固定 Infrastructure の `STM32_UART.cs` および `STM32F7_USART.cs`。上流テストの旧 STM32_UART レジスタ配置を F072 の `CR1=0x00`、`BRR=0x0c`、`ISR=0x1c`、`RDR=0x24` に置き換え、同じバイト、時刻、フラグを検査した。固定 F072 REPL の周波数も USART へ引き渡す。周波数の記載がない簡易 REPL や BRR がゼロの構成では即時入力を維持する。

- 16 MHz、BRR `0x10`、1 Mbaud で `0x12`、`0x34` の各フレームは9 µsで未完了、10 µsで RXNE。8ビットワード内にパリティを含めても10 µs。
- 最終フレーム到着後の IDLE は9 µsでは未設定、10 µsで設定。9 µsの時点で次フレームを開始すると元の期限では設定されない。F072 の IDLEIE、ICR.IDLECF と WFI 中の NVIC 保留も検査した。
- RE の解除で受信途中のデータを破棄する。RE または UE が無効な間の入力は、再有効化後も届かない。BRR をゼロにした際は受信途中の `0x12` を排出してから即時入力の `0x34` が届く。
- F072 の FIFO オーバーラン・パリティエラー、実行時の RCC クロック切替、個別の TX フレーム時間はこの移植では検証していない。F072 の上流 REPL が使う `STM32F7_USART` では IDLE のビットがタグ扱いで、時間・フラグの比較条件は旧 STM32_UART の Robot フィクスチャを用いた。

```sh
cargo test --locked stm32_usart
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-uart-timing-evidence
```


## Cortex-M0 の例外復帰と CPUWAIT リセット境界

固定上流の `arm-cortex-m-signals.robot` と Infrastructure の `CortexM.cs` を参照した。CPUWAIT はアサートだけでは実行中の CPU を止めず、リセット時に保持状態へ入る。保持中は PC/SP のベクター取得を保留する。解除後の同期点でベクターを取得するか、保持中に PC が明示設定された場合はその PC を保持する。同期点ではゲスト命令を実行しない。REPL の `isCpuWaitSignalSet` と RESC の `cpu CpuWaitSignal Set/Unset`、`cpu Reset`、`machine Reset`、`emulation RunToNearestSyncPoint` で再現できる。同期点は penv の次の仮想ステップで扱う。

- 0.1秒ごとにアドレス `0x4` のカウンターを観測し、アサート後もリセット前は増加し、リセット後は停止することと、信号がリセットをまたいで維持されることを確認した。RA8M1 固有の ELF とベクター値を検査する行は部分移植として残し、同じ遅延取得を Cortex-M0 のローカルベクターと公開 F072 Hello World ELF の両方で検査した。
- 固定上流の `arm-cortex-m-exception-return.robot` にある Thread の `BX` では `0xffffffb8` と `0xfeffffff` をリテラルからロードし、同じ着地点を確認した。NMI ハンドラーの `BLX LR` はフレームを取り出さず、Armv6-M の EXC_RETURN に対応する `0xfffffff8` へ分岐する。NMI のアンスタック Fault から HardFault ベクターの値 `0xffffffb0` を PC に読み込むケースでは、MPU の代わりに未マップのスタックを用いた。
- 例外復帰時、宛先コンテキストより優先度の高い保留割り込みへ保存フレームの読み出し前にテールチェーンする。読み出しを失敗させたフレームを保持し、NMI → IRQ16 → IRQ17 の順に処理した後、同じフレームから Thread に戻れることを検査した。ベクター読出し Fault の移植では IRQ16 の保留と単一フレームを保持したまま HardFault に遷移することを検査した。

RA8M1 のリセットマクロとホスト側 pause/halt、M33 MPU、M55 固有の Fault 状態レジスタ、ベクター取得中の新規割り込み到着は残件。公開 ELF の CPUWAIT 保持中の無出力と解除後の UART 完了マーカーは通常の RESC/CLI で検証した。

```sh
cargo test --locked cpuwait
cargo test --locked exception_return
cargo test --locked renode_thread_bx
cargo test --locked renode_exception_vector
cargo test --locked renode_irq_vector
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-exception-boundaries-evidence
```


## STM32F072 CRC 設定と計算

固定 Infrastructure の `CRCTests.cs` と `STM32_CRC.cs` を参照した。CR または POL の書き込みを保留し、次の CRC_DR 読み出しまたは書き込みで、その時点の INIT と POLYSIZE から計算状態を初期化する。INIT だけの書き込みは進行中の計算状態を変更しない。固定上流の F072 REPL は `configurablePoly: true` を指定する。

上流データの逐次更新1構成、入出力反転3構成、初期設定3構成、多項式チェック値8構成を移植した。入力、初期値、多項式、反転条件、期待値を上流と揃え、反転には CRC_CR の設定を用いた。上流設定で最終 XOR を行う3構成では、STM32 が CRC_DR に返した値にソフトウェア側で同じ XOR を適用して比較する。

`CRCConfig` コンストラクタの無効幅 0/33 と、幅7に収まらない多項式の例外を観測する3構成は未達。F072 のレジスタには無効幅を指定できる符号化やホストのコンストラクタ例外を返す経路がない。台帳の分母を維持した。31ビット構成は従来どおり対象外。

```sh
cargo test --locked stm32_crc
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-crc-verification
```


## SystemBus のブロック転送

固定 Infrastructure の `SystemBusTests.cs` 382–489行と、`SystemBus.cs` の `ReadBytes` / `WriteBytes` を参照した。RESC の `sysbus WriteBytes [0x11, 0x22] 0x1000` と `sysbus ReadBytes 0x1000 2` で実行する。読み出し結果はアドレス昇順の16進リストで、UART 待機履歴から分離する。ホストからのブロック転送では読み出しの穴をゼロで埋め、書き込みの穴は捨てる。CPU の幅付きアクセス、ELF/HEX ローダーは既存の境界判定を使う。

- `0` と `0xc0000000` の各16バイト領域を使い、全体が領域内、片端だけが領域内、全体が穴、領域が書き込み範囲に含まれる場合の5構成を移植した。上流の5構成は同じ MappedMemory の2登録を作るが、観測するのは書き込みと同じアドレス範囲のため、移植では独立した2領域を使う。
- オフセット50–149と200–299に RAM またはバイト幅のテスト用周辺機器を置き、4組の組み合わせでオフセット0からの350バイト転送と穴のゼロ埋めを確認した。
- RESC の解析と CLI 表示、ゼロ長の読み出し、32ビットアドレス空間端を越える書き込みの事前拒否も検査した。

隣接領域をまたぐ `FindBytes` は下記で検証した。CPU コンテキスト付き転送、`onlyMemory` オプション、バス上の別名登録はこの移植の範囲外。

```sh
cargo test --locked bulk
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-systembus-verification
```


## SystemBus のバイト列検索

固定 Infrastructure の `SystemBusTests.cs` の `ShouldFindBytePattern` と、`SystemBus.cs` の `FindBytes` を移植した。上流と同じように隣接する1 MiBのメモリを2領域登録し、バイト列 `00 01 02 03 04 05 06 07` を `0x00000000` と `0x000ffffc` に書く。2件がアドレス順に見つかり、後者は領域と上流の1 MiB検索バッファの境界をまたぐ。

RESC の `sysbus FindBytes "00 01 02 03 04 05 06 07"` は引用符付き16進文字列とバイトリストを受け付ける。開始・終了アドレスは任意で、指定時は両端を含む。マップされたメモリのみ走査し、隣接領域の間では途中一致を維持し、穴や MMIO では途切れる。検索結果はアドレスリストとして標準出力へ返し、UART の待機履歴には入れない。

CPU コンテキスト付き検索と動的な別名登録は残件。未割り当てアクセス方針とタグの分割は以下を参照。

```sh
cargo test --locked systembus
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-findbytes-verification
```


## SystemBus の未割り当てアクセスとタグ

固定 Infrastructure の `SystemBusTests.cs` 41–129行の8ケースと、`SystemBus.cs` の `Tag`、`ReportNonExistingRead`、`ReportNonExistingWrite` を参照した。RESC に `sysbus ReadByte`、`sysbus WriteByte`、`sysbus UnhandledAccessBehaviour`、`sysbus Tag` を追加した。未割り当て領域の既定のモニター読み出しはゼロ。指定可能な方針は `ThrowException`、`ThrowExceptionIfNotTagged`、`DoNotReport`。設定した方針はゲストのスカラー・データアクセスにも使い、従来のゲスト既定動作は設定を省略したときに維持する。

テストはアドレス `0xabcd1234` の既定値とエラー、タグの既定値 `0x42`、`DoNotReport` を指定した場合も例外タグが優先されること、`0x1000`–`0x10ff` の親タグを `0x1010`–`0x101f` の子で分割したときに左右のエラー属性が残ることを確認する。通常のタグは未割り当て領域に対して適用する。ブロック転送の穴の扱いは前節のとおり。

周辺機器自身が返す `CommandError` オブジェクトの同一性、上流のログ報告フィルター、マップ済み周辺機器を置き換えるタグ、ワンショットタグ、SVD メタデータは未移植。

```sh
cargo test --locked systembus
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-systembus-tags-verification
```


## Cortex-M の同期ロード・ストア Fault

固定 Renode の `arm-cortex-m-bus-fault.robot` 322–343行、1267–1271行を Armv6-M へ移植した。単一ステップでは `0x100000` で失敗する周辺機器への `LDR` / `STR` と、`ThrowException` を設定した `0x200000` への未割り当て `LDR` を実行した。R1/R2、SP `0xfe0`、保存 PC `0x200` を照合し、失敗したストアが値を変えないことも検査した。

連続実行ではベクターテーブルから起動して100ステップを実行する。失敗命令のアドレスを R10 に残し、例外ハンドラーがスタックから読み取った PC を R11 に保存して WFI で待つ。Cortex-M0 の命令符号化に合わせ、SP 相対ロードの結果を低位レジスタ経由で R11 に移す。M55 の独立した BusFault と CFSR/BFAR/HFSR が必要な等価検証は Cortex-M0 ではできないため、5構成とも部分移植とした。

保存フレームの観測には通常の RESC で `sysbus ReadDoubleWord` を使える。今回、16・32ビットのスカラー読み書きも追加し、CLI のリトルエンディアン値と幅を検査した。ホスト側の周辺機器例外オブジェクトの同一性を検証する NUnit ケースは未達。

```sh
cargo test --locked renode_precise
cargo test --locked --test cli renode_resc_systembus
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-precise-verification
```


## Cortex-M の条件コードと状態レジスタ

固定 Renode の `it-status.robot` 112行の `Should Evaluate Condition Codes Properly` を通常の RESC で移植した。Cortex-M0 の `MOVS`、`CMP`、`LSLS`、`ADDS` で上流と同じオペランドと4つの観測点を作る。先行する `MOVS` と `LSLS` のフラグは直後の上流相当の演算で更新され、観測値に影響しない。

`cpu Step 2/1/1/3` ごとに `cpu EvaluateConditionCode 0/2/4/6/8/10/12/14` と `cpu GetRegister "CPSR"` を実行し、上流の32判定と状態値 `0x81000000`、`0x61000000`、`0x21000000`、`0x71000000` を照合した。モニターの `CPSR` 名で Cortex-M の xPSR を返す。`cpu Step` は既存の仮想実行ステップを使用し、例外・スリープ境界の Renode デバッガーのステップ動作は別の検証項目。ITSTATE を必要とする残りのケースには Armv7E-M が必要。

```sh
cargo test --locked renode_resc_cortex_m_evaluates_condition_codes
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-conditions-verification
```


## RESC の NVIC Frequency と SysTick

固定 Renode の `arm-cortex-m-interrupts.robot` の SysTick 2ケースを、通常の RESC で移植した。Cortex-M0 の WFI ループを使い、`nvic Frequency 100`、CSR の無効化、CVR=0 と RVR=10 の両順序、CSR=5 を上流と揃えた。CVR は有効化直後と、続けて `RunFor "0.09"`、`RunFor "0.01"`、`RunFor "0.01"` を実行した直後に読み、いずれも `10 → 1 → 10 → 9` となることを検査した。CPU が WFI に入っても仮想時間上の SysTick は進む。周波数の範囲は既存の penv SysTick と同じ正の32ビット整数。

共通機能のうち、CPU pause/halt の上流ケースは壁時計経過中の自律実行と同期量子の観測が必要なため、この同期 RESC 実装では未達に残した。DWT と組み合わせるクロック状態のケースも部分移植のまま。

```sh
cargo test --locked renode_resc_systick
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-systick-resc-verification
```


## RESC の UART テスター選択

固定上流の `uart-tester.robot` 21–96行と `TestersProvider.cs` のテスター登録・選択を移植した。RESC では `CreateTerminalTester sysbus.usart1` でゼロからの ID を取得し、`SetDefaultTester 0` / `SetDefaultTester null` で既定を設定・解除する。`WaitForLineOnUart "文字列" testerId=0 timeout=1` の ID とタイムアウトは任意。明示 ID、既定 ID、唯一のテスターの順で選択する。複数あって選択されていなければエラー。同じ USART の再登録は同じ ID を返し、テスターの履歴は登録時から始まる。通常のポート指定の待機は独立した履歴を使う。

選択・エラー検証の上流9ケースで、FE310 フィクスチャの `ZEPHYR` 出力を F072 型の2 USART の片側に注入して同じ選択条件を再現した。公開 Hello World ELF では、USART2 を既定にして明示 ID が USART1 の起動出力に一致すること、明示 ID を省略すると USART2 が1秒でタイムアウトすることを検査した。

バイナリモードは下記を参照。正規表現、失敗文字列の登録、複数マシンや STM32F7_USART 以外へのテスター接続は残件。

```sh
cargo test --locked uart_tester
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-uart-testers-verification
```


## UART テスターのバイナリ待機

固定 Renode の `uart-tester.robot` 97–162行と、Infrastructure の `TerminalTester.cs` のバイナリモードを参照した。`CreateTerminalTester sysbus.usart1 binaryMode=true` で生バイト履歴を作り、`WaitForBytesOnUart "2a 2a 2a 2a 0d 0a 73 68" matchStart=true timeout=0` で照合する。ID とタイムアウトの選択はテキスト待機と共通。成功時に一致バイト列を16進リストで返す。履歴の上限は既存の UART 履歴と同じ64 KiB。同期 RESC の待機は一致時点で終了するため、上流フィクスチャの `defaultPauseEmulation=true` も受け付ける。

テキストとバイナリの不一致2ケース、CRLF をまたぐ一致、隣接パターン、先頭一致、`00`–`ff` の全256値の計6ケースを、STM32 USART に固定上流と同じバイト列を送って検証した。失敗時のカーソル維持は即時照合と履歴の Rust テストで検証した。FE310 のシェルと TrivialUart の代わりにこの注入経路を用いる。公開 F072 Hello World ELF でも `Hello World!` の直後から残りの文字列と CRLF を先頭一致で照合した。

バイナリ正規表現と失敗文字列の登録は下記を参照。他の UART モデルへのテスター接続は未達。

```sh
cargo test --locked renode_binary_uart_history
cargo test --locked renode_resc_uart_testers
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-uart-binary-verification
```


## UART テスターの正規表現と失敗文字列

固定 Renode の `uart-tester.robot` 後半4ケースと `TerminalTester.cs` を移植した。`WaitForBytesOnUart "pattern" treatAsRegex=true` はバイト単位で照合し、ドットは改行にも一致する。一致本体とキャプチャを順に16進リストで返す。未参加グループは `null`。履歴は一致の末尾まで消費する。上流と同じ `00`–`ff` を3回投入し、3個のパターン、2個のキャプチャと残った `fe ff` を照合した。

テキストテスターの `RegisterFailingUartString "pattern" [testerId=番号] [treatAsRegex=true]` は完了行の失敗パターンを登録する。待機時は成功パターンに先立って失敗パターンを検査し、該当行を報告する。`UnregisterFailingUartString` は同じ文字列と照合方式の登録を解除する。Tock 専用の出力は同じ文字列を STM32 USART に投入して、正規表現による検出、指定テスターでの失敗、解除後の成功を検査した。

正規表現には Cargo.lock で固定した Rust `regex` を使用する。.NET の後方参照、先読み・後読み、テキスト待機自体の正規表現、複数マシンは今回の対象外。

```sh
cargo test --locked renode_resc_uart_testers
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-uart-patterns-verification
```


## STM32 比較タイマー

固定 Infrastructure の `ComparingTimerTests.cs` と `STM32_Timer.cs` を参照した。REPL の `initialLimit` の最上位セットビットに合わせて CNT・ARR・CCR の幅を設定し、ゼロ幅になる設定を拒否する。例えば `initialLimit: 0x1ff` は9ビット。この変更は既存 F072 の16・32ビット設定にも同じ規則を適用する。

上流の1 MHz と各経過時間を維持し、周期 `limit` を STM32 の `ARR=limit-1` に、比較値を CCR1 に対応させた。比較イベント数は CC1IE を有効にし、CC1IF をクリアしながら数える。65536カウント周期の途中で CCR1 を 65535 から0へ変える場合、CNT をゼロにしても CCR1 を保持する場合、ゼロへの書き込み後に次の比較を発火する場合、現在値を CNT に書き直しても余分な比較イベントが発生しない場合の4ケースを検証した。

`ShouldThrowOnCompareHigherThanLimit` は専用ホスト API の例外を観測する。STM32 の CCR1 は幅でマスクされる MMIO レジスタのため、ARR を超える値をホスト例外として見せる互換動作は未達のまま。入力キャプチャは別の残件。

```sh
cargo test --locked stm32_timer
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-compare-timer-verification
```

## STM32 タイマーの周波数と更新イベント

固定 Infrastructure の `LimitTimerTests.cs` と `LimitTimer.cs` を参照した。RESC の `sysbus.timer2 Frequency 4000000` で REPL に登録したタイマーのカウンター周波数を変更できる。変更時に固定上流の `ClockEntry.With` と同様に CNT を保持してクロックの端数を破棄し、`machine Reset` で REPL の周波数に戻す。0 Hz はエラーになる。CLI 回帰は同じタイマーを走らせたまま 1 MHz から 4 MHz に変更し、変更直前の CNT が1、変更後の0.20 µs で1、さらに0.80 µs で5、リセット後の1 µs で1となることを確認する。

周期を `ARR=period-1`、イベントの有効化を UIE、イベントの観測を更新 IRQ に対応させた。100 Hz・周期100000の昇順と降順を100秒間毎秒観測し、1 Hz・周期100の範囲を99、100、101秒で確認した。1 Hz・周期10の降順で、UIE 有効時の10秒時点の割り込みと、無効時の11秒間の割り込み抑止を確認した。この6件を等価合格とした。長い仮想間隔の Rust 回帰は、通常のバスの1ステップと同じタイマー更新処理にまとめて仮想刻みを渡す。ファームウェアや RESC の実行経路は従来どおり1ステップずつ進む。

1 MHz・10 µs と 10 MHz・1 µs はどちらも周期4で2回の割り込みを観測した。終了時は STM32 の CNT が1、上流の汎用 LimitTimer の Value は2なので2件を部分移植とした。途中で降順に反転する残り1件は、汎用モデルがゼロ到達時にコールバックし、STM32 は次のアンダーフローで更新を通知する。汎用モデルの境界を再現する経路が必要。

```sh
cargo test --locked stm32_timer
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-limit-timer-verification
```

## Cortex-M の Pause、Halt、仮想時刻

固定 Renode の `tests/unit-tests/cpu-pause-halt.robot` の開始・Pause・Halt・併用を移植した。併用の4順序を含む7構成を CLI で検証する。リセットベクター `0x101` と `0x100` の NOP・分岐ループを Cortex-M0 で実行する。固定ステップの `emulation RunFor` で開始し、上流の PC フックと仮想時刻の観測を `cpu PC` と TIM2 の CNT 読み出しに対応させた。上流と同じ `Pause`、`Resume`、`IsHalted`、`IsPaused`、`emulation SynchroniseQuantum` を使う。同期コマンドを処理する時点で、先行する実行区間は完了している。

単一コアの Pause 中は、スケジュール済みイベントも含めて仮想時刻が進まない。Halt 中は命令と例外の受理を止め、タイマー、UART、割り込みのサンプリングを続ける。Pause と Halt は独立し、Resume 後も明示的に解除するまで Halt は継続する。

リセットの8構成も同じ回帰に含めた。実行中・Halt・Pause・両方の4状態から、単独 `cpu Reset` と `machine Reset` をそれぞれ実行する。CPUWAIT が解除された状態の単独リセットは Halt を解除して Pause に入り、PC はリセットベクター、TIM2 の CNT は停止前の値を維持する。CPUWAIT がアサートされた状態のリセットは既存の同期解除経路を使う。マシンリセットは Halt・Pause を解除し、上流と同じ `macro reset` の `sleep 0.5` と `log "Machine resetting"` を次の命令の前に実行する。CLI でログ、リセット直後の PC、ループ復帰とタイマーの進行を順に確認した。

自動リセットマクロの対応範囲は RESC の明示的な `machine Reset` における `sleep` と `log`。任意のモニターコマンドやゲストによるリセット時のマクロは引き続き対象外。GPIO の接続解除群は動的な周辺機器の登録解除と接続先の列挙が必要で、現在の固定 REPL 配線からは同じ刺激を与えられない。

```sh
cargo test --locked renode_resc_cortex_m_pause_halt_and_virtual_time
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-cpu-flow-verification
```

## GPIO 出力と入力への配線

固定 Infrastructure の `GPIOTests.cs` 4件を移植した。接続前の空の一覧、制限なしの受信先と入力番号2、5入力の受信先への入力番号2、入力番号10の拒否と上流のエラー文を、REPL と同じ接続処理で検証した。Rust では受信先の識別子にポートのベースアドレスを使う。

`Miscellaneous.Button` は接続先なしで登録でき、`-> gpioA@2 | gpioB@3` で複数の STM32 GPIO 入力に配線できる。接続先はポートとピンの組で重複を除き、全入力のレベルを更新してから EXTI などへ反映する。CLI 回帰では押下・解放による両ポートの IDR と、16入力ポートの範囲外エラーを確認する。

`GPIODetachingTests.cs` のうち、入力1に接続した受信先を登録解除した際に送信元の接続先一覧が空になる2件を等価合格とした。通常の RESC では `sysbus Unregister sysbus.gpioB` または `machine UnregisterFromParent sysbus.gpioB` を使う。STM32 GPIO の MMIO、ボタンと LED の配線、EXTI レベル、タイマーの GPIO 入出力を除去または再評価する。取り除いた子ボタンへの予約済みイベントは、他のボタンに影響しない。

複数の送信元にまたがる選択的な切断と、受信先の重複排除も Rust 回帰で確認した。上流と同じ汎用送信元の複数番号付き出力や送受信を兼ねるデバイスを使う2件は部分移植とした。残りは番号付き出力の列挙、汎用デバイスの連鎖、外部 GPIOConnector とマシンのライフサイクルが必要。

```sh
cargo test --locked renode_gpio_endpoint_registration_and_input_bounds
cargo test --locked renode_gpio_unregister_receiver_detaches_only_its_endpoints
cargo test --locked renode_resc_gpio_button_fans_out_to_distinct_pins
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-gpio-endpoints-verification
```

## CRC の設定検証

固定 Infrastructure の `CRCTests.cs::ShouldThrowArgumentException` で使う多項式 `0xff` を、同じ幅7、0、33で検証した。幅7では多項式の桁あふれ、0と33では幅の範囲外を型付きエラーで返す。設定検証は STM32 CRC がデータレジスタの読み出しや書き込みで保留設定を適用する際にも使う。

`POLYSIZE` や `POL` の書き込み、構成レジスタの読み出しは先に行える。確定時の構成エラーはバスの未マップエラーと区別して RESC に返し、有効な多項式を書き直せばその時点の INIT から再開する。0や33ビットは STM32 の `POLYSIZE` から選べないため、固定上流と同じ入力を Rust 側の設定検証に直接渡す。31ビットの CRC 計算には汎用エンジンが必要で、STM32F072 の範囲外。

```sh
cargo test --locked stm32_crc
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-crc-config-verification
```

## STM32 PWM のポート間配線と4チャネル入力捕捉

固定上流の `STM32F4_Discovery.robot` の PWM Capture キーワードと同じ `gpioPortA: { 6 -> gpioPortD@12 }` を REPL で記述できる。GPIO の出力モードまたはタイマーの代替機能が駆動したレベルを、受信側ポートの入力へ渡してから EXTI、タイマー捕捉、LED を更新する。配線から受けたレベルはボタン入力と別に保持し、受信しただけのピンは新たな駆動元にならない。送信側か受信側ポートを登録解除すると、その配線を外す。

Rust 回帰では PA6 から PD12 へ 1 kHz・33% の PWM を配線し、単一の捕捉ペアと CCR1～CCR4 の同時捕捉を別構成で確認した。2回目の立ち上がり・立ち下がりと前周期のサンプルから周期1000カウント、パルス幅330カウントを得た。上流と同じ周波数の許容差 ±25 Hz、デューティ比の許容差 ±1 パーセントポイントで検証する。

F4 専用 ELF が UART へ報告する周波数・デューティ比は F4 のプラットフォーム対応が必要なため、両ケースを部分移植として記録した。F072 の実イメージ検証では上流 REPL を拡張し、通常の RESC で PA6 → PD12 の捕捉レジスタを確認してから公開 Hello World ELF の UART 完了行を確認する。

```sh
cargo test --locked renode_stm32_pwm_input_capture_routes_period_and_pulse_to_gpio
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-pwm-gpio-verification
```

## Cortex-M の PC フックからのモニターログ

固定上流の `hooks.robot` の `Should Parse Monitor in CPU Hook` を Cortex-M0 に移植した。上流と同じ `cpu AddHook` の現在 PC 式と `monitor.Parse` の `log` コマンドを使い、CPU を実行した時点で `message from the cpu hook` を観測する。RESC では有限の `emulation RunFor` を使う。数値アドレスの別フックと、Pause 中にログが発生しないことも確認する。フックのログは標準エラー、ゲスト UART は標準出力へ出す。

同じ `hooks.robot` の SysTick ケースも移植した。Cortex-M0 の RAM と NVIC を用意し、上流と同じ `nvic WriteDoubleWord 0x14 0`、PC 0x10 の `machine.SystemBus.WriteDoubleWord(0xe000e010, 0x1)`、PC 0x20 の `machine.PauseAndRequestEmulationPause()` を実行する。起動用バイナリにリセットベクターを持たせ、PC 0 から実行する点とフックのアドレスは上流と同じ。 `logLevel -1 nvic` で有効化した診断と `Machine paused` の順序を比較する。停止は命令境界で適用し、残りの実行予算で再開できることも確認した。フックの実行範囲はログ、32ビットバス書き込み、停止の3操作。汎用 Python フックと別 ISA のウォッチポイントは引き続き対象外。汎用 LimitTimer の降順反転も専用モデルのゼロ到達イベントが必要なため、STM32 タイマーのアンダーフローとは別の残件。

```sh
cargo test --locked renode_resc_cortex_m_hook
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-pc-hook-verification
```

## 32ビット空間外のグローバルメモリ登録

固定上流の `array_memory_global_above_32bit.resc` に合わせ、REPL の単一グローバルメモリ登録に 64 ビットのベースアドレスを許可した。32 ビットゲストのアクセス範囲と既存 MMIO を保持し、範囲のオーバーフロー、重複、32 ビット境界をまたぐ低位メモリ登録を拒否する。高位メモリは RESC の `sysbus ReadByte/ReadWord/ReadDoubleWord` と対応する書き込みで参照できる。バルク転送とローダーのアドレスは従来の32ビット範囲。

CLI 回帰では同じ `0x8_0000_0000`、RAM 各32 KiB と仮想時間 0.001 秒を使う。Cortex-M0 向けに置き換えたループが `0x40000000`～`0x40006ffc` にアドレスを順に書き、高位の値と独立であることを確認した。上流の Cortex-R8 固有の CPU コンテキスト登録と JIT の無効化ログは再現しないため部分移植。SystemBus の単一・複数登録2件は、上流と同じ登録範囲と読み出し値を事前に設定した ArrayMemory で検証した。名前付きの複数バス領域と幅変換のケースには、登録ごとのアクセスディスパッチが引き続き必要。

```sh
cargo test --locked renode_resc_global_array_memory
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-high-ram-verification
```

## SystemBus の CommandError 伝播

固定 Infrastructure の `SystemBusTests.cs::ShouldPropagateBusAccessExceptionFromPeripheral` を移植した。上流と同じ `0x1000` から `0x100` バイトのテスト用周辺機器を使い、32ビット読み出しの `CommandError` が同じ原因オブジェクトを保持することを `Arc::ptr_eq` で確認した。隣接する未マップアドレスの `AddressError` と区別する。STM32 CRC の設定エラーは共通の `CommandError` を通して診断を返し、元の型付きエラーを原因として保持する。

汎用タイマーの残件には、`ComparingTimer.Compare` の上限チェックと、降順に反転した `LimitTimer` のゼロ到達イベントが必要。STM32 の CCR の幅マスクと更新境界に影響するため、専用の汎用タイマーモデルを用いるケースとして保留した。

```sh
cargo test --locked renode_systembus
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-command-error-verification
```

## ホスト UART 用の PTY 作成

固定 Renode の `tests/unit-tests/host-uart.robot` と同じ `emulation CreateUartPtyTerminal "tmp" "<一時ファイル>" true` を Linux で実行できる。端末作成だけの RESC はプラットフォームをロードした時点で実行できる。CLI 回帰では既存ファイルに対する拒否と強制置換を、Rust 回帰ではプロセス内でリンクを開いて PTY のキャラクタデバイスであることと、破棄時のリンク削除を確認した。

固定シナリオは端末の作成までを観測する。ゲスト UART と PTY の接続・双方向転送、Linux 以外での PTY 作成は今後の範囲。Mi-V 固有周辺機器の REPL は移植せず、同じ端末作成刺激を Cortex-M0 の REPL で検証した。

```sh
cargo test --locked renode_host_pty_force_replaces_temporary_file
cargo test --locked renode_resc_opens_host_uart_pty_without_booting
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-host-pty-verification
```

## マシンのリアルタイムクロックモード

固定上流 `real-time-clock-mode.robot` の共有コマンド `machine RealTimeClockMode` と `machine RealTimeClockDateTime` を実装した。既定の Epoch は 1970-01-01、HostTimeLocal と HostTimeUTC は単一のマシン作成時刻が起点。途中でモードを切り替えても Cortex-M の仮想経過時間を維持し、秒単位で表示する。HostTimeLocal は64ビット Linux のローカルタイムゾーンを使う。CLI 回帰では固定上流と同じ Epoch → HostTimeLocal → HostTimeUTC → Epoch を試し、Rust 回帰では仮想時間による閏日の繰り上がりを確認する。

上流の AmbiqApollo4 と MAX32650 の両 RTC はマシンのモード変更を購読し、自身の `CurrentDateTime` を更新する。固定上流の STM32F4 RTC は独立したカレンダーを使う。STM32 RTC のカレンダー動作を維持し、デバイス固有の同期と両時刻の一致を未達条件として2件とも部分移植にした。

```sh
cargo test --locked rtc
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-rtc-modes-verification
```

## 同期境界での遅延アクション

固定 Infrastructure の `TimeSourceTests.cs` の遅延アクション4件を、初回同期と 10 tick ごとの境界を持つ単一クロックの Rust フィクスチャに移植した。初回の要求は tick 10、第10ラウンドの要求は tick 100、tick 30 の指定は tick 30、tick 31 の指定は tick 40 で処理する。指定時刻より前には実行せず、次の同期境界でアクションを消費するため、同じアクションは再実行されない。

STM32F072 の CPUWAIT 解除に同じ処理を使う。CPUWAIT が保持中に解除された場合は次の実行境界でベクターを取得する。境界の前に再アサートするかリセットを行うと保留を取り消す。ブロック中の時間付与と記録・再生を観測する時間ソースケースは未達。

```sh
cargo test --locked renode_time_source
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-sync-actions-verification
```

## 時間ソースの完了量子

固定上流の `TimeSourceTests.cs` から、5つの sink、同じ量子、異なる量子、端数が残る量子、切断後に進まない sink の5ケースをまとめて移植した。上流の初回同期では経過時間が進まない。以後、付与された時刻までに完了できる量子だけを進め、未完了部分は次回の付与に持ち越す。Rust フィクスチャでは上流と同じ量子とラウンド数を使い、同期回数、sink のラウンド数、仮想経過時刻を確認する。実行時の Cortex-M は同じ時間ソースを1ステップ量子で使い、CPUWAIT や周辺機器イベントへ時刻を供給する。

この実装は直列の完了量子を扱う。TimeHandle のブロック・報告と記録・再生には別の実装が必要。ホスト Stopwatch を測る AdvanceImmediately のケースは、個別に指定された性能工程に残す。

```sh
cargo test --locked renode_time_source
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-time-grants-verification
```

## TimeHandle の付与と継続報告

固定 Infrastructure の `TimeHandleTests.cs` から、1000 tick の付与・要求・継続報告を共有する8ケースを移植した。要求前の待機は未進行を返し、付与を受け取った後は報告があるまで待機状態になる。継続報告後に待機すると、完了と使用した量子を返す。通常の `RunFor` と `cpu Step` は直列の付与・要求・報告を通り、RESC の実行予算は従来どおり実際に経過したステップ数で更新する。

待機が直ちに戻る2ケースは等価合格。別スレッドの実際の待機と解除を含む6ケースは、同じ順序の `Pending` / `Ready` と戻り値を確認した部分移植。ホストスレッドの待機・解除、Unblock、残余量子の再付与、無効化・破棄は残件。

```sh
cargo test --locked renode_time_handle
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-time-handle-verification
```

## TimeHandle の Break と操作順序

固定 Infrastructure の `TimeHandleTests.cs` から Break と順序検証の14件を追加した。同じ1000 tick の付与を使い、Break が使った時間を初回の待機で返し、2回目の待機でゼロを返すことを確認した。順序違反11件は上流のデバッグ時アサーションに相当する Rust のエラーを検証する。これらと即時の待機2件を等価合格、先行する別スレッドの待機が必要な1件を部分移植とした。

通常の `RunFor` と `cpu Step` では、量子を使い切らず停止した場合に Break として実経過を計上する。次の RESC コマンドは、新しい実行区間として未使用の継続予算を使う。残った量子を同じハンドルで Unblock する操作は次節に記録する。

```sh
cargo test --locked renode_time_handle
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-time-break-verification
```

## TimeHandle の Unblock と残余量子

Break の報告後、消費時間は最初の Wait に渡し、残った量子はハンドルに保持する。受信側が再要求してから Unblock すると、その残余だけを再提示する。次の完了または中断の報告に再開フラグを付け、一度の Wait で消費する。固定上流と同じ1000 tick の再提示と、全量消費時の0 tick を検証した。補助回帰では750 tick を消費し、残る250 tick だけを再取得する。

ブロック中の新規付与の拒否を等価合格とし、実スレッドの待機と解除を含む4件を部分移植に更新した。ここでの Unblock は直列ハンドルの API。RESC の `RunFor` 区間をまたぐ実行は、従来の明示的な Resume と新しい実行予算を使う。ラッチと並行スレッドの待機・解除は残件。無効化と破棄は次節に記録する。

```sh
cargo test --locked renode_time_handle
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-time-unblock-verification
```

## TimeHandle の有効状態と破棄

固定 Infrastructure の `TimeHandleTests.cs` から状態遷移11件を一括移植した。有効状態・付与元の活動状態・破棄を別の条件として扱い、要求に対して未到着の付与、無効による拒否、0 tick の付与を区別する。無効時に付与された量子は実行せず完了として返す。500 tick を残して Break したハンドルを無効化すると残り500 tick を報告し、再有効化後は新しい量子を受け取る。破棄後は新規付与を拒否する。

この状態モデルで即時応答と順序違反を確認した6件は等価合格。ホストスレッドが実際に待機・再開する5件は部分移植。ラッチ中の無効化・再有効化や実スレッドの同期は残件。

```sh
cargo test --locked renode_time_handle
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-time-lifecycle-verification
```

## TimeHandle のラッチと残余を含む進捗

固定 Infrastructure の TimeHandle 4件を一括移植した。1000 tick の継続報告で100 tick を残し、次の要求で1100 tick を取得して進捗を報告する。既に計上した100 tick を除き、仮想時刻は2000 tick になる。このケースを等価合格とした。

ラッチ中に有効状態の変更を要求すると、最後のラッチを解除するまで保留する。無効中の量子は Wait で完了として報告する。Break の報告待ち2件と、ラッチ中の再有効化1件は直列の `Pending` / `Ready` と戻り値を確認した。固定上流の別スレッドでの実際の待機と起床が残るため、この3件は部分移植。RESC の実行は従来どおり単一スレッドの量子を使う。

```sh
cargo test --locked renode_time_handle
scripts/verify-stm32f072.sh ../renode /tmp/penv-arm-time-latch-verification
```
