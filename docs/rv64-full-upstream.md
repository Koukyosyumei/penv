# RV64 上流ケース台帳

この台帳は BeagleV StarLight、BeagleV-Fire、RV64 virt と、そこで適用できる ISA・特権・ネットワーク・ストレージのためのものです。固定したソースは下表の通りです。Renode の参照作業ツリーは読み取り専用で使い、未展開のサブモジュールはコミット指定のアーカイブを別ディレクトリに展開しました。

| ソース | リビジョン |
| --- | --- |
| Renode | `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` |
| Renode Infrastructure | `09ac7c563095c5410258ecc1b1a6b841a013db85` |
| Renode UI | `5bc51e6eb4b1c36421527c1fab8c0fc3f6b6947f` |
| tlib | `c04522a9c886ba743c77a86b0c879103a2ef46d0` |
| Packet.Net | `20e0f2fe40743fed4e6a81bb8fe9523c257f168b` |
| FdtSharp | `fb43c1507c2c7680d386fcef2dcb75bbe1fae273` |
| ELFSharp | `bf797c5bbec983578ded3081a8e17d1c646e7c32` |
| InpliTftpServer | `2a4877dce99fd7a30b85b5bf217c631538cd7086` |

[台帳](rv64-upstream.tsv)はソースツリー、ファイル、行番号、ケース名、パラメータを保持します。同名の Robot ケースも行番号で区別します。ベースは Robot のケース宣言と NUnit の属性です。機種固有の StarLight 7 ケースと Fire 1 ケースを収録しました。固定 Renode の virt REPL は RV32 定義で、Penv の RV64 virt Linux 起動自動検証は追加の回帰です。既存の上流ケース移植は、期待値を再照合できるまで `partial` に置いています。両 BeagleV の過去の実ファームウェア結果は [README](../README.md) の「上流 Renode の RV64 ケース対応」にあります。

| 状態 | 判定 |
| --- | --- |
| `pass` | 上流と同じ入力とアサーションを確認した移植 |
| `partial` | 既存検証があるケース。上流との差分または照合待ち |
| `pending` | 候補として保持中。共有フィクスチャと実行方法を確認する |
| `review` | ケース固有の適用可否を確認する。集計上も除外しない |
| `unexpanded` | パラメータを生成する入力待ち |
| `out` | 当該ケースが RV32 のハートやレジスターを明示的に要求する。根拠を各行に記録 |

## 再現

```sh
cargo test
cargo build --release
work=/tmp/penv-rv64-full
mkdir -p "$work/isa"
curl -fLsS https://codeload.github.com/renode/renode-infrastructure/tar.gz/09ac7c563095c5410258ecc1b1a6b841a013db85 | tar xz -C "$work"
curl -fLsS https://codeload.github.com/antmicro/renode-ui/tar.gz/5bc51e6eb4b1c36421527c1fab8c0fc3f6b6947f | tar xz -C "$work"
curl -fLsS https://codeload.github.com/antmicro/tlib/tar.gz/c04522a9c886ba743c77a86b0c879103a2ef46d0 | tar xz -C "$work"
for dep in 'Packet.Net 20e0f2fe40743fed4e6a81bb8fe9523c257f168b' \
           'FdtSharp fb43c1507c2c7680d386fcef2dcb75bbe1fae273' \
           'elfsharp bf797c5bbec983578ded3081a8e17d1c646e7c32' \
           'InpliTftpServer 2a4877dce99fd7a30b85b5bf217c631538cd7086'; do
  read -r repo revision <<<"$dep"
  curl -fLsS "https://codeload.github.com/antmicro/$repo/tar.gz/$revision" | tar xz -C "$work"
done
curl -fLsS https://github.com/riscv-software-src/sail-riscv-tests/releases/download/2026-08-20/riscv-tests.tar.gz -o "$work/isa.tar.gz"
printf '%s  %s\n' eb24265e83ca0b68a24a513dff7e4535975e724fe07d71f3cf75fef87fd75bec "$work/isa.tar.gz" | sha256sum -c -
tar xzf "$work/isa.tar.gz" -C "$work/isa" --wildcards './rv64*'
python3 scripts/inventory-rv64-upstream.py ../renode "$work"/renode-infrastructure-* \\
  --ui "$work"/renode-ui-* --tlib "$work"/tlib-* --isa "$work/isa" \
  --dependency packet-net "$work"/Packet.Net-* --dependency fdtsharp "$work"/FdtSharp-* \
  --dependency elfsharp "$work"/elfsharp-* --dependency inpli-tftp "$work"/InpliTftpServer-* \
  > target/rv64-upstream.tsv
cmp target/rv64-upstream.tsv docs/rv64-upstream.tsv
python3 scripts/probe-rv64-isa.py "$work/isa" --logs target/full-upstream-rv64/isa \\
  > target/full-upstream-rv64/isa.tsv
python3 scripts/verify-rv64-virt.py "$virt_work" --logs target/full-upstream-rv64/virt-final
```

