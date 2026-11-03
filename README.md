<div align="center">

# orbital-shell-os

**The Orbital OS shell: writes a program and its language into an seL4 root-task project that runs on the kernel.**

[![CI](https://github.com/almadar-io/orbital-shell-os/actions/workflows/ci.yml/badge.svg)](https://github.com/almadar-io/orbital-shell-os/actions/workflows/ci.yml) [![License: BUSL-1.1](https://img.shields.io/badge/license-BUSL--1.1-blue)](LICENSE)

</div>

<p align="center"><sub>Part of <a href="https://github.com/almadar-io/orb"><b>Orb</b></a>: a programming language of composable state machines. <a href="https://orb.almadar.io">orb.almadar.io</a></sub></p>

`orb compile --shell os` runs this crate as the `orb-shell-os` program. It copies the `sel4-root-task` project template and embeds the resolved program (`program.orb`) and the language bundle (`language.json`) in it. The root task hosts the `orbital-core` kernel on seL4 and answers the kernel's asks.

## Where this fits

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/almadar-io/orb/main/static/img/readme/architecture/orbital-shell-os-dark.svg">
  <img src="https://raw.githubusercontent.com/almadar-io/orb/main/static/img/readme/architecture/orbital-shell-os-light.svg" width="100%" alt="Where orbital-shell-os sits in the Almadar architecture">
</picture>

| Upstream (this repo depends on) | Downstream (depends on this repo) |
|---|---|
| [orbital-core](https://github.com/almadar-io/orbital-core): the shell protocol, language bundle and canonical JSON (`orbital-core-base`, `orbital-core-eval`) | [orbital-cli](https://github.com/almadar-io/orbital-cli): the `orb` binary runs this shell as a program for `--shell os` |

## Quick start

```bash
orb compile <program.lolo> --shell os -o <out-dir>
cd <out-dir>
./build.sh        # inside the seL4 build container: kernel, loader, root task, image
./run-qemu.sh     # boot the image on QEMU's aarch64 virt board
```

These are the next steps the shell prints after generating. Building needs the seL4 container and QEMU; see the opt-in end-to-end test below.

## What's inside

| Path | Contents |
|---|---|
| `src/shell_program.rs` | `OsProgram`, the shell program: `describe`, `generate`, `emit` |
| `src/bin/orb-shell-os.rs` | The `orb-shell-os` binary |
| `templates/sel4-root-task/` | The project scaffold: `Cargo.toml`, `build.sh`, `run-qemu.sh`, `src/main.rs`, `src/devices.rs` |
| `tests/qemu_echo.rs`, `tests/fixtures/` | The end-to-end gate and its echo programs (`echo-console.lolo`, `echo-console-ungranted.lolo`) |

The shell needs the resolved program and refuses a request that carries none (`CODE_BAD_REQUEST`). `generate` writes the template tree plus `program.orb` and `language.json`; `emit` returns just `program.orb`.

## Development

The crate reads the Almadar language data (patterns, operators, themes, vocabularies) from installed npm packages, so install those first (this is the CI recipe; the toolchain is pinned to Rust 1.93.0 in `rust-toolchain.toml`):

```bash
npm install --no-save --no-package-lock @almadar/core @almadar/std @almadar/lolo-ar @almadar/lolo-sl
cargo test
```

Its `orbital-*` dependencies are git dependencies on the `main` branch of their own repos.

The QEMU test is opt-in: `ORBITAL_OS_E2E=1` needs `orb` on PATH, Docker with `trustworthysystems/sel4`, `qemu-system-aarch64`, and the seL4 16.0.0 and rust-sel4 v5.0.0 checkouts under `$ORBITAL_SEL4_DIR` (default `~/.almadar/sel4`).

Open gaps are tracked in [Almadar_Gaps.md](Almadar_Gaps.md). Work on `main`; never create a feature branch. Inside the monorepo this repo is the submodule `orbital-rust/crates/orbital-shell-os`; work from `orbital-rust` with `cargo test -p orbital-shell-os`.

## Docs

- [Orb documentation](https://orb.almadar.io/docs): the language, the `orb` CLI and how the pieces fit

## License

[Business Source License 1.1](LICENSE): free for non-commercial use and internal evaluation; commercial production use needs a licence from Almadar FZE. Converts to Apache-2.0 on 2030-02-01.
