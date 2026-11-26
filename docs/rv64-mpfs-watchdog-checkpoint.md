# PolarFire ウォッチドッグ検証の区切り

固定 Renode の `tests/platforms/PolarFireSoC.robot` は公開 ELF 7本で全ケース成功。Penv では同じ ELF を使うケース別の UART・ウォッチドッグログ検証を準備し、MPFS ウォッチドッグの状態遷移とリセットの診断を追加した。`cargo test` は125件合格。台帳の7ケースは保留を維持する。

短いバッチは1件成功・6件未到達。禁止区間を無効にして禁止区間内でリフレッシュキーを書くケースが所定の UART・ログ条件を満たした。残る6件は12億ステップまたはホスト期限内に UART の後半へ届かなかった。ケース A の85億ステップ枠の試行は成功し、タイムアウトの PLIC 通知とウォッチドッグによる再起動を確認した。短い実行の未到達は機器動作の失敗として扱わない。

## ログと再現

公開素材と SHA-1 は `target/backlog-rv64-mpfs-watchdog/assets` および `assets.log`。固定 Robot のログは同ディレクトリーの `upstream.log` と `upstream/robot_output.xml`。短いバッチは `penv.log`、ケース A の長い試行は `full-a.log`。各ディレクトリーには UART、標準エラー、ケース判定、ゲストプロセスの終了状態を保存する。

```sh
out=target/backlog-rv64-mpfs-watchdog
python3 scripts/verify-rv64-mpfs-watchdog.py --penv "$out/penv-checkpoint" \
  --assets "$out/assets" --logs "$out/penv-repro" --jobs 4 --timeout 40
python3 scripts/verify-rv64-mpfs-watchdog.py --penv "$out/penv-checkpoint" \
  --assets "$out/assets" --logs "$out/full-a-repro" --case a --timeout 1500 --max-steps 8500000000
```

## 固定統合版への引き継ぎ

固定 baseline は `2cfdb6fdcf4e2b7534e4800b626afd3589d0cd72`。実行時は配信された `baseline/binary-ready` の SHA とバイナリパスを確認し、上記スクリプトの `--penv` に指定する。ローカルのウォッチドッグ診断を含むバイナリは固定 baseline の証跡から分ける。共有シンボル操作の依存は `rv64-symbol-memory-ee41a22.md` に再現を記録済みで、追加の共有修正は baseline の失敗集計後に扱う。
