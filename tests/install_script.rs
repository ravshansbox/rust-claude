#![cfg(unix)]

use std::{
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const LATEST_ZIP: &str =
    "https://github.com/ravshansbox/rust-claude/releases/latest/download/rust-claude.zip";

struct Setup {
    root: PathBuf,
    bin: PathBuf,
}

impl Setup {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("rust-claude-install-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let mut zip = zip::ZipWriter::new(std::fs::File::create(root.join("fixture.zip")).unwrap());
        zip.start_file(
            "rust-claude/Cargo.toml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(b"[package]\nname = \"rust-claude\"\nversion = \"0.5.0\"\n")
            .unwrap();
        zip.finish().unwrap();
        let setup = Self { root, bin };
        setup.script(
            "curl",
            &format!(
                "echo \"$@\" > '{root}/curl.log'\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = -o ]; then out=$2; fi\n  shift\ndone\ncp '{root}/fixture.zip' \"$out\"\n",
                root = setup.root.display()
            ),
        );
        setup
    }

    fn script(&self, name: &str, body: &str) {
        let path = self.bin.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn with_cargo(self) -> Self {
        self.script(
            "cargo",
            &format!(
                "echo \"$@\" > '{root}/cargo.log'\nfor last; do :; done\necho \"$last\" > '{root}/folder'\ngrep -q 'version = \"0.5.0\"' \"$last/Cargo.toml\" && echo unpacked >> '{root}/cargo.log'\n",
                root = self.root.display()
            ),
        );
        self
    }

    fn install(&self) -> Output {
        Command::new("/bin/sh")
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/install.sh"))
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .output()
            .unwrap()
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.root.join(name)).unwrap_or_default()
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

#[test]
fn installs_the_latest_release_with_cargo() {
    let setup = Setup::new("latest").with_cargo();
    let output = setup.install();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        setup.read("curl.log").contains(LATEST_ZIP),
        "{}",
        setup.read("curl.log")
    );
    let cargo = setup.read("cargo.log");
    assert!(
        cargo.starts_with("install --locked --force --path "),
        "{cargo}"
    );
    assert!(cargo.contains("unpacked"), "{cargo}");
    let folder = setup.read("folder");
    assert!(!Path::new(folder.trim()).exists(), "left {folder} behind");
}

#[test]
fn stops_when_cargo_is_missing() {
    let setup = Setup::new("no-cargo");
    let output = setup.install();
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("cargo not found on PATH, install Rust from https://rustup.rs"),
        "{}",
        stderr(&output)
    );
    assert!(setup.read("curl.log").is_empty());
}

#[test]
fn stops_when_the_download_fails() {
    let setup = Setup::new("offline").with_cargo();
    setup.script(
        "curl",
        "echo 'curl: (6) Could not resolve host' >&2\nexit 6\n",
    );
    let output = setup.install();
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("Could not resolve host"),
        "{}",
        stderr(&output)
    );
    assert!(setup.read("cargo.log").is_empty());
}
