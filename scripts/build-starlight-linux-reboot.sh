#!/usr/bin/env bash
# Build a StarLight FIT with the GPIO restart driver enabled.
set -euo pipefail

if [[ $# != 2 ]]; then
    printf 'Usage: %s RENODE_ASSETS BUILD_DIRECTORY\n' "$0" >&2
    exit 2
fi

assets=$(realpath "$1")
mkdir -p "$2"
work=$(realpath "$2")
commit=93604a0a3ecc4ef8efb9d877854f89df2c7208f7
toolchain=riscv64--glibc--bleeding-edge-2021.05-1
source_dir="$work/linux-$commit"
cross="$work/$toolchain/bin/riscv64-buildroot-linux-gnu-"
build="$work/build"
out=$(realpath -m "$work/assets")
if [[ "$out" == "$assets" ]]; then
    printf 'The generated assets directory must differ from the input assets directory.\n' >&2
    exit 2
fi
elf=beaglev--fw_payload.elf-s_1218880-55f2d87dc1257f8e490cb4bdada2131d2b10d636
vmlinux=beaglev--vmlinux-s_13244120-1fca14926a0b891b58bb716a9422dbbbca6543d2
fit=beaglev--image.fit-s_83025501-3c24fa8a3dd087ac05f6a17399ff2f44f673e1c4

for name in "$elf" "$vmlinux" "$fit"; do
    printf '%s  %s\n' "${name##*-}" "$assets/$name" | sha1sum -c -
done

fetch() {
    local url=$1 archive=$2 sha=$3
    if [[ ! -f "$archive" ]]; then
        curl -fLsS --retry 2 "$url" -o "$archive.partial"
        mv "$archive.partial" "$archive"
    fi
    printf '%s  %s\n' "$sha" "$archive" | sha256sum -c -
}

fetch "https://codeload.github.com/starfive-tech/beagle_kernel_5.10/tar.gz/$commit" \
    "$work/linux.tar.gz" 3235629e9ac765470dacbda40af7521c1950aae952328d187ba248262444a6f1
fetch "https://toolchains.bootlin.com/downloads/releases/toolchains/riscv64/tarballs/$toolchain.tar.bz2" \
    "$work/toolchain.tar.bz2" 69fdb6df516ff7cc0ad56b5cddcd44a381a44c67d81f3517854a8b1950d4d70c
[[ -x "$source_dir/scripts/config" ]] || tar -xf "$work/linux.tar.gz" -C "$work"
[[ -x "${cross}gcc" ]] || tar -xf "$work/toolchain.tar.bz2" -C "$work"

mkdir -p "$build" "$out"
python3 - "$assets/$vmlinux" "$build/.config" <<'PY'
from pathlib import Path
import sys
import zlib

elf = Path(sys.argv[1]).read_bytes()
start = elf.index(b"IKCFG_ST") + len(b"IKCFG_ST")
Path(sys.argv[2]).write_bytes(zlib.decompressobj(31).decompress(elf[start:]))
PY
"$source_dir/scripts/config" --file "$build/.config" \
    --enable POWER_RESET_GPIO_RESTART --set-str LOCALVERSION "-penv-gpio" --disable LOCALVERSION_AUTO

export KBUILD_BUILD_TIMESTAMP='2021-04-26 09:57:54 UTC'
export KBUILD_BUILD_USER=penv KBUILD_BUILD_HOST=reproducible KBUILD_BUILD_VERSION=1
make -C "$source_dir" O="$build" ARCH=riscv CROSS_COMPILE="$cross" olddefconfig
grep -qx 'CONFIG_POWER_RESET_GPIO_RESTART=y' "$build/.config"
make -C "$source_dir" O="$build" ARCH=riscv CROSS_COMPILE="$cross" -j"${JOBS:-$(getconf _NPROCESSORS_ONLN)}" Image

"${DUMPIMAGE:-dumpimage}" -T flat_dt -p 1 -o "$work/rootfs.cpio" "$assets/$fit"
"${DUMPIMAGE:-dumpimage}" -T flat_dt -p 2 -o "$work/board.dtb" "$assets/$fit"
# Keep GPIO10 as an input until gpio-restart's notifier asserts reset.
fdtput "$work/board.dtb" /soc/gpio-restart open-source
cat > "$work/image.its" <<'ITS'
/dts-v1/;
/ {
    description = "BeagleV StarLight GPIO restart";
    #address-cells = <1>;
    images {
        vmlinux {
            description = "Linux with GPIO restart";
            data = /incbin/("build/arch/riscv/boot/Image");
            type = "kernel";
            arch = "riscv";
            os = "linux";
            compression = "none";
            load = <0x80200000>;
            entry = <0x80200000>;
            hash { algo = "crc32"; };
        };
        ramdisk {
            description = "Renode StarLight Buildroot initramfs";
            data = /incbin/("rootfs.cpio");
            type = "ramdisk";
            arch = "riscv";
            os = "linux";
            compression = "none";
            load = <0x86100000>;
            hash { algo = "crc32"; };
        };
        fdt {
            description = "StarLight with open-source reset GPIO";
            data = /incbin/("board.dtb");
            type = "flat_dt";
            arch = "riscv";
            compression = "none";
            load = <0x86000000>;
            hash { algo = "crc32"; };
        };
    };
    configurations {
        default = "config-1";
        config-1 {
            description = "StarLight GPIO restart";
            kernel = "vmlinux";
            fdt = "fdt";
            loadables = "ramdisk";
        };
    };
};
ITS

# The unchanged upstream RESC resolves assets by basename in the staged directory.
SOURCE_DATE_EPOCH=1619431159 "${MKIMAGE:-mkimage}" -f "$work/image.its" "$out/$fit.partial"
mv "$out/$fit.partial" "$out/$fit"
cp "$assets/$elf" "$out/$elf"
sha256sum "$out/$fit"
