//! Local sandboxes: boot under an AMC lease, and thin wrappers over microsandbox.
use crate::pool::Pool;
use amc_admission::client::{Exec, ExecRequest};
use anyhow::{Context, Result, bail, ensure};
use microsandbox::{Sandbox, SecretSource};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{path::PathBuf, process::Command, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::Duration};
use tokio::signal::unix::{SignalKind, signal};

/// Guest overhead on top of the VM's RAM when sizing the AMC reservation.
const OVERHEAD_MIB: u64 = 512;
const PREFIX: &str = "mu-";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Spec {
    pub name: Option<String>,
    pub repo: Option<String>,
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,
    pub cpus: Option<u8>,
    pub memory_mib: Option<u32>,
    /// Fork this local sandbox instead of cloning a repo.
    pub fork_of: Option<String>,
}

fn safe(s: &str, extra: &str) -> bool {
    !s.is_empty() && !s.starts_with('-') && s.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c) || extra.contains(c))
}

impl Spec {
    /// Trust boundary: repo and ref are interpolated into a guest shell script.
    fn validate(&self) -> Result<()> {
        if let Some(n) = &self.name {
            ensure!(safe(n, ""), "bad sandbox name");
        }
        if self.fork_of.is_none() {
            ensure!(self.repo.as_deref().is_some_and(|r| safe(r, "/")), "repo must be owner/name");
            ensure!(self.git_ref.as_deref().is_some_and(|r| safe(r, "/")), "ref has unsafe characters");
        }
        Ok(())
    }
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default().trim().into()
}

fn suffix() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    format!("{:08x}", t.subsec_nanos() ^ std::process::id())
}

fn ready_file(name: &str) -> Result<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR unset")?).join("mutiny");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("{name}.ready")))
}

