# RV64 Smepmp

CPU の REPL に `cpuType: "rv64gc_smepmp"` を指定すると、内部 PMP に `mseccfg`（`0x747`）が追加されます。MML・MMWP の固定化、RLB の再設定制限とロック中の上書き、ロック済み TOR の下限保護、MML の共有データ・コード・読み取り専用ルールを実装しました。最初に一致する PMP エントリーがアクセス全体を覆うことを要求し、拒否した命令フェッチ・ロード・ストアにはそれぞれ例外 1・5・7 を返します。Smepmp と外部 PMP の併用、および内部 PMP が 0 エントリーの構成は拒否します。[RISC-V Privileged Architecture の Smepmp 規則](https://docs.riscv.org/reference/isa/priv/machine.html)

## 上流との対応と結果

| 出典 | 確認結果 |
| --- | --- |
| 固定 Renode `tests/unit-tests/pmp.robot:142` の奇数 `pmpcfg` アクセス、同 151 行の範囲外 `pmpaddr42` | 固定 Renode で両ケース通過。Penv は同じ命令語と 16 エントリー構成で違法命令例外と `mtval` を照合。上流の警告文字列は直接実行のログに保持し、Penv の検証は例外への部分移植 |
| Smepmp 仕様の MML 権限表およびロック | Rust で全 16 の権限符号、M/S/U、RLB と TOR、無効なロック済みエントリー、MMWP と不一致アクセスを照合。命令・ロード・ストアのアクセス例外で `mcause`、`mtval`、`mepc` を検証 |
| 公開 riscv-tests RV64 ELF | Smepmp 付き診断プロファイルで 372 本中 368 本が終了値 0。`rv64mi-p-csr`、`rv64mi-p-mcsr`、`rv64mi-p-pmpaddr` が通過。これらは PMP/CSR の回帰として記録 |
| 固定 ACT `Should Run Arch Tests 64` | Smepmp を含む診断 ISA と RAM 1 MiB では 242 本中 214 本の署名・終了を確認。`Sm-00.elf` は 4 MiB の診断で署名・終了を確認。固定 Robot の ISA 全体では V が未実装のため引き続き構成段階で拒否。診断は等価合格に加えない |
| RV64 virt の実ファームウェア | 公開 OpenSBI 1.6 と Linux 5.10.6 を使用。検証用 REPL と DTB に Smepmp を追加し、OpenSBI の `Boot HART ISA Extensions: smepmp,zicntr` と Linux の UART ログインを確認 |

`cargo test` は 106 件通過しました。ログは `target/full-rv64-smepmp` にあります。

## 再現

`$renode_work` は [ACT 上流照合](rv64-act-equivalence.md) のビルド済み Renode、`$act_work` と `$isa` は [ACT と ISA ELF の準備](rv64-act-and-failures.md) で作成したディレクトリ、`$virt_work` は README の RV64 virt の公開素材ディレクトリです。

```sh
cargo test
cargo build --release
out=target/full-rv64-smepmp
mkdir -p "$out"
"$renode_work/renode-test" "$renode_work/tests/unit-tests/pmp.robot" \\
  --fixture 'Should Trap On *' --results-dir "$out/upstream-pmp" -P 0 --show-log
python3 scripts/probe-rv64-isa.py --smepmp "$isa" --logs "$out/isa" > "$out/isa.tsv"
python3 scripts/probe-rv64-isa.py --act --smepmp "$act_work" --logs "$out/act" > "$out/act.tsv"
python3 scripts/probe-rv64-isa.py --act --smepmp "$act_work" --ram-size 0x400000 \\
  --elf spike-rv64-max/elfs/priv/Sm/Sm-00.elf --logs "$out/act-sm"
sed 's/"rv64gc"/"rv64gc_smepmp"/' platforms/rv64_virt.repl > "$out/virt-smepmp.repl"
mkdir -p "$out/assets-smepmp"
for file in Image rootfs.cpio opensbi-1.6; do
  ln -s "$(realpath "$virt_work/$file")" "$out/assets-smepmp/$file"
done
cp "$virt_work/virt-padded.dtb" "$out/assets-smepmp/virt-smepmp.dtb"
fdtput -t s "$out/assets-smepmp/virt-smepmp.dtb" /cpus/cpu@0 riscv,isa rv64imafdc_smepmp
dtc -I dtb -O dtb -p 4096 -o "$out/assets-smepmp/virt-padded.dtb" "$out/assets-smepmp/virt-smepmp.dtb"
python3 scripts/verify-rv64-virt.py "$out/assets-smepmp" \\
  --platform "$out/virt-smepmp.repl" --logs "$out/virt-advertised"
```

固定 ACT の完全な実行条件には V の実装が残ります。Robot の 1 MiB RAM を越える 26 ELF と署名生成用 ELF の扱いは [上流照合](rv64-act-equivalence.md) に記録しています。