`$virt_work` は README の OpenSBI・Linux・DTB 準備で作成したディレクトリです。ISA の試行は Renode の Python セレクターを使用し、`llvm-nm` で ELF ごとの `tohost` を解決します。終了値を通知する奇数書き込みがなければ成功扱いにしません。CPU には現在の Penv が扱える拡張を設定し、結果は [ELF 別診断](rv64-isa-diagnostics.tsv) と個別の標準出力・エラーログに記録します。Renode の CPU 構成、タイムアウト、全書き込み経路の等価性は未確認です。

ACT の再現では、一時ディレクトリに `riscv-arch-test` の固定コミットを Git でチェックアウトし、`git submodule update --init -- external/riscv-unified-db` を実行します。`test_config.yaml` のコンパイラー、objdump、Sail と UDB が使うコンテナ実行環境を用意し、次を実行します。

```sh
CONFIG_FILES=config/spike/spike-rv64-max/test_config.yaml make -C "$act" -j8
# 台帳生成コマンドに --arch "$act/work" を追加する。
```

## 結果と残課題

- `cargo test` は 97 件通過しました。AMOCAS の未整列 RAM アクセスを REPL の `allowUnalignedAccesses` に従って追加し、後続ページがアクセス不能な場合に先頭側が変化しないことも検証しました。d/q は単一ページとページ境界またぎの 8 ケースが通過しました。q のゼロレジスターと奇数レジスター対の 3 ケースも、上流の値と例外レジスターを照合しました。w の 4 ケースは上流の旧値がゼロ拡張で、Penv は符号拡張するため部分移植です。AMOADD.W/D の単一ページとページ境界またぎも、上流の 2 回の加算と読み戻しを含む 4 ケースを移植しました。ArrayMemory を使う AMO の MMIO ケースは残っています。
- RV64 の MRET は上流の MachineUser と Machine の 2 ケースを移植しました。REPL の `privilegeLevels` を設定に反映し、例外前後の PC と MPP を同じ値で検証しました。
- Zicond の RV64 用 11 ケースを移植し、無効時のトラップ、64 ビット値とレジスター別名を確認しました。Zbkb は pack・packh・brev8 の 3 ケースが通過しました。packw は固定 tlib と Robot がゼロ拡張を期待する一方、Penv は RV64 命令の符号拡張を実装しているため部分移植です。
- 公開 ELF 372 本の診断は 346 本が終了値 0、26 本が失敗または未完了でした。上流等価の合格数には加えていません。残りには半精度浮動小数点、未整列アクセス、ブレークポイント、カウンター、CBO、Svnapot と ELF の配置差が含まれます。
- 上流の `riscv-arch-tests.robot` は `spike-rv64-max` の生成 ELF が必要です。Robot に記載された `riscv-arch-test` の参照は `1dfa48c2adecaa16fae9fca14d34c591ea99321c` です。ビルド出力を用意して `--arch` に渡すと、同梱セレクターの結果をパラメータに展開できます。固定 ACT・UDB を Git の別作業ツリーに用意し、uv 0.8.24、xPack GCC 15.2.0、Sail 0.9 で生成を試行しました。UDB の構成検証が要求する Podman が見つからず、Docker デーモンにも接続できませんでした。生成は未完了です。ログは `target/full-upstream-rv64/arch-build-pinned.log` にあります。NUnit の `TestCaseSource`、動的 Robot テンプレートとサブモジュールの追加テスト入力も個別展開が必要です。Packet.Net と FdtSharp の NUnit 行はケース別の適用判断待ちです。
- RV64 virt では OpenSBI v1.6、Linux 5.10.6、UART root ログインと `id -u`／`uname -m` を実イメージで確認しました。
- Fire の eMMC パーティションを扱う共有 NUnit フィクスチャ 5 ケースを保持しています。SD 書き込み、eMMC のパーティション、StarLight のネットワークに関する共有ケースの適用判断と移植は継続項目です。共有スイートの `review` 行は全件の判定を終えていません。全件合格ゲートの条件は未成立です。
