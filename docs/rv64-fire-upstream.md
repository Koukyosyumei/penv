# Fire の固定 SD 起動ケース

固定 Renode の `tests/platforms/BeagleV-Fire.robot` の `Should Boot Buildroot From SD Card` に対応する検証を `verify-fire.py --upstream-case` に追加した。同じ公開 HSS、U-Boot、SD イメージを元の `beaglev-fire.resc` で読み込む。使用した3ファイルの SHA-1 は RESC の URL に含まれる値と照合した。

固定ケースと同じ順序で `mmuart0` の `OpenSBI - version 1.2`、`U-Boot 2023.07`、`Model: BeagleBoard BeagleV-Fire`、`buildroot login:` を確認する。`root` を入力し、各コマンドのエコー、`uname -m` の `riscv64` と `uname -n` の `buildroot` を照合する。既存の診断モードでは、この後の SD 先頭セクターの SHA-256 照合も引き続き実行できる。

固定 Robot のログイン待ちは仮想時間25秒。Penv の移植はホスト側の期限を使い、同じ順序の UART 入出力を照合する。

## 再現

```sh
out=target/backlog-rv64-fire-upstream
ref=/var/tmp/penv-rv64-act-upstream
mkdir -p "$out"
cargo test > "$out/cargo-test.log" 2>&1
cargo build --release
PATH="$ref/dotnet:$ref/robot-venv/bin:$PATH" \
  "$ref/renode/renode-test" "$ref/renode/tests/platforms/BeagleV-Fire.robot" \
  --fixture 'Should Boot Buildroot From SD Card' --results-dir "$PWD/$out/upstream" -P 0
python3 scripts/verify-fire.py "$fire_assets" --upstream-case --max-steps 9000000000 \
  --logs "$out/penv" > "$out/penv.log" 2>&1
```

`fire_assets` の取得は [README の Fire 手順](../README.md) を参照。固定 Robot の直接実行ログは前回の `target/backlog-rv64-starlight-network/fire-upstream` に保存済み。今回の Penv UART ログとケース状態は `out` に保存した。固定 Robot と Penv の両方でケースが通過し、`cargo test` は125件合格した。

## 関連する残課題

Linux は SD の root ファイルシステムを読み取り専用でマウントした。再マウント時の書き込みエラーと Ethernet PHY の未接続も UART ログで確認した。固定ケースのログインと `uname` はどちらも通過した。

Fire REPL を使う `monitor-tests.robot` の3件は、固定 Renode で成功した。Penv は `qspi.pseFlash SFDPSignature [1, 2, 3]` を実行する時点で未対応の Monitor コマンドを返す。共有の配列プロパティ、`Select ToString`、型付きスカラーのエラーについて、再現手順を ARM 担当の `rv64-fire-monitor-cc6888d.md` に登録した。Fire 固有の QSPI モデルと共有入口の接続は残っている。
