# RV64 ISA 診断失敗と固定 ACT

[242 ELF の実行結果と失敗の分類](rv64-act-run.md)を追加しました。

[原因別台帳](rv64-isa-failures.tsv)に `0b01721` の診断で失敗した ELF 26 本を、当時の終了理由と再実行結果付きで記録しました。

| 原因 | ELF | 再実行 |
| --- | ---: | --- |
| Zfh の命令・binary16 が未実装 | 20 | 半精度のロード・ストア、NaN ボックス、演算、変換に対応。標準診断で全 20 本が終了値 0 |
| `minstret` の明示書き込みに退役分を二重反映 | 1 | 明示書き込みを最終値として扱い、次の命令で読み戻しとラップを確認 |
| `Zicboz` の `cbo.zero` が未実装 | 1 | 64 バイト RAM ブロックと M/S の `CBZE` 制御を追加 |
| 診断プロファイルが非整列ロード・ストアを禁止 | 2 | `allowUnalignedAccesses: true` の別プロファイルで終了値 0 |
| ELF のロードセグメントが 1 MiB のマップを越える | 1 | 4 MiB の別プロファイルで終了値 0。ゲストは正しいページフォルトでも成功するため Svnapot 対応の証拠には含めない |
| デバッガーのトリガー CSR とアドレス照合が未実装 | 1 | `rv64mi-p-breakpoint` は終了コード 2 のまま |

`cargo test` は 100 件通過し、実イメージの RV64 virt で OpenSBI、Linux、UART root ログインと `id -u`／`uname -m` を確認しました。

標準診断は 372 本中 368 本が終了値 0、4 本が失敗・未完了です。台帳の合否と診断結果は別に扱います。固定 Robot の `riscv-isa-tests.robot` は `rv64gcv_zba_zbb_zbc_zbs_zfh_zacas_smepmp`、RAM 1 MiB と、CPU 別の `ArrayMemoryRiscVTestWatcher` を使います。Penv の診断は `probe-rv64-isa.py` に記載したサポート済みの拡張、RAM 上の終了通知、命令上限を使います。別プロファイルでは RAM サイズまたは非整列許可も変更します。公開 ELF が終了値 0 を返した場合も上流の等価合格には加えません。[Zicsr の退役カウンター規則](https://docs.riscv.org/reference/isa/unpriv/zicsr.html) [RISC-V CMO](https://docs.riscv.org/reference/isa/unpriv/cmo.html)

```sh
cargo test
cargo build --release
out=target/full-rv64-act-and-failures
mkdir -p "$out"
python3 scripts/probe-rv64-isa.py "$work/isa" --logs "$out/isa" > "$out/isa.tsv"
python3 scripts/probe-rv64-isa.py "$work/isa" --allow-unaligned \\
  --elf rv64ui-p-ma_data --elf rv64ui-v-ma_data --logs "$out/unaligned"
python3 scripts/probe-rv64-isa.py "$work/isa" --ram-size 0x400000 \\
  --elf rv64ssvnapot-p-napot --logs "$out/large-ram"
python3 scripts/verify-rv64-virt.py "$virt_work" --logs "$out/virt-final"
```

`$work/isa` と `$virt_work` の準備は [上流台帳の再現手順](rv64-full-upstream.md) と README の RV64 virt 節にあります。

## ACT のコンテナ構成

固定 ACT `1dfa48c2adecaa16fae9fca14d34c591ea99321c` の `parse_udb_config.py` は UDB の検証と拡張一覧の生成で `PODMAN=true` を渡します。固定 UDB `285e23a1a3f1df8114df2dcf9463f05d8b507026` の起動スクリプトは、ローカルの `.container-type` を環境変数より優先します。一時チェックアウトでこれを `docker` に設定し、UDB のソースはそのまま使えます。イメージは `ghcr.io/riscv/udb:0.19`、確認したマニフェストは `sha256:693d8070498ac64263cb7ab0684024fc89985e13fbd24712142fcfd02eb7474b` です。

ACT と UDB は Git の一時チェックアウトに用意します。ACT で `git submodule update --init -- external/riscv-unified-db` を実行します。`uv` 0.8.24、Sail 0.9、RISC-V ELF 用の GCC と objdump を準備します。この実行では xPack GCC 15.2.0 の `riscv-none-elf-` を使ったため、固定 `test_config.yaml` をコピーし、`compiler_exe` と `objdump_exe` のパスだけを置き換えました。

利用可能な Docker デーモンがある場合はそのソケットを指定します。この環境では `/var/tmp` に専用ソケットとデータ領域を設け、ビルドコンテナにブリッジや iptables の変更が不要な構成でデーモンを起動しました。`/tmp` の tmpfs と vfs を使った初回のイメージ展開は容量不足でした。

```sh
# 専用デーモンが必要な場合のみ、同じ値を別端末にも設定する。
docker_work=$(mktemp -d /var/tmp/penv-act-docker.XXXXXXXX)
sudo dockerd --host "unix://$docker_work/docker.sock" --group "$(id -gn)" \\
  --pidfile "$docker_work/dockerd.pid" --data-root "$docker_work/data" \\
  --exec-root "$docker_work/exec" --storage-driver overlay2 \\
  --bridge=none --iptables=false --ip6tables=false --ip-forward=false --ip-masq=false

export DOCKER_HOST="unix://$docker_work/docker.sock"
image=ghcr.io/riscv/udb@sha256:693d8070498ac64263cb7ab0684024fc89985e13fbd24712142fcfd02eb7474b
docker pull "$image"
docker tag "$image" ghcr.io/riscv/udb:0.19
printf 'docker\n' > "$act/external/riscv-unified-db/.container-type"
CONFIG_FILES=config/spike/spike-rv64-max/test_config-penv.yaml make -C "$act" -j8
```

ビルドは終了コード 0 で完了しました。固定 Renode のセレクターが `spike-rv64-max` の 242 ELF を選択しました。最終 ELF 241 本に加え、拡張子とパスの規則で選ばれた中間署名 ELF `build/priv/Sm/Sm-00.sig.elf` も保持し、全パスを上流台帳の `Should Run Arch Tests 64` に記録しました。各行の状態は `pending` です。ビルドログと ELF の SHA-256 一覧はそれぞれ `target/full-rv64-act-and-failures/act-build.log` と `act-elfs.sha256` にあります。

ACT の構成は固定 `spike-rv64-max` とソース既定の生成拡張を使います。生成した ELF を台帳に展開するときは、台帳生成コマンドに `--arch "$act/work"` を追加します。
