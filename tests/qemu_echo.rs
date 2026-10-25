//! The Orbital OS end-to-end gate: `orb compile --shell os` on the echo fixture, the image built in the seL4 container,
//! booted on QEMU's aarch64 virt board, one byte typed on the serial line. Opt-in (`ORBITAL_OS_E2E=1`): it needs
//! `orb` on PATH, Docker with `trustworthysystems/sel4`, `qemu-system-aarch64`, and the seL4 16.0.0 and rust-sel4
//! v5.0.0 checkouts under `$ORBITAL_SEL4_DIR` (default `~/.almadar/sel4`).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn sel4_dir() -> PathBuf {
    std::env::var_os("ORBITAL_SEL4_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").expect("HOME")).join(".almadar/sel4")
        })
}

fn run(command: &mut Command, what: &str) {
    let status = command.status().unwrap_or_else(|e| panic!("{what}: {e}"));
    assert!(status.success(), "{what} failed: {status}");
}

/// Boot `image`, type each of `keys` once the prompt is up, and answer everything the serial line printed.
fn boot_and_type(image: &Path, keys: &[u8], within: Duration) -> String {
    let mut qemu = Command::new("qemu-system-aarch64")
        .args([
            "-machine",
            "virt,virtualization=on",
            "-cpu",
            "cortex-a57",
            "-smp",
            "1",
            "-m",
            "size=2048M",
        ])
        .args([
            "-nographic",
            "-serial",
            "stdio",
            "-monitor",
            "none",
            "-kernel",
        ])
        .arg(image)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("qemu-system-aarch64 starts");
    let mut stdout = qemu.stdout.take().expect("qemu stdout");
    let reader = std::thread::spawn(move || {
        let mut seen = Vec::new();
        let mut buffer = [0u8; 256];
        while let Ok(n) = stdout.read(&mut buffer) {
            if n == 0 {
                break;
            }
            seen.extend_from_slice(&buffer[..n]);
        }
        String::from_utf8_lossy(&seen).into_owned()
    });
    std::thread::sleep(Duration::from_secs(10));
    let mut stdin = qemu.stdin.take().expect("qemu stdin");
    for key in keys {
        stdin.write_all(&[*key]).expect("type on the serial line");
        stdin.flush().expect("flush the serial line");
        std::thread::sleep(Duration::from_secs(2));
    }
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
    }
    qemu.kill().expect("qemu stops");
    let _ = qemu.wait();
    reader.join().expect("the serial reader ends")
}

#[test]
fn the_echo_console_boots_on_sel4_and_echoes_each_typed_byte() {
    if std::env::var("ORBITAL_OS_E2E").as_deref() != Ok("1") {
        eprintln!("skipped: set ORBITAL_OS_E2E=1 to boot the image on QEMU");
        return;
    }
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let core_dir = crate_dir
        .join("../orbital-core")
        .canonicalize()
        .expect("the orbital-core checkout");
    let monorepo = crate_dir
        .join("../../..")
        .canonicalize()
        .expect("the monorepo root");
    let sel4 = sel4_dir();
    let project = sel4.join("projects/echo-console-e2e");
    let _ = std::fs::remove_dir_all(&project);

    run(
        Command::new("orb")
            .current_dir(&monorepo)
            .arg("compile")
            .arg(crate_dir.join("tests/fixtures/echo-console.lolo"))
            .args(["--shell", "os", "-o"])
            .arg(&project),
        "orb compile --shell os",
    );
    run(
        Command::new("docker")
            .args(["run", "--rm", "-v"])
            .arg(format!("{}:/sel4", sel4.display()))
            .arg("-v")
            .arg(format!("{}:/project", project.display()))
            .arg("-v")
            .arg(format!("{}:/orbital-core:ro", core_dir.display()))
            .args([
                "-w",
                "/project",
                "trustworthysystems/sel4:latest",
                "bash",
                "/project/build.sh",
            ]),
        "the image build",
    );

    let transcript = boot_and_type(&project.join("image.elf"), b"hi", Duration::from_secs(4));
    let console = transcript
        .split_once("Booting all finished, dropped to user space")
        .map(|(_, after)| after)
        .unwrap_or_else(|| panic!("the kernel never reached user space:\n{transcript}"));
    assert!(
        !console.contains("refused"),
        "no device ask is refused:\n{console}"
    );
    let echoed = console.replace('\r', "");
    assert!(
        echoed.contains("> h\nok\n> i\nok\n> "),
        "each typed byte is echoed, then ok and a new prompt:\n{echoed}"
    );
}
