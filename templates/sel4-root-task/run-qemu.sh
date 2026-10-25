#!/bin/bash
# Boot the image on QEMU's aarch64 virt board; the PL011 UART is this terminal.
exec qemu-system-aarch64 -machine virt,virtualization=on -cpu cortex-a57 -smp 1 -m size=2048M \
  -nographic -serial mon:stdio -kernel "$(dirname "$0")/image.elf"
