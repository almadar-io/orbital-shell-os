#!/bin/bash
# Build the image inside the seL4 build container (trustworthysystems/sel4) with:
#   /sel4          seL4 16.0.0 and rust-sel4 v5.0.0 checkouts, plus a writable build/ cache
#   /project       this project
#   /orbital-core  an orbital-core checkout to build against (optional; else its git main)
# The kernel is seL4 for QEMU's aarch64 virt board, one core, non-MCS, at EL2 (see docs/Almadar_Orbital_OS.md).
set -euo pipefail
trap 'echo "build failed at line $LINENO" >&2' ERR
SEL4=/sel4
OUT=$SEL4/build
export CARGO_HOME=$OUT/cargo-home RUSTUP_HOME=$OUT/rustup-home
export PATH=$CARGO_HOME/bin:$PATH
[ -x "$CARGO_HOME/bin/rustup" ] || curl -sSf -L https://sh.rustup.rs | sh -s -- -y --no-modify-path --default-toolchain none

PREFIX=$OUT/prefix-os
if [ ! -f "$PREFIX/bin/kernel.elf" ]; then
  cmake -G Ninja -S $SEL4/seL4 -B $OUT/kernel-os \
    -DCROSS_COMPILER_PREFIX=aarch64-linux-gnu- -DCMAKE_TOOLCHAIN_FILE=$SEL4/seL4/gcc.cmake -DCMAKE_INSTALL_PREFIX=$PREFIX \
    -DKernelArch=arm -DKernelSel4Arch=aarch64 -DKernelPlatform=qemu-arm-virt -DARM_CPU=cortex-a57 -DQEMU_MEMORY=2048 \
    -DKernelMaxNumNodes=1 -DKernelIsMCS=OFF -DKernelAllowSMCCalls=ON -DKernelArmHypervisorSupport=ON \
    -DKernelRootCNodeSizeBits=20 -DKernelArmExportVCNTUser=ON -DKernelVerificationBuild=OFF -DKernelDebugBuild=ON -DKernelPrinting=ON
  ninja -C $OUT/kernel-os all install
fi

export SEL4_PREFIX=$PREFIX CC_aarch64_unknown_none=aarch64-linux-gnu-gcc
BUILD_STD=(-Z build-std=core,alloc,compiler_builtins -Z build-std-features=compiler-builtins-mem)
(cd $SEL4/rust-sel4 \
  && cargo build "${BUILD_STD[@]}" --target aarch64-unknown-none --release -p sel4-kernel-loader --target-dir $OUT/target-os \
  && cargo build --release -p sel4-kernel-loader-add-payload --target-dir $OUT/target-host)

PATCHES=(--config "patch.'https://github.com/seL4/rust-sel4'.sel4.path='$SEL4/rust-sel4/crates/sel4'"
         --config "patch.'https://github.com/seL4/rust-sel4'.sel4-root-task.path='$SEL4/rust-sel4/crates/sel4-root-task'")
if [ -d /orbital-core ]; then
  for c in base eval runtime; do
    PATCHES+=(--config "patch.'https://github.com/almadar-io/orbital-core'.orbital-core-$c.path='/orbital-core/crates/$c'")
  done
fi
cd /project
cargo build -Z json-target-spec "${BUILD_STD[@]}" "${PATCHES[@]}" \
  --target $SEL4/rust-sel4/support/targets/aarch64-sel4-roottask.json --release --target-dir $OUT/target-os
$OUT/target-host/release/sel4-kernel-loader-add-payload --sel4-prefix $PREFIX \
  --loader $OUT/target-os/aarch64-unknown-none/release/sel4-kernel-loader \
  --app $OUT/target-os/aarch64-sel4-roottask/release/orbital-os-root-task.elf \
  -o /project/image.elf
ls -la /project/image.elf
