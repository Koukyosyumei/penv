# ARM 上流回帰の調査

対象は STM32F072B Discovery、Armv6-M の Cortex-M0/M0+、および使用する STM32 の周辺機器。既存受入の対象と結果は README の「固定した上流 ARM 回帰の対応表」に記録している。

| 参照ツリー | 固定コミット |
| --- | --- |
| Renode | `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` |
| `src/Infrastructure` | `09ac7c563095c5410258ecc1b1a6b841a013db85` |
| Infrastructure の `src/Emulator/Cores/tlib` | `c04522a9c886ba743c77a86b0c879103a2ef46d0` |

[ケース台帳](arm-upstream-cases.tsv)は調査候補を上流パス・ケース・ソース行で記録する。例外復帰のキーワード引数行は独立させた。その他の `see source case body` の行は、パラメータの展開と照合が残っている。Infrastructure の時計、GPIO、バス、CRC 等の NUnit 候補も収録した。固定 tlib ツリーには Robot と C# テストファイルがなかった。

`existing translation` は既存の移植または今回追加した移植を示し、全ケース受入の判定には使用しない。`partial` は対応する観測条件の一部、`open applicability` は適用の根拠の確認が残る候補を示す。台帳は調査途中であり、全件合格ゲートの根拠としては未完成。

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

台帳は固定 Infrastructure ソースを展開したディレクトリを第2引数に渡し、`python3 scripts/inventory-arm-upstream.py ../renode /path/to/infrastructure` で標準出力に再生成できる。

## 残件

共有テストの適用可否とパラメータを、上流と同じ入力・タイミング・失敗判定まで突合する。例外復帰のうち BLX の戻り先差、HardFault のベクター取得中の割り込み到着、CPUWAIT のリセット境界、Robot UART テスター、STM32 USART のボーレート・IDLE・パリティ、GPIO とタイマーの NUnit 条件が未完了。既存の公開イメージ回帰と部分移植は個別の証拠として維持する。

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
