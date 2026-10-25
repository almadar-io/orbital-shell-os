//! `orb-shell-os`: the Orbital OS shell as a program (`orbital_core::shell_protocol`).

use orbital_core_base::shell_protocol::{
    description, GenerateRequest, GenerateResponse, ShellDescription, ShellFailure, ShellProgram,
    CODE_BAD_REQUEST, CODE_GENERATE_FAILED,
};
use std::path::{Path, PathBuf};

pub struct OsProgram;

/// The root-task project template, found the way every shell finds its own.
pub const TEMPLATE: &str = "sel4-root-task";

/// The program as the root task embeds it: the resolved schema, canonical JSON.
pub const PROGRAM_FILE: &str = "program.orb";

/// The language bundle the root task installs before it registers the program.
pub const LANGUAGE_FILE: &str = "language.json";

fn failed(what: &str, error: impl std::fmt::Display) -> ShellFailure {
    ShellFailure::new(CODE_GENERATE_FAILED, format!("{what}: {error}"))
}

fn program_json(request: &GenerateRequest) -> Result<String, ShellFailure> {
    let resolved = request.resolved.as_ref().ok_or_else(|| {
        ShellFailure::new(
            CODE_BAD_REQUEST,
            "the os shell embeds the resolved program, and the request carries none",
        )
    })?;
    let value =
        serde_json::to_value(resolved).map_err(|e| failed("serialize the resolved program", e))?;
    serde_json::to_string(&orbital_core_eval::util::canonical_json(&value))
        .map_err(|e| failed("serialize the resolved program", e))
}

fn copy_tree(
    from: &Path,
    to: &Path,
    written: &mut Vec<String>,
    root: &Path,
) -> Result<(), ShellFailure> {
    std::fs::create_dir_all(to).map_err(|e| failed(&format!("create {}", to.display()), e))?;
    let mut entries: Vec<PathBuf> = std::fs::read_dir(from)
        .map_err(|e| failed(&format!("read the template {}", from.display()), e))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    for source in entries {
        let target = to.join(source.file_name().expect("a directory entry has a name"));
        if source.is_dir() {
            copy_tree(&source, &target, written, root)?;
        } else {
            std::fs::copy(&source, &target)
                .map_err(|e| failed(&format!("copy {}", source.display()), e))?;
            written.push(
                target
                    .strip_prefix(root)
                    .unwrap_or(&target)
                    .display()
                    .to_string(),
            );
        }
    }
    Ok(())
}

fn template_dir() -> PathBuf {
    orbital_core_base::renderer_packs::shell_template(
        TEMPLATE,
        Path::new(env!("CARGO_MANIFEST_DIR")),
    )
}

impl ShellProgram for OsProgram {
    fn describe(&self) -> ShellDescription {
        description("os", env!("CARGO_PKG_VERSION"), true, vec![])
    }

    fn generate(
        &self,
        request: &GenerateRequest,
        output: &Path,
    ) -> Result<GenerateResponse, ShellFailure> {
        let program = program_json(request)?;
        let language = orbital_core_base::language_data::language_bundle_json()
            .map_err(|e| ShellFailure::new(CODE_GENERATE_FAILED, format!("{}: {e}", e.code())))?;
        let mut files = Vec::new();
        copy_tree(&template_dir(), output, &mut files, output)?;
        for (name, text) in [(PROGRAM_FILE, &program), (LANGUAGE_FILE, &language)] {
            std::fs::write(output.join(name), text)
                .map_err(|e| failed(&format!("write {name}"), e))?;
            files.push(name.to_string());
        }
        Ok(GenerateResponse {
            files,
            warnings: vec![],
            next_steps: vec![
                format!("cd {}", output.display()),
                "./build.sh        # inside the seL4 build container: kernel, loader, root task, image".to_string(),
                "./run-qemu.sh     # boot the image on QEMU's aarch64 virt board".to_string(),
            ],
        })
    }

    fn emit(
        &self,
        request: &GenerateRequest,
    ) -> Result<orbital_core_base::generate::GeneratedOutput, ShellFailure> {
        let program = program_json(request)?;
        Ok(orbital_core_base::generate::GeneratedOutput {
            files: vec![orbital_core_base::generate::GeneratedFile::new(
                PROGRAM_FILE,
                program,
            )],
            warnings: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core_base::ir::OirModule;
    use orbital_core_base::schema::types::OrbitalSchema;
    use orbital_core_base::shell_protocol::IR_VERSION;

    fn resolved() -> OrbitalSchema {
        serde_json::from_value(serde_json::json!({
            "name": "echo",
            "orbitals": [{
                "name": "Echo",
                "entity": { "name": "E", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
                "traits": [],
                "pages": []
            }]
        }))
        .unwrap()
    }

    fn request(resolved: Option<OrbitalSchema>) -> GenerateRequest {
        GenerateRequest {
            ir_version: IR_VERSION,
            module: OirModule::new("echo", "1.0.0"),
            resolved,
            served: None,
            source_path: None,
            write: Default::default(),
            options: Default::default(),
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orb-os-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_shell_asks_for_the_resolved_program_and_never_the_served_one() {
        let described = OsProgram.describe();
        assert_eq!(described.name, "os");
        assert!(described.needs_resolved);
        assert!(
            !described.needs_served,
            "control: the os shell joins no http server"
        );
    }

    #[test]
    fn generate_writes_the_program_the_language_and_the_root_task_template() {
        let out = scratch("generate");
        let answer = OsProgram
            .generate(&request(Some(resolved())), &out)
            .unwrap();
        for file in [
            PROGRAM_FILE,
            LANGUAGE_FILE,
            "Cargo.toml",
            "src/main.rs",
            "build.sh",
            "run-qemu.sh",
        ] {
            assert!(out.join(file).is_file(), "{file} written");
            assert!(
                answer.files.iter().any(|f| f == file),
                "{file} reported in {:?}",
                answer.files
            );
        }
        let program: OrbitalSchema =
            serde_json::from_str(&std::fs::read_to_string(out.join(PROGRAM_FILE)).unwrap())
                .unwrap();
        assert_eq!(program.name, "echo");
        let language: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(out.join(LANGUAGE_FILE)).unwrap())
                .unwrap();
        assert!(
            language.get("core").is_some() && language.get("std").is_some(),
            "the bundle carries core and std"
        );
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn control_a_request_without_the_resolved_program_is_refused_and_writes_nothing() {
        let out = scratch("refused");
        let error = OsProgram.generate(&request(None), &out).unwrap_err();
        assert_eq!(error.code, CODE_BAD_REQUEST);
        assert!(!out.exists(), "nothing is written for a refused request");
    }
}
