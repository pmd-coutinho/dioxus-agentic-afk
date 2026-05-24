//! Real-Docker smoke test for the runtime image and the mise install
//! path (issue #72). Excluded from default `cargo test`; gated by the
//! `docker-smoke` Cargo feature on the orchestrator crate.
//!
//! Invoke with:
//!   cargo test -p agentic-afk-orchestrator --features docker-smoke --test docker_smoke
//!
//! Requires:
//!   - Docker daemon reachable from the current process.
//!   - The repository's `mise.toml` at the workspace root (used as the
//!     bind-mounted project).

#![cfg(feature = "docker-smoke")]

use std::path::PathBuf;
use std::process::Command;

use agentic_afk_orchestrator::sandbox::{
    DockerSandboxLauncher, MISE_CACHE_VOLUME, RuntimeImageBuilder, SandboxLaunchSpec,
    SandboxLauncher, SandboxMount, SandboxPhase, runtime_image_tag,
};

fn workspace_root() -> PathBuf {
    // crates/orchestrator/tests -> ../../..
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

fn current_uid_gid() -> (u32, u32) {
    let uid = String::from_utf8(Command::new("id").arg("-u").output().unwrap().stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let gid = String::from_utf8(Command::new("id").arg("-g").output().unwrap().stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    (uid, gid)
}

#[test]
fn smoke_build_and_mise_install_against_repo_mise_toml() {
    let project = workspace_root();
    assert!(
        project.join("mise.toml").exists(),
        "smoke test expects a mise.toml at the workspace root"
    );

    let builder = RuntimeImageBuilder::new("docker");
    let tag = builder.ensure().expect("runtime image builds");
    assert_eq!(tag, runtime_image_tag());

    let launcher = DockerSandboxLauncher::new("docker");
    let (uid, gid) = current_uid_gid();

    let base_spec = |command: Vec<String>, name: &str| SandboxLaunchSpec {
        image_tag: tag.clone(),
        container_name: name.to_string(),
        phase: SandboxPhase::Planning,
        labels: vec![
            ("agentic-afk.phase".to_string(), "planning".to_string()),
            ("agentic-afk.smoke-test".to_string(), "issue-72".to_string()),
        ],
        mounts: vec![
            SandboxMount::Bind {
                host_path: project.clone(),
                container_path: "/work".into(),
                read_only: true,
            },
            SandboxMount::Volume {
                name: MISE_CACHE_VOLUME.to_string(),
                container_path: "/cache/mise".into(),
                read_only: false,
            },
        ],
        workdir: "/work".into(),
        env: vec![("HOME".to_string(), "/tmp/codex-home".to_string())],
        command,
        memory: "8g".to_string(),
        cpus: "2.0".to_string(),
        pids_limit: 512,
        user: Some((uid, gid)),
    };

    let version_out = launcher
        .launch(
            base_spec(
                vec!["mise".to_string(), "--version".to_string()],
                "agentic-afk-smoke-mise-version",
            ),
            None,
        )
        .expect("mise --version succeeds in container");
    assert!(
        !version_out.trim().is_empty(),
        "mise --version produced output"
    );

    // First mise install — primes the named cache volume.
    let t0 = std::time::Instant::now();
    launcher
        .launch(
            base_spec(
                vec!["mise".to_string(), "install".to_string()],
                "agentic-afk-smoke-mise-install-1",
            ),
            None,
        )
        .expect("mise install (first run) succeeds");
    let first_elapsed = t0.elapsed();

    // Second mise install — validates the cache volume remains reusable
    // across sandbox launches. Wall-clock speed depends on the host and
    // on mise's current plugin behavior, so this is intentionally not a
    // performance assertion.
    let t1 = std::time::Instant::now();
    launcher
        .launch(
            base_spec(
                vec!["mise".to_string(), "install".to_string()],
                "agentic-afk-smoke-mise-install-2",
            ),
            None,
        )
        .expect("mise install (second run) succeeds");
    let second_elapsed = t1.elapsed();

    eprintln!(
        "smoke: mise install first={:?} second={:?}",
        first_elapsed, second_elapsed
    );

    // Best-effort: confirm the cache volume exists after the runs.
    let inspect = Command::new("docker")
        .args(["volume", "inspect", MISE_CACHE_VOLUME])
        .output()
        .expect("docker volume inspect");
    assert!(
        inspect.status.success(),
        "mise cache volume should exist after first install"
    );
}
