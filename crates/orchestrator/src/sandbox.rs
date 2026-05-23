//! Codex Sandbox primitives — runtime image build, launch spec, and the
//! `SandboxLauncher` trait. Wired into Plan Run phases in later slices
//! (issue #74).
//!
//! The Dockerfile and entrypoint are embedded into the binary at compile
//! time so the build is hermetic and the image content hash is a pure
//! function of those two files.
//!
//! See ADR-0041.

use std::path::PathBuf;
use std::process::Command;

use sha2::{Digest, Sha256};

pub const RUNTIME_DOCKERFILE: &str = include_str!("../runtime/Dockerfile");
pub const RUNTIME_ENTRYPOINT: &str = include_str!("../runtime/entrypoint.sh");

pub const RUNTIME_IMAGE_REPO: &str = "agentic-afk-runtime";
pub const MISE_CACHE_VOLUME: &str = "agentic-afk-mise-cache";

/// Content-addressed tag for the runtime image. Hash covers the embedded
/// Dockerfile and entrypoint so any change to either produces a fresh
/// tag and a fresh build.
pub fn runtime_image_tag() -> String {
    let mut hasher = Sha256::new();
    hasher.update(RUNTIME_DOCKERFILE.as_bytes());
    hasher.update(b"\0");
    hasher.update(RUNTIME_ENTRYPOINT.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("{RUNTIME_IMAGE_REPO}:{hex}")
}

/// Codex phase this sandbox is hosting. Drives label values and mount
/// semantics (Planning bind-mounts the Project read-only; the others
/// bind-mount the Assignment Worktree read-write).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SandboxPhase {
    Planning,
    Implementation,
    Review,
    Merge,
}

impl SandboxPhase {
    pub fn as_label(&self) -> &'static str {
        match self {
            SandboxPhase::Planning => "planning",
            SandboxPhase::Implementation => "implementation",
            SandboxPhase::Review => "review",
            SandboxPhase::Merge => "merge",
        }
    }
}

/// One bind mount or named-volume mount in a Codex Sandbox.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SandboxMount {
    /// Host bind mount.
    Bind {
        host_path: PathBuf,
        container_path: PathBuf,
        read_only: bool,
    },
    /// Named Docker volume (e.g. the shared mise cache).
    Volume {
        name: String,
        container_path: PathBuf,
        read_only: bool,
    },
}

/// Everything `DockerSandboxLauncher` needs to translate into a single
/// `docker run --rm` invocation.
#[derive(Clone, Debug)]
pub struct SandboxLaunchSpec {
    pub image_tag: String,
    pub container_name: String,
    pub phase: SandboxPhase,
    pub labels: Vec<(String, String)>,
    pub mounts: Vec<SandboxMount>,
    pub workdir: PathBuf,
    pub env: Vec<(String, String)>,
    /// The full agent invocation argv (e.g. `["codex", "exec", "--...",
    /// prompt]`). Appended after the image tag in `docker run`.
    pub command: Vec<String>,
    pub memory: String,
    pub cpus: String,
    pub pids_limit: u32,
    /// `Some((uid, gid))` runs the container under that identity; `None`
    /// uses the image default (only useful in smoke tests).
    pub user: Option<(u32, u32)>,
}

impl SandboxLaunchSpec {
    /// Standard 8 GiB / 2 vCPU / 512 pid caps from ADR-0041.
    pub fn default_caps() -> (String, String, u32) {
        ("8g".to_string(), "2.0".to_string(), 512)
    }
}

/// Typed failure modes from a launcher invocation. Mapped to the
/// per-phase `PlanRunPhaseError` variant by the caller.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("failed to spawn docker: {0}")]
    Spawn(String),
    #[error("docker run exited with status {status}: {stderr}")]
    NonZero { status: i32, stderr: String },
    #[error("failed to build runtime image: {0}")]
    Build(String),
    #[error("invalid sandbox launch spec: {0}")]
    InvalidSpec(String),
}

