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
| Renode Robot | 113 | 8 | 37 | 68 | 239 |
| Renode Infrastructure NUnit | 134 | 2 | 0 | 132 | 9 |
| 合計 | **247** | **10** | **37** | **200** | **248** |

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

対象のうち部分移植37件と未達200件が残る。例外復帰のうち BLX の戻り先差、HardFault のベクター取得中の割り込み到着、CPUWAIT のリセット境界、Robot UART テスター、STM32 USART のボーレート・IDLE・パリティ、GPIO とタイマーの NUnit 条件が未完了。既存の公開イメージ回帰と部分移植は個別の証拠として維持する。

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

上流の Cortex-M55 は一部のケースで BusFault を有効化する。Armv6-M では HardFault に昇格するため、その例外番号とベクターを検証した。CFSR、HFSR、SHCSR、ITSTATE、FP、TrustZone の観測は対象外。通常の例外復帰で保留中の割り込みを選ぶテールチェーンと、ベクター取得中に到着する割り込みの優先順位評価は残件。Fault 注入は Rust テスト内で行い、公開 STM32F072 イメージは既存動作の回帰に用いる。

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

同じ USART では BRR を `0x10` から `0` に変えたとき、`0x12` と `0x34` の順に RDR で読めることを確認した。受信フレームを待たず RXNE が立つため、上流の送信途中キューを排出する観測は部分移植。追加プローブでは `0x12` 投入直後も RE 解除後も RXNE が1で、RE 解除前の飛行中バイトを破棄するケースは未達。ボーレート、パリティ込みのフレーム時間、IDLE の発生と取り消しも未達。上流の複数 TerminalTester ID、バイナリモード、正規表現は別の残件。

```sh
# verify-stm32f072.sh で取得済みの assets を使用
PENV_RENODE_ROOT="$(realpath ../renode)" PENV_RENODE_ASSETS=/path/to/evidence/assets \
    cargo test --locked --test cli upstream_stm32f072_console_echo -- --ignored
cargo test --locked text_waits_ignore_nul
cargo test --locked renode_stm32_usart
```
