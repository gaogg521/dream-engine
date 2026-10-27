use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use super::StdioTransport;
use crate::transport::McpTransport;

// Invoked only as a child of the containment tests. No npm or network access.
#[test]
#[ignore = "subprocess fixture"]
fn process_tree_fixture() {
    let Ok(path) = std::env::var("MCP_TREE_PID_FILE") else {
        return;
    };
    let depth: usize = std::env::var("MCP_TREE_DEPTH").unwrap().parse().unwrap();
    writeln!(
        OpenOptions::new().create(true).append(true).open(&path).unwrap(),
        "{}",
        std::process::id()
    )
    .unwrap();
    if depth < 2 {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--ignored",
                "--exact",
                "transport::stdio::stdio_test::process_tree_fixture",
            ])
            .env("MCP_TREE_DEPTH", (depth + 1).to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        command.spawn().unwrap().wait().unwrap();
    } else {
        std::thread::sleep(Duration::from_secs(30));
    }
}

fn alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        && stat.rsplit_once(") ").is_some_and(|(_, state)| state.starts_with('Z'))
    {
        // An orphan waiting for init to reap it cannot execute or hold an MCP
        // connection; kill -0 alone also reports these zombies as alive.
        return false;
    }
    #[cfg(windows)]
    {
        Command::new("tasklist.exe")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).contains(&format!("\",\"{pid}\",")))
    }
    #[cfg(not(windows))]
    {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}

async fn exercise_tree(close: bool) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pids");
    let env = HashMap::from([
        ("MCP_TREE_PID_FILE".into(), path.to_string_lossy().into_owned()),
        ("MCP_TREE_DEPTH".into(), "0".into()),
    ]);
    let transport = StdioTransport::spawn(
        std::env::current_exe().unwrap().to_str().unwrap(),
        &[
            "--ignored".into(),
            "--exact".into(),
            "transport::stdio::stdio_test::process_tree_fixture".into(),
        ],
        &env,
    )
    .await
    .unwrap();
    let pids = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pids: Vec<u32> = std::fs::read_to_string(&path)
                .unwrap_or_default()
                .lines()
                .filter_map(|line| line.parse().ok())
                .collect();
            if pids.len() == 3 {
                break pids;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("fixture must spawn root, child and grandchild");
    assert!(
        pids.iter().all(|pid| alive(*pid)),
        "all fixture processes must be live before teardown"
    );
    if close {
        transport.close().await.unwrap();
    }
    drop(transport);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if pids.iter().all(|pid| !alive(*pid)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("MCP teardown must reap the whole process tree");
}

#[tokio::test]
async fn close_reaps_launcher_children_and_grandchildren() {
    exercise_tree(true).await;
}

#[tokio::test]
async fn canceled_initialization_drop_reaps_process_tree() {
    exercise_tree(false).await;
}