/// Translates a `SandboxLaunchSpec` into the argv passed to `docker`.
/// Public for unit-testability — the production launcher just calls
/// `Command::new("docker")` with the result.
pub fn docker_run_args(spec: &SandboxLaunchSpec) -> Vec<String> {
    let mut args = vec![
        "run".to_string(),
        "--rm".to_string(),
        "--name".to_string(),
        spec.container_name.clone(),
        "--memory".to_string(),
        spec.memory.clone(),
        "--cpus".to_string(),
        spec.cpus.clone(),
        "--pids-limit".to_string(),
        spec.pids_limit.to_string(),
        "--restart".to_string(),
        "no".to_string(),
        "--workdir".to_string(),
        spec.workdir.to_string_lossy().into_owned(),
    ];
    if let Some((uid, gid)) = spec.user {
        args.push("--user".to_string());
        args.push(format!("{uid}:{gid}"));
    }
    for (k, v) in &spec.labels {
        args.push("--label".to_string());
        args.push(format!("{k}={v}"));
    }
    for (k, v) in &spec.env {
        args.push("--env".to_string());
        args.push(format!("{k}={v}"));
    }
    for mount in &spec.mounts {
        args.push("--mount".to_string());
        args.push(render_mount(mount));
    }
    args.push(spec.image_tag.clone());
    for piece in &spec.command {
        args.push(piece.clone());
    }
    args
}

fn render_mount(mount: &SandboxMount) -> String {
    match mount {
        SandboxMount::Bind {
            host_path,
            container_path,
            read_only,
        } => format!(
            "type=bind,source={},target={}{}",
            host_path.to_string_lossy(),
            container_path.to_string_lossy(),
            if *read_only { ",readonly" } else { "" }
        ),
        SandboxMount::Volume {
            name,
            container_path,
            read_only,
        } => format!(
            "type=volume,source={name},target={}{}",
            container_path.to_string_lossy(),
            if *read_only { ",readonly" } else { "" }
        ),
    }
}

/// Launches a Codex Sandbox container per spec and returns captured
/// stdout on success.
pub trait SandboxLauncher: Send + Sync {
    fn launch(&self, spec: SandboxLaunchSpec) -> Result<String, SandboxError>;
}

/// Builds the runtime image on demand and returns the content-hash tag.
/// The builder is idempotent: a second call with the same embedded
/// inputs short-circuits via `docker image inspect`.
pub struct RuntimeImageBuilder {
    docker_binary: PathBuf,
}

impl RuntimeImageBuilder {
    pub fn new(docker_binary: impl Into<PathBuf>) -> Self {
        Self {
            docker_binary: docker_binary.into(),
        }
    }

    /// Returns the content-hash image tag, building the image if it is
    /// not already present in the local daemon.
    pub fn ensure(&self) -> Result<String, SandboxError> {
        let tag = runtime_image_tag();
        if self.image_present(&tag)? {
            return Ok(tag);
        }
        self.build(&tag)?;
        Ok(tag)
    }

    fn image_present(&self, tag: &str) -> Result<bool, SandboxError> {
        let output = Command::new(&self.docker_binary)
            .args(["image", "inspect", tag])
            .output()
            .map_err(|e| SandboxError::Spawn(format!("docker image inspect: {e}")))?;
        Ok(output.status.success())
    }

    fn build(&self, tag: &str) -> Result<(), SandboxError> {
        // Materialise the embedded build context into a temp dir so the
        // image build is hermetic — the on-disk runtime/ dir is not
        // required at runtime.
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let ctx_dir = std::env::temp_dir().join(format!(
            "agentic-afk-runtime-build-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&ctx_dir)
            .map_err(|e| SandboxError::Build(format!("create build context: {e}")))?;
        std::fs::write(ctx_dir.join("Dockerfile"), RUNTIME_DOCKERFILE)
            .map_err(|e| SandboxError::Build(format!("write Dockerfile: {e}")))?;
        std::fs::write(ctx_dir.join("entrypoint.sh"), RUNTIME_ENTRYPOINT)
            .map_err(|e| SandboxError::Build(format!("write entrypoint: {e}")))?;

        let output = Command::new(&self.docker_binary)
            .args(["build", "--tag", tag, "."])
            .current_dir(&ctx_dir)
            .output()
            .map_err(|e| SandboxError::Spawn(format!("docker build spawn: {e}")))?;
        let _ = std::fs::remove_dir_all(&ctx_dir);

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            return Err(SandboxError::Build(stderr));
        }
        Ok(())
    }
}

/// Production launcher: shells out to `docker run` with the argv from
/// `docker_run_args` and returns stdout.
pub struct DockerSandboxLauncher {
    docker_binary: PathBuf,
}

impl DockerSandboxLauncher {
    pub fn new(docker_binary: impl Into<PathBuf>) -> Self {
        Self {
            docker_binary: docker_binary.into(),
        }
    }
}

/// Test launcher that records every launch into a queryable list and
/// returns canned stdout. Used by the `DockerCodexRunner` per-phase
/// launch-shape test (issue #74) and any future test that wants to
/// assert what the orchestrator hands to Docker without spawning real
/// containers.
pub struct FakeSandboxLauncher {
    stdouts: std::sync::Mutex<Vec<String>>,
    launches: std::sync::Mutex<Vec<RecordedLaunch>>,
    failure: std::sync::Mutex<Option<SandboxError>>,
}

#[derive(Clone, Debug)]
pub struct RecordedLaunch {
    pub phase: SandboxPhase,
    pub container_name: String,
    pub image_tag: String,
    pub labels: Vec<(String, String)>,
    pub mounts: Vec<SandboxMount>,
    pub workdir: PathBuf,
    pub env: Vec<(String, String)>,
    pub command: Vec<String>,
}

impl FakeSandboxLauncher {
    pub fn new() -> Self {
        Self {
            stdouts: std::sync::Mutex::new(vec!["".to_string()]),
            launches: std::sync::Mutex::new(Vec::new()),
            failure: std::sync::Mutex::new(None),
        }
    }

