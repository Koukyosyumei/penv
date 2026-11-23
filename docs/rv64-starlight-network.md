# StarLight の固定 Linux・Ethernet ケース

固定 Renode の `tests/platforms/BeagleV-StarLight.robot` と同じ公開 RESC、ファームウェアおよび2台の構成で照合する。通常構成と外部 PMP 構成のどちらも、各ゲストの U-Boot 確認後にループバック上の仮想スイッチに接続する。

`verify-starlight.py --upstream-cases` は固定 Robot と同じ順で各 UART に `root` とパスワードを送り、`ifconfig eth0 down`、`ifconfig eth0 hw ether <MAC>`、`ifconfig eth0 mtu 440 up <IP>` を実行する。通常の入力はエコーも待つ。ping は両構成で2台目から1台目に `ping -As 56 -c 10 192.168.0.5` を送り、通常構成では続けて3200バイトでも照合する。期待値はいずれも10送信・10受信・損失0%。コマンド実行や IRQ 回帰を含む既存の診断モードは、`--upstream-cases` を付けずに実行できる。

固定 Robot はゲストの直列スケジューリング、仮想時間上の期限と UART 待機ごとの pause を使用する。Penv はゲストを別プロセスで走らせ、ホスト側の期限内で同じ順序の UART 観測を照合する。

## 再現

```sh
out=target/backlog-rv64-starlight-network
ref=/var/tmp/penv-rv64-act-upstream
mkdir -p "$out"
cargo test > "$out/cargo-test.log" 2>&1
cargo build --release
PATH="$ref/dotnet:$ref/robot-venv/bin:$PATH" \
  "$ref/renode/renode-test" "$ref/renode/tests/platforms/BeagleV-StarLight.robot" \
  --results-dir "$PWD/$out/upstream" -P 0 > "$out/upstream.log" 2>&1
python3 scripts/verify-starlight.py "$assets" --upstream-cases \
  --ports 31541 31542 --switch-port 31540 --logs "$out/internal" > "$out/internal.log" 2>&1
python3 scripts/verify-starlight.py "$assets" --upstream-cases --external-pmp \
  --ports 31551 31552 --switch-port 31550 --logs "$out/external" > "$out/external.log" 2>&1
```

`assets` の準備は [StarLight 起動の記録](rv64-starlight-boot.md) を参照。検証に使用した4ファイルの SHA-1 は `assets.log` に記録した。

固定 Renode と Penv で StarLight の7ケースが通過した。`cargo test` は125件合格。独立調査で Fire の固定 SD ケースも Renode で直接実行し、成功ログを `fire-upstream` に保存した。Fire の台帳判定は、同じ条件での Penv 再実行が残るため維持した。

## 残課題

ゲストのプロセス実行順や仮想時間の進め方は固定 Renode と異なる。PC の進行を伴う UART pause の汎用的な再現はこの移植の対象外。