/// `host run`: start `mutiny host admit` in a transient user unit (independent of this SSH session)
/// and print the id once the sandbox is ready.
pub async fn run(pool: &Pool, spec_json: &str) -> Result<()> {
    let mut spec: Spec = serde_json::from_str(spec_json).context("parse --spec")?;
    spec.validate()?;
    let name = spec.name.get_or_insert_with(|| format!("{PREFIX}{}", suffix())).clone();
    let ready = ready_file(&name)?;
    let _ = std::fs::remove_file(&ready);
    let unit = format!("mutiny-{name}");
    let status = Command::new("systemd-run")
        .args(["--user", "--quiet", "--collect", "--unit", &unit])
        .arg(std::env::current_exe()?)
        .arg("--pool").arg(&pool.path)
        .args(["host", "admit", "--spec", &serde_json::to_string(&spec)?])
        .status()?;
    ensure!(status.success(), "systemd-run failed");
    for _ in 0..600 {
        if ready.exists() {
            println!("{}", json!({ "id": format!("{}:{name}", hostname()) }));
            return Ok(());
        }
        if !Command::new("systemctl").args(["--user", "--quiet", "is-active", &unit]).status()?.success() {
            bail!("unit {unit} ended before the sandbox was ready; see journalctl --user -u {unit}");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    bail!("sandbox {name} not ready after 600s")
}

/// `host admit` (runs inside the transient unit): wait for AMC admission, then run `host boot`
/// under the lease. The lease lasts exactly as long as the boot process.
pub async fn admit(pool: &Pool, spec_json: &str) -> Result<()> {
    let spec: Spec = serde_json::from_str(spec_json)?;
    let mem = (u64::from(spec.memory_mib.unwrap_or(2048)) + OVERHEAD_MIB) << 20;
    let amc = std::env::var_os("PATH")
        .and_then(|p| std::env::split_paths(&p).map(|d| d.join("amc")).find(|c| c.is_file()))
        .context("amc not on PATH")?;
    let mut req = ExecRequest::queued(&pool.contract, mem, 86400, amc)?;
    // ponytail: 24h lease cap (AMC's runtime ceiling) and 5 min queue wait; make configurable if either bites
    req.wait = Some(Duration::from_secs(300));
    let mut exec = Exec::new(req, [std::env::current_exe()?.as_os_str(), "--pool".as_ref(), pool.path.as_os_str(), "host".as_ref(), "boot".as_ref(), "--spec".as_ref(), spec_json.as_ref()]);
    exec.env("XDG_RUNTIME_DIR", std::env::var("XDG_RUNTIME_DIR")?);
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let mut term = signal(SignalKind::terminate())?;
    tokio::spawn(async move {
        term.recv().await;
        flag.store(true, Ordering::SeqCst);
    });
    let code = tokio::task::spawn_blocking(move || exec.run(|| stop.load(Ordering::SeqCst).then_some(15))).await??;
    std::process::exit(code)
}

/// `host boot` (runs inside the AMC unit): boot the VM, set it up, hold it until it exits.
pub async fn boot(pool: &Pool, spec_json: &str) -> Result<()> {
    let spec: Spec = serde_json::from_str(spec_json)?;
    spec.validate()?;
    let name = spec.name.clone().context("spec has no name")?;
    let sandbox = if let Some(src) = &spec.fork_of {
        Sandbox::get(src).await?.connect().await?.fork(&name).fork().await?
    } else {
        let cache = pool.sccache_dir.to_string_lossy().into_owned();
        let mut b = Sandbox::builder(&name)
            .image("alpine") // ponytail: stock alpine + apk add at boot (slow start, musl); bake an image when start latency matters
            .cpus(spec.cpus.unwrap_or(2))
            .memory(spec.memory_mib.unwrap_or(2048))
            .volume(cache.clone(), |v| v.bind(cache.clone()))
            .env("RUSTC_WRAPPER", "sccache")
            .env("SCCACHE_DIR", cache);
        for (key, file) in &pool.secret_files {
            // Value goes file -> env -> microsandbox `source`, so it is never in the spec, the DB or logs.
            let var = format!("MUTINY_SECRET_{key}");
            // SAFETY: still single-threaded here apart from tokio workers that never read the environment.
            unsafe { std::env::set_var(&var, std::fs::read_to_string(file).with_context(|| format!("read secret {key}"))?.trim()) };
            // ponytail: every secret is released only to GitHub hosts; add per-secret hosts to pool.json for other providers
            b = b.secret(|s| s.env(key).source(SecretSource::env(var)).allow("github.com").allow("api.github.com"));
        }
        b.create().await?
    };
    if spec.fork_of.is_none() {
        let (repo, git_ref) = (spec.repo.as_deref().unwrap_or_default(), spec.git_ref.as_deref().unwrap_or_default());
        let auth = if pool.secret_files.contains_key("GITHUB_TOKEN") { "x-access-token:$GITHUB_TOKEN@" } else { "" };
        let script = format!("set -e; apk add --no-cache git sccache >/dev/null; git clone -q https://{auth}github.com/{repo}.git /work/repo; cd /work/repo; git checkout -q {git_ref}");
        let out = sandbox.shell(script).await?;
        if !out.status().success {
            let err = out.stderr().unwrap_or_default();
            let _ = sandbox.destroy().await;
            bail!("setup failed ({}): {}", out.status().code, err.trim());
        }
    }
    let ready = ready_file(&name)?;
    std::fs::write(&ready, "")?;
    let mut term = signal(SignalKind::terminate())?;
    tokio::select! {
        _ = sandbox.wait() => {}
        _ = term.recv() => { let _ = sandbox.stop().await; }
    }
    let _ = std::fs::remove_file(ready);
    Ok(())
}

async fn connect(name: &str) -> Result<Sandbox> {
    Ok(Sandbox::get(name).await?.connect().await?)
}

pub async fn exec(name: &str, command: &str) -> Result<()> {
    let out = connect(name).await?.exec_with("sh", |e| e.args(["-c", command]).cwd("/work/repo")).await?;
    println!("{}", json!({
        "exitCode": out.status().code,
        "stdout": String::from_utf8_lossy(out.stdout_bytes()),
        "stderr": String::from_utf8_lossy(out.stderr_bytes()),
    }));
    Ok(())
}

pub async fn read(name: &str, path: &str) -> Result<()> {
    use std::io::Write;
    std::io::stdout().write_all(&connect(name).await?.fs().read(path).await?)?;
    Ok(())
}

pub async fn write(name: &str, path: &str) -> Result<()> {
    let mut data = Vec::new();
    std::io::Read::read_to_end(&mut std::io::stdin(), &mut data)?;
    connect(name).await?.fs().write(path, data).await?;
    Ok(())
}

pub async fn snapshot(name: &str) -> Result<()> {
    let snap = format!("{name}-{}", suffix());
    Sandbox::get(name).await?.snapshot(&snap).await?;
    println!("{}", json!({ "snapshot": snap }));
    Ok(())
}

pub async fn destroy(name: &str) -> Result<()> {
    Sandbox::get(name).await?.destroy().await?;
    Ok(())
}

pub async fn list() -> Result<()> {
    // ponytail: first page only (default page size); follow next_cursor if a host holds more sandboxes
    let page = Sandbox::list().await?;
    let host = hostname();
    let items: Vec<_> = page.sandboxes.iter().filter(|s| s.name().starts_with(PREFIX))
        .map(|s| json!({ "id": format!("{host}:{}", s.name()), "status": format!("{:?}", s.status_snapshot()) })).collect();
    println!("{}", serde_json::to_string(&items)?);
    Ok(())
}
