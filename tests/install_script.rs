#![cfg(unix)]

use std::{
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const LATEST_ZIP: &str = "https://github.com/ravshansbox/rust-coding-agent/releases/latest/download/rust-coding-agent.zip";

struct Setup {
    root: PathBuf,
    bin: PathBuf,
}

impl Setup {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "rust-coding-agent-install-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let mut zip = zip::ZipWriter::new(std::fs::File::create(root.join("fixture.zip")).unwrap());
        zip.start_file(
            "rust-coding-agent/Cargo.toml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(b"[package]\nname = \"rust-coding-agent\"\nversion = \"0.5.0\"\n")
            .unwrap();
        zip.finish().unwrap();
        let setup = Self { root, bin };
        setup.script(
            "curl",
            &format!(
                "echo \"$@\" >> '{root}/curl.log'\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in -o) out=$2 ;; *.minisig) signature=1 ;; esac\n  shift\ndone\nif [ -n \"${{signature:-}}\" ]; then echo signature > \"$out\"; else cp '{root}/fixture.zip' \"$out\"; fi\n",
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

    fn with_minisign(self, exit_code: i32) -> Self {
        self.script(
            "minisign",
            &format!(
                "echo \"$@\" > '{root}/minisign.log'\nfor argument; do case \"$argument\" in *.minisig) cat \"$argument\" >> '{root}/minisign.log' ;; esac; done\nexit {exit_code}\n",
                root = self.root.display()
            ),
        );
        self
    }

    fn install(&self) -> Output {
        Command::new("/bin/sh")
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/install.sh"))
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("HOME", self.root.join("home"))
            .env_remove("CARGO_HOME")
            .env_remove("CARGO_INSTALL_ROOT")
            .output()
            .unwrap()
    }

    fn build(&self) -> PathBuf {
        self.root.join("home/.rust-coding-agent/build")
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.root.join(name)).unwrap_or_default()
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
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
        cargo.starts_with(&format!(
            "install --locked --force --target-dir {} --path ",
            setup.build().display()
        )),
        "{cargo}"
    );
    assert!(cargo.contains("unpacked"), "{cargo}");
    let folder = setup.read("folder");
    assert!(!Path::new(folder.trim()).exists(), "left {folder} behind");
}

#[test]
fn unpacks_the_release_in_the_config_folder() {
    let setup = Setup::new("unpack-folder").with_cargo();
    let output = setup.install();
    assert!(output.status.success(), "{}", stderr(&output));
    let folder = setup.read("folder");
    assert!(
        Path::new(folder.trim()).starts_with(setup.root.join("home/.rust-coding-agent")),
        "{folder}"
    );
}

#[test]
fn checks_the_signature_with_minisign() {
    let setup = Setup::new("signed").with_cargo().with_minisign(0);
    let output = setup.install();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        setup
            .read("curl.log")
            .contains(&format!("{LATEST_ZIP}.minisig")),
        "{}",
        setup.read("curl.log")
    );
    let minisign = setup.read("minisign.log");
    assert!(
        minisign
            .starts_with("-V -q -P RWSa19cgsH6sJ8TomZ0rN68H2QSZmUNhCpxJtnW6IQW/gn6XDhilg5A8 -m "),
        "{minisign}"
    );
    assert!(minisign.ends_with("signature\n"), "{minisign}");
    assert!(!stderr(&output).contains("minisign"), "{}", stderr(&output));
    assert!(setup.read("cargo.log").contains("unpacked"));
}

#[test]
fn stops_when_the_signature_is_not_valid() {
    let setup = Setup::new("bad-signature").with_cargo().with_minisign(1);
    let output = setup.install();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("the signature of rust-coding-agent.zip is not valid"),
        "{}",
        stderr(&output)
    );
    assert_eq!(setup.read("cargo.log"), "");
}

#[test]
fn warns_and_installs_without_minisign() {
    let setup = Setup::new("no-minisign").with_cargo();
    let output = setup.install();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stderr(&output)
            .contains("minisign not found on PATH, installing without checking the signature"),
        "{}",
        stderr(&output)
    );
    assert!(setup.read("cargo.log").contains("unpacked"));
}

#[test]
fn creates_the_config_folder_only_the_user_can_open() {
    let setup = Setup::new("private").with_cargo();
    let output = setup.install();
    assert!(output.status.success(), "{}", stderr(&output));
    let folder = setup.root.join("home/.rust-coding-agent");
    let mode = std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700, "{}", folder.display());
}

#[test]
fn keeps_the_build_folder_while_the_compiler_stays_the_same() {
    let setup = Setup::new("same-compiler").with_cargo();
    setup.script("rustc", "echo 'rustc 1.99.0'\n");
    for _ in 0..2 {
        let output = setup.install();
        assert!(output.status.success(), "{}", stderr(&output));
        std::fs::write(setup.build().join("earlier-build"), "").unwrap();
    }
    assert!(setup.build().join("earlier-build").exists());
}

#[test]
fn empties_the_build_folder_when_the_compiler_changes() {
    let setup = Setup::new("new-compiler").with_cargo();
    for version in ["rustc 1.98.0", "rustc 1.99.0"] {
        setup.script("rustc", &format!("echo '{version}'\n"));
        let output = setup.install();
        assert!(output.status.success(), "{}", stderr(&output));
        std::fs::write(setup.build().join(version), "").unwrap();
    }
    assert!(!setup.build().join("rustc 1.98.0").exists());
    assert!(setup.build().join("rustc 1.99.0").exists());
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

#[test]
fn moves_the_old_config_folder_to_the_new_one() {
    let setup = Setup::new("old-config").with_cargo();
    let old = setup.root.join("home/.rust-claude");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("auth.json"), "{}").unwrap();
    let output = setup.install();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        setup
            .root
            .join("home/.rust-coding-agent/auth.json")
            .exists()
    );
    assert!(!old.exists());
}

#[test]
fn links_rca_to_the_installed_binary() {
    let setup = Setup::new("link").with_cargo();
    let output = setup.install();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        std::fs::read_link(setup.root.join("home/.cargo/bin/rca")).unwrap(),
        Path::new("rust-coding-agent")
    );
}

#[test]
fn keeps_an_existing_rca() {
    let setup = Setup::new("existing-rca").with_cargo();
    let bin = setup.root.join("home/.cargo/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("rca"), "other").unwrap();
    let output = setup.install();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(std::fs::read_to_string(bin.join("rca")).unwrap(), "other");
}
