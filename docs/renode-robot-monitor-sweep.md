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
