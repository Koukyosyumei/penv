# StarLight の2台 U-Boot ケース

固定 Renode の `tests/platforms/BeagleV-StarLight.robot` の `Should Boot U-Boot` と `Should Boot U-Boot With External PMP` を独立して検証できるようにした。上流の StarLight RESC と公開ファームウェアを使い、各ケースで2台を起動した。各 UART で OpenSBI v0.9、プラットフォーム名 StarFive、U-Boot 2021.01 の行と `dwmac.10020000` のプロンプトをこの順に照合した。外部 PMP ケースでは `U74_1` と `U74_2` にそれぞれ登録する。

Penv の各ゲストは個別プロセスで動く。固定 Robot のグローバル直列実行と仮想時間タイムアウトに対し、移植では同じ順序の UART アサーションをホスト側の期限内で評価する。両ケースともゲスト間の通信を必要とせず、スイッチは次の Linux ケースで接続される。

## 再現

```sh
out=target/backlog-rv64-starlight-boot
ref=/var/tmp/penv-rv64-act-upstream
mkdir -p "$out"
cargo test > "$out/cargo-test.log" 2>&1
PATH="$ref/dotnet:$ref/robot-venv/bin:$PATH" \
  "$ref/renode/renode-test" "$ref/renode/tests/platforms/BeagleV-StarLight.robot" \
  --fixture 'Should Boot U-Boot' --results-dir "$PWD/$out/upstream" -P 0 > "$out/upstream.log" 2>&1
python3 scripts/verify-starlight.py "$assets" --upstream-boot-only \
  --logs "$out/internal" > "$out/internal.log" 2>&1
python3 scripts/verify-starlight.py "$assets" --upstream-boot-only --external-pmp \
  --logs "$out/external" > "$out/external.log" 2>&1
```

`assets` の取得手順は [README](../README.md) の StarLight 公開素材を参照。使用した4ファイルの SHA-1 は RESC の URL に含まれる値と一致した。固定 Robot と同じ Penv バイナリで2ケースとも通過し、`cargo test` は125件合格。ケース別ログと終了状態は `out` に保存した。

## 残課題

Linux ログインと Ethernet の固定ケースは [ネットワーク検証の記録](rv64-starlight-network.md) を参照。固定 RV64 メモリ値ケースの負数入力は共有 CLI で拒否されるため、再現手順と既存差分を `rv64-memory-values-73d7174.md` として ARM 担当の共有依頼キューに登録した。
