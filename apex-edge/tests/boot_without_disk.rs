//! The hub must boot where it cannot write next to its working directory when there is
//! nothing to persist: an in-memory database (the Docker smoke test) or auth disabled.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn an_in_memory_hub_with_auth_disabled_boots_and_writes_no_key_file() {
    let cwd = std::env::temp_dir().join(format!("apex-edge-boot-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&cwd).unwrap();
    let port = free_port();
    let mut hub = Command::new(env!("CARGO_BIN_EXE_apex-edge"))
        .current_dir(&cwd)
        .env(
            "APEX_EDGE_DB",
            "sqlite:file:boot_test?mode=memory&cache=shared",
        )
        .env("APEX_EDGE_AUTH_ENABLED", "false")
        .env("APEX_EDGE_BIND", format!("127.0.0.1:{port}"))
        .env_remove("APEX_EDGE_AUTH_SESSION_SIGNING_SECRET")
        .env_remove("APEX_EDGE_AUTH_SESSION_KEY_PATH")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start hub");

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut outcome = Err(format!("hub never started listening on {port}"));
    while Instant::now() < deadline {
        if let Some(status) = hub.try_wait().unwrap() {
            outcome = Err(format!("hub exited during boot: {status}"));
            break;
        }
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            outcome = Ok(());
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = hub.kill();
    let _ = hub.wait();
    outcome.unwrap();
    assert!(
        std::fs::read_dir(&cwd).unwrap().next().is_none(),
        "nothing should be written to the working directory"
    );
}