    pub fn with_stdout(stdout: impl Into<String>) -> Self {
        let launcher = Self::new();
        *launcher.stdouts.lock().unwrap() = vec![stdout.into()];
        launcher
    }

    pub fn with_stdouts<S: Into<String>>(stdouts: impl IntoIterator<Item = S>) -> Self {
        let launcher = Self::new();
        let queued: Vec<String> = stdouts.into_iter().map(Into::into).collect();
        assert!(
            !queued.is_empty(),
            "FakeSandboxLauncher needs at least one stdout"
        );
        *launcher.stdouts.lock().unwrap() = queued;
        launcher
    }

    pub fn fail_with(self, error: SandboxError) -> Self {
        *self.failure.lock().unwrap() = Some(error);
        self
    }

    pub fn launches(&self) -> Vec<RecordedLaunch> {
        self.launches.lock().unwrap().clone()
    }
}

impl Default for FakeSandboxLauncher {
    fn default() -> Self {
        Self::new()
    }
}

impl SandboxLauncher for FakeSandboxLauncher {
    fn launch(&self, spec: SandboxLaunchSpec) -> Result<String, SandboxError> {
        self.launches.lock().unwrap().push(RecordedLaunch {
            phase: spec.phase,
            container_name: spec.container_name,
            image_tag: spec.image_tag,
            labels: spec.labels,
            mounts: spec.mounts,
            workdir: spec.workdir,
            env: spec.env,
            command: spec.command,
        });
        if let Some(err) = self.failure.lock().unwrap().take() {
            return Err(err);
        }
        let mut queue = self.stdouts.lock().unwrap();
        let stdout = if queue.len() == 1 {
            queue[0].clone()
        } else {
            queue.remove(0)
        };
        Ok(stdout)
    }
}

impl SandboxLauncher for DockerSandboxLauncher {
    fn launch(&self, spec: SandboxLaunchSpec) -> Result<String, SandboxError> {
        let args = docker_run_args(&spec);
        let output = Command::new(&self.docker_binary)
            .args(&args)
            .output()
            .map_err(|e| SandboxError::Spawn(format!("docker run: {e}")))?;
        if !output.status.success() {
            return Err(SandboxError::NonZero {
                status: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

// --- Plan Run trigger-time preflight (issue #73) ---

use std::path::Path;
use std::sync::Arc;

use crate::coordinator::CoordinatorError;

/// Probe the Docker daemon for reachability. Trait-fronted so tests can
/// simulate an unavailable daemon without poking the host.
pub trait DockerProbe: Send + Sync {
    fn ping(&self) -> Result<(), String>;
}

/// Ensure the runtime image is present locally, building it if it is
/// not. Returns the resolved image tag on success; on failure returns
/// the `docker build` stderr that the preflight surfaces in the
/// RFC-7807 `detail` field.
pub trait RuntimeImageEnsurer: Send + Sync {
    fn ensure(&self) -> Result<String, String>;
}

/// Production `DockerProbe` that shells out to `docker version`. A
/// non-zero exit (or spawn failure) is treated as "daemon unavailable".
pub struct CliDockerProbe {
    docker_binary: PathBuf,
}

impl CliDockerProbe {
    pub fn new(docker_binary: impl Into<PathBuf>) -> Self {
        Self {
            docker_binary: docker_binary.into(),
        }
    }
}

impl DockerProbe for CliDockerProbe {
    fn ping(&self) -> Result<(), String> {
        let output = Command::new(&self.docker_binary)
            .args(["version", "--format", "{{.Server.Version}}"])
            .output()
            .map_err(|e| format!("docker not invocable: {e}"))?;
        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            Err(format!(
                "docker version exited with {}: {}",
                output.status,
                truncate_stderr(&stderr)
            ))
        }
    }
}

/// `RuntimeImageEnsurer` backed by the on-host `RuntimeImageBuilder`.
pub struct BuilderImageEnsurer {
    builder: RuntimeImageBuilder,
}

impl BuilderImageEnsurer {
    pub fn new(builder: RuntimeImageBuilder) -> Self {
        Self { builder }
    }
}

impl RuntimeImageEnsurer for BuilderImageEnsurer {
    fn ensure(&self) -> Result<String, String> {
        self.builder.ensure().map_err(|e| match e {
            SandboxError::Build(stderr) => truncate_stderr(&stderr),
            other => other.to_string(),
        })
    }
}

/// Cap stderr blobs surfaced in problem-JSON `detail` so a runaway
/// `docker build` log cannot blow up the response size.
fn truncate_stderr(stderr: &str) -> String {
    const LIMIT: usize = 4 * 1024;
    if stderr.len() <= LIMIT {
        return stderr.to_string();
    }
    let head = &stderr[..LIMIT];
    format!("{head}\n…(truncated {} bytes)", stderr.len() - LIMIT)
}

/// Why a Plan Run trigger failed Sandbox preflight. Each variant maps
/// to a stable RFC-7807 problem-type URN at the HTTP boundary.
#[derive(Debug, thiserror::Error)]
pub enum SandboxPreflightFailure {
    #[error("Docker daemon unavailable: {0}")]
    DockerUnavailable(String),
    #[error("Codex auth file not found at {0}")]
    CodexAuthMissing(PathBuf),
    #[error("Project worktree has no mise.toml at {0}")]
    MiseTomlMissing(PathBuf),
    #[error("runtime image build failed: {0}")]
    RuntimeImageBuildFailed(String),
}

impl From<SandboxPreflightFailure> for CoordinatorError {
    fn from(failure: SandboxPreflightFailure) -> Self {
        let (urn, detail) = match &failure {
            SandboxPreflightFailure::DockerUnavailable(detail) => (
                "urn:agentic-afk:sandbox-docker-unavailable",
                format!("Docker daemon unavailable: {detail}"),
            ),
            SandboxPreflightFailure::CodexAuthMissing(path) => (
                "urn:agentic-afk:sandbox-codex-auth-missing",
                format!(
                    "Codex auth file not found at {}. Run `codex login` once to create it.",
                    path.display()
                ),
            ),
            SandboxPreflightFailure::MiseTomlMissing(path) => (
                "urn:agentic-afk:sandbox-mise-toml-missing",
                format!(
                    "Project worktree at {} has no mise.toml. Codex Sandbox toolchain comes from mise.toml.",
                    path.display()
                ),
            ),
            SandboxPreflightFailure::RuntimeImageBuildFailed(stderr) => (
                "urn:agentic-afk:sandbox-runtime-image-build-failed",
                stderr.clone(),
            ),
        };
        CoordinatorError::new(422, urn, detail)
    }
}

/// Trait surface for dependency-injecting the preflight at the HTTP
/// handler boundary. Production wires `SandboxPreflight`; integration
/// tests inject a fake that returns each failure variant to drive the
/// 422 + URN response without simulating real Docker/filesystem state.
pub trait SandboxPreflightCheck: Send + Sync {
    fn check(&self, project_path: &Path) -> Result<(), SandboxPreflightFailure>;
}

impl SandboxPreflightCheck for SandboxPreflight {
    fn check(&self, project_path: &Path) -> Result<(), SandboxPreflightFailure> {
        SandboxPreflight::check(self, project_path)
    }
}

/// Test fake that always passes — used by existing integration tests
/// that drive the Plan Run handler without simulating Sandbox state.
pub struct AlwaysOkSandboxPreflight;

impl SandboxPreflightCheck for AlwaysOkSandboxPreflight {
    fn check(&self, _project_path: &Path) -> Result<(), SandboxPreflightFailure> {
        Ok(())
    }
}

/// Test fake that always returns a fixed failure. Used by the
/// integration test in `apps/control-plane-server/tests/` to assert
/// each preflight URN maps to a 422 problem response.
pub struct RejectingSandboxPreflight {
    template: SandboxFailureTemplate,
}

#[derive(Clone, Debug)]
pub enum SandboxFailureTemplate {
    DockerUnavailable(String),
    CodexAuthMissing(PathBuf),
    MiseTomlMissing(PathBuf),
    RuntimeImageBuildFailed(String),
}

impl RejectingSandboxPreflight {
    pub fn new(template: SandboxFailureTemplate) -> Self {
        Self { template }
    }

    fn make(&self) -> SandboxPreflightFailure {
        match &self.template {
            SandboxFailureTemplate::DockerUnavailable(d) => {
                SandboxPreflightFailure::DockerUnavailable(d.clone())
            }
            SandboxFailureTemplate::CodexAuthMissing(p) => {
                SandboxPreflightFailure::CodexAuthMissing(p.clone())
            }
            SandboxFailureTemplate::MiseTomlMissing(p) => {
                SandboxPreflightFailure::MiseTomlMissing(p.clone())
            }
            SandboxFailureTemplate::RuntimeImageBuildFailed(s) => {
                SandboxPreflightFailure::RuntimeImageBuildFailed(s.clone())
            }
        }
    }
}

impl SandboxPreflightCheck for RejectingSandboxPreflight {
    fn check(&self, _project_path: &Path) -> Result<(), SandboxPreflightFailure> {
        Err(self.make())
    }
}

/// Plan Run trigger-time preflight for the Codex Sandbox. Four checks
/// in fixed order; first failure wins. Holds host-wide collaborators
/// (docker probe, codex auth path, runtime-image builder) behind small
/// traits so unit tests can drive each branch without touching the host.
/// The per-Plan-Run project path is supplied to `check`.
pub struct SandboxPreflight {
    docker: Arc<dyn DockerProbe>,
    codex_auth_path: PathBuf,
    image: Arc<dyn RuntimeImageEnsurer>,
}

impl SandboxPreflight {
    pub fn new(
        docker: Arc<dyn DockerProbe>,
        codex_auth_path: impl Into<PathBuf>,
        image: Arc<dyn RuntimeImageEnsurer>,
    ) -> Self {
        Self {
            docker,
            codex_auth_path: codex_auth_path.into(),
            image,
        }
    }

    /// Single entrypoint. Order mirrors ADR-0041 §"preflight": docker
    /// daemon → codex auth → mise.toml → runtime image. Stops at the
    /// first failure so an unreachable daemon does not cascade into a
    /// meaningless image-build attempt. Sync because all four checks
    /// are blocking I/O and the host trigger handler is the only caller.
    pub fn check(&self, project_path: &Path) -> Result<(), SandboxPreflightFailure> {
        self.docker
            .ping()
            .map_err(SandboxPreflightFailure::DockerUnavailable)?;

        if !file_readable(&self.codex_auth_path) {
            return Err(SandboxPreflightFailure::CodexAuthMissing(
                self.codex_auth_path.clone(),
            ));
        }

        let mise_toml = project_path.join("mise.toml");
        if !mise_toml.is_file() {
            return Err(SandboxPreflightFailure::MiseTomlMissing(
                project_path.to_path_buf(),
            ));
        }

        self.image
            .ensure()
            .map(|_tag| ())
            .map_err(SandboxPreflightFailure::RuntimeImageBuildFailed)
    }
}

fn file_readable(path: &Path) -> bool {
    std::fs::File::open(path).is_ok()
}

#[cfg(test)]
mod preflight_tests {
    use super::*;

    struct OkDocker;
    impl DockerProbe for OkDocker {
        fn ping(&self) -> Result<(), String> {
            Ok(())
        }
    }
    struct FailDocker(&'static str);
    impl DockerProbe for FailDocker {
        fn ping(&self) -> Result<(), String> {
            Err(self.0.to_string())
        }
    }

    struct OkImage(&'static str);
    impl RuntimeImageEnsurer for OkImage {
        fn ensure(&self) -> Result<String, String> {
            Ok(self.0.to_string())
        }
    }
    struct FailImage(String);
    impl RuntimeImageEnsurer for FailImage {
        fn ensure(&self) -> Result<String, String> {
            Err(self.0.clone())
        }
    }

    fn tmpdir(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let p = std::env::temp_dir().join(format!(
            "agentic-afk-preflight-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn write_auth(dir: &Path) -> PathBuf {
        let auth = dir.join("auth.json");
        std::fs::write(&auth, "{}").unwrap();
        auth
    }

    fn write_mise(dir: &Path) -> &Path {
        std::fs::write(dir.join("mise.toml"), "[tools]\n").unwrap();
        dir
    }

    #[test]
    fn happy_path_all_four_checks_pass() {
        let dir = tmpdir("happy");
        let auth = write_auth(&dir);
        write_mise(&dir);
        let pf = SandboxPreflight::new(
            Arc::new(OkDocker),
            &auth,
            Arc::new(OkImage("agentic-afk-runtime:abc")),
        );
        pf.check(&dir).expect("preflight passes");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn docker_unavailable_short_circuits() {
        let dir = tmpdir("docker-down");
        // No auth, no mise.toml — still expect DockerUnavailable
        // because docker is checked first.
        let pf = SandboxPreflight::new(
            Arc::new(FailDocker("connect refused")),
            dir.join("missing-auth.json"),
            Arc::new(OkImage("t")),
        );
        let err = pf.check(&dir).unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(matches!(
            err,
            SandboxPreflightFailure::DockerUnavailable(detail) if detail.contains("connect refused")
        ));
    }

    #[test]
    fn codex_auth_missing_after_docker_passes() {
        let dir = tmpdir("auth-missing");
        write_mise(&dir);
        let auth = dir.join("missing-auth.json");
        let pf = SandboxPreflight::new(
            Arc::new(OkDocker),
            &auth,
            Arc::new(OkImage("t")),
        );
        let err = pf.check(&dir).unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(matches!(
            err,
            SandboxPreflightFailure::CodexAuthMissing(p) if p == auth
        ));
    }

    #[test]
    fn mise_toml_missing_after_auth_passes() {
        let dir = tmpdir("mise-missing");
        let auth = write_auth(&dir);
        let pf = SandboxPreflight::new(
            Arc::new(OkDocker),
            &auth,
            Arc::new(OkImage("t")),
        );
        let err = pf.check(&dir).unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(matches!(
            err,
            SandboxPreflightFailure::MiseTomlMissing(p) if p == dir
        ));
    }

    #[test]
    fn runtime_image_build_failure_surfaces_stderr_in_detail() {
        let dir = tmpdir("img-fail");
        let auth = write_auth(&dir);
        write_mise(&dir);
        let stderr = "step 5/7: COPY entrypoint.sh — file not found";
        let pf = SandboxPreflight::new(
            Arc::new(OkDocker),
            &auth,
            Arc::new(FailImage(stderr.to_string())),
        );
        let err = pf.check(&dir).unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        let coordinator_err: CoordinatorError = err.into();
        assert_eq!(coordinator_err.status, 422);
        assert_eq!(
            coordinator_err.problem_type,
            "urn:agentic-afk:sandbox-runtime-image-build-failed"
        );
        assert!(coordinator_err.detail.contains("entrypoint.sh"));
    }

    #[test]
    fn coordinator_error_status_is_422_for_all_variants() {
        let urns = [
            (
                SandboxPreflightFailure::DockerUnavailable("x".into()),
                "urn:agentic-afk:sandbox-docker-unavailable",
            ),
            (
                SandboxPreflightFailure::CodexAuthMissing("/x".into()),
                "urn:agentic-afk:sandbox-codex-auth-missing",
            ),
            (
                SandboxPreflightFailure::MiseTomlMissing("/x".into()),
                "urn:agentic-afk:sandbox-mise-toml-missing",
            ),
            (
                SandboxPreflightFailure::RuntimeImageBuildFailed("x".into()),
                "urn:agentic-afk:sandbox-runtime-image-build-failed",
            ),
        ];
        for (failure, expected_urn) in urns {
            let err: CoordinatorError = failure.into();
            assert_eq!(err.status, 422);
            assert_eq!(err.problem_type, expected_urn);
        }
    }

    #[test]
    fn truncate_stderr_caps_long_blobs() {
        let big = "x".repeat(5000);
        let out = truncate_stderr(&big);
        assert!(out.len() < big.len());
        assert!(out.contains("truncated"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_image_tag_uses_repo_and_short_hex() {
        let tag = runtime_image_tag();
        let (repo, hex) = tag.split_once(':').expect("tag has colon");
        assert_eq!(repo, RUNTIME_IMAGE_REPO);
        assert_eq!(hex.len(), 12, "12 hex chars from 6 bytes");
        assert!(
            hex.chars().all(|c| c.is_ascii_hexdigit()),
            "tag suffix is hex"
        );
    }

    #[test]
    fn runtime_image_tag_is_deterministic_across_calls() {
        assert_eq!(runtime_image_tag(), runtime_image_tag());
    }

    #[test]
    fn docker_run_args_includes_rm_caps_workdir_and_image_last() {
        let spec = SandboxLaunchSpec {
            image_tag: "agentic-afk-runtime:abc123".to_string(),
            container_name: "agentic-afk-assignment-aaa".to_string(),
            phase: SandboxPhase::Planning,
            labels: vec![("agentic-afk.phase".to_string(), "planning".to_string())],
            mounts: vec![SandboxMount::Bind {
                host_path: "/host/proj".into(),
                container_path: "/work".into(),
                read_only: true,
            }],
            workdir: "/work".into(),
            env: vec![("HOME".to_string(), "/tmp/codex-home".to_string())],
            command: vec!["codex".to_string(), "exec".to_string(), "go".to_string()],
            memory: "8g".to_string(),
            cpus: "2.0".to_string(),
            pids_limit: 512,
            user: Some((1000, 1000)),
        };
        let args = docker_run_args(&spec);
        assert_eq!(args.first().map(String::as_str), Some("run"));
        assert!(args.iter().any(|a| a == "--rm"));
        assert!(args.windows(2).any(|w| w[0] == "--memory" && w[1] == "8g"));
        assert!(args.windows(2).any(|w| w[0] == "--cpus" && w[1] == "2.0"));
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--pids-limit" && w[1] == "512")
        );
        assert!(args.windows(2).any(|w| w[0] == "--user" && w[1] == "1000:1000"));
        assert!(args.windows(2).any(|w| w[0] == "--workdir" && w[1] == "/work"));
        assert!(args.windows(2).any(
            |w| w[0] == "--label" && w[1] == "agentic-afk.phase=planning"
        ));
        assert!(args.windows(2).any(
            |w| w[0] == "--env" && w[1] == "HOME=/tmp/codex-home"
        ));
        let image_idx = args
            .iter()
            .position(|a| a == "agentic-afk-runtime:abc123")
            .expect("image tag present");
        assert_eq!(
            args[image_idx + 1..],
            ["codex", "exec", "go"],
            "command follows image tag"
        );
    }

    #[test]
    fn docker_run_args_renders_bind_mount_readonly() {
        let spec = sample_spec(vec![SandboxMount::Bind {
            host_path: "/h/x".into(),
            container_path: "/c/x".into(),
            read_only: true,
        }]);
        let args = docker_run_args(&spec);
        let mount = args
            .windows(2)
            .find(|w| w[0] == "--mount")
            .map(|w| w[1].clone())
            .expect("bind mount rendered");
        assert_eq!(mount, "type=bind,source=/h/x,target=/c/x,readonly");
    }

    #[test]
    fn docker_run_args_renders_named_volume_mount_rw() {
        let spec = sample_spec(vec![SandboxMount::Volume {
            name: MISE_CACHE_VOLUME.to_string(),
            container_path: "/cache/mise".into(),
            read_only: false,
        }]);
        let args = docker_run_args(&spec);
        let mount = args
            .windows(2)
            .find(|w| w[0] == "--mount")
            .map(|w| w[1].clone())
            .expect("volume mount rendered");
        assert_eq!(
            mount,
            "type=volume,source=agentic-afk-mise-cache,target=/cache/mise"
        );
    }

    fn sample_spec(mounts: Vec<SandboxMount>) -> SandboxLaunchSpec {
        SandboxLaunchSpec {
            image_tag: "agentic-afk-runtime:000000000000".to_string(),
            container_name: "agentic-afk-assignment-test".to_string(),
            phase: SandboxPhase::Implementation,
            labels: vec![],
            mounts,
            workdir: "/work".into(),
            env: vec![],
            command: vec!["true".to_string()],
            memory: "8g".to_string(),
            cpus: "2.0".to_string(),
            pids_limit: 512,
            user: None,
        }
    }
}
