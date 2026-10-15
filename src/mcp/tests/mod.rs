use std::{
    collections::{BTreeMap, HashMap},
    fmt::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use tokio::io::AsyncWriteExt;

use super::config_file::{OAuthConfig, expand_variables, read_config};
use super::server::start;
use super::sign_in::{begin_sign_in_with, sign_out_with};
use super::startup::restart_from;
use super::*;
use crate::agent::test_support::read_chunk;
use crate::auth::now_millis;
use crate::tools::test_support::{TemporaryDir, TemporaryFile};

mod config_errors;
mod config_files;
mod http;
mod http_server;
mod servers;
pub(crate) mod sign_in;
mod stdio;
mod tool;

use http_server::*;

#[test]
fn leaves_an_unclosed_variable_as_written() {
    assert_eq!(
        expand_variables("Bearer ${TOKEN"),
        Ok("Bearer ${TOKEN".into())
    );
}

fn echo_server() -> ServerConfig {
    server_answering_calls_with(
        r#"printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"pong"}]}}\n' "$id""#,
    )
}

pub(crate) async fn echo_start(name: &str) -> Started {
    start(name.into(), Scope::Project, echo_server()).await
}

fn server_answering_calls_with(call: &str) -> ServerConfig {
    server_with("", call)
}

/// A bash server; `cases` are extra `case` arms tried before the defaults.
fn server_with(cases: &str, call: &str) -> ServerConfig {
    let script = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
CASES
    *'"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"t","version":"1"}}}\n' "$id" ;;
    *'"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","inputSchema":{"type":"object"}}]}}\n' "$id" ;;
    *'"tools/call"'*) CALL ;;
  esac
done
"#
        .replace("CASES", cases)
        .replace("CALL", call);
    ServerConfig {
        command: Some("bash".into()),
        args: vec!["-c".into(), script],
        timeout: Some(5),
        ..Default::default()
    }
}

async fn eventually<T>(mut value: impl FnMut() -> T, done: impl Fn(&T) -> bool) -> T {
    for _ in 1..50 {
        let current = value();
        if done(&current) {
            return current;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    value()
}

async fn wait_until_gone(pid: &str) -> String {
    let listed = || {
        let output = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", pid])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    eventually(listed, String::is_empty).await
}
