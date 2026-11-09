//! Local sandboxes: boot under an AMC lease, and thin wrappers over microsandbox.
use crate::pool::Pool;
use anyhow::{Context, Result, bail, ensure};
use microsandbox::{
    Sandbox, SecretSource,
    sandbox::{MAX_SANDBOX_LIST_LIMIT, SandboxBuilder},
    size::SizeExt,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{path::PathBuf, process::Command, time::Duration};
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
    !s.is_empty()
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c) || extra.contains(c))
}

/// Trust boundary: names reach the microsandbox DB and systemd unit names; only ours (`mu-…`).
fn check_name(n: &str) -> Result<()> {
    ensure!(safe(n, "") && n.starts_with(PREFIX), "bad sandbox name");
    Ok(())
}

impl Spec {
    /// Trust boundary: repo and ref are interpolated into a guest shell script.
    fn validate(&self) -> Result<()> {
        if let Some(n) = &self.name {
            check_name(n)?;
        }
        if let Some(n) = &self.fork_of {
            check_name(n)?;
        } else {
            ensure!(
                self.repo.as_deref().is_some_and(|r| {
                    safe(r, "/")
                        && r.split_once('/').is_some_and(|(o, n)| {
                            !n.contains('/')
                                && ![o, n].iter().any(|p| matches!(*p, "" | "." | ".."))
                        })
                }),
                "repo must be owner/name"
            );
            ensure!(
                self.git_ref.as_deref().is_some_and(|r| safe(r, "/")),
                "ref has unsafe characters"
            );
        }
        Ok(())
    }
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .unwrap_or_default()
        .trim()
        .into()
}

fn suffix() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{:08x}", t.subsec_nanos() ^ std::process::id())
}

fn ready_file(name: &str) -> Result<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR unset")?)
        .join("mutiny");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("{name}.ready")))
}

/// Variables a wrapper sets for us (runtime store paths, amc on PATH). Transient units start
/// from the user manager's environment and `current_exe()` is the unwrapped binary, so each
/// detach must carry them explicitly.
fn carried() -> impl Iterator<Item = (&'static str, String)> {
    ["PATH", "MSB_PATH", "MSB_LIBKRUNFW_PATH", "MSB_HOME"]
        .into_iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (k, v)))
}

/// `host run`: start `mutiny host admit` in a transient user unit (independent of this SSH session)
/// and print the id once the sandbox is ready.
pub async fn run(pool: &Pool, spec_json: &str) -> Result<()> {
    let mut spec: Spec = serde_json::from_str(spec_json).context("parse --spec")?;
    spec.validate()?;
    let name = spec
        .name
        .get_or_insert_with(|| format!("{PREFIX}{}", suffix()))
        .clone();
    let ready = ready_file(&name)?;
    let _ = std::fs::remove_file(&ready);
    let unit = format!("mutiny-{name}");
    let status = Command::new("systemd-run")
        .args(["--user", "--quiet", "--collect", "--unit", &unit])
        .args(carried().map(|(k, v)| format!("--setenv={k}={v}")))
        .arg(std::env::current_exe()?)
        .arg("--pool")
        .arg(&pool.path)
        .args(["host", "admit", "--spec", &serde_json::to_string(&spec)?])
        .status()?;
    ensure!(status.success(), "systemd-run failed");
    for _ in 0..600 {
        if ready.exists() {
            println!("{}", json!({ "id": format!("{}:{name}", hostname()) }));
            return Ok(());
        }
        if !Command::new("systemctl")
            .args(["--user", "--quiet", "is-active", &unit])
            .status()?
            .success()
        {
            bail!(
                "unit {unit} ended before the sandbox was ready; see journalctl --user -u {unit}"
            );
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    bail!("sandbox {name} not ready after 600s")
}

/// `host admit` (runs inside the transient unit): queue for AMC admission, then run `host boot`
/// under the lease. The lease lasts exactly as long as the boot process.
pub async fn admit(pool: &Pool, spec_json: &str) -> Result<()> {
    let spec: Spec = serde_json::from_str(spec_json)?;
    let mem = u64::from(spec.memory_mib.unwrap_or(2048)) + OVERHEAD_MIB;
    // ponytail: 24h lease cap (AMC's runtime ceiling) and 5 min queue wait; make configurable if either bites
    // amc forwards this environment (PATH, MSB_*, XDG_RUNTIME_DIR) to the admitted command.
    let mut amc = tokio::process::Command::new("amc")
        .args(["admission", "exec", "--queue", "--contract", &pool.contract])
        .args(["--max-ram-usage", &format!("{mem}MiB")])
        .args(["--runtime-max-sec", "86400", "--timeout", "300", "--"])
        .arg(std::env::current_exe()?)
        .arg("--pool")
        .arg(&pool.path)
        .args(["host", "boot", "--spec", spec_json])
        .spawn()
        .context("run amc")?;
    let mut term = signal(SignalKind::terminate())?;
    let status = tokio::select! {
        s = amc.wait() => s?,
        _ = term.recv() => {
            // amc cancels or stops the admitted command on SIGTERM and releases the lease.
            if let Some(pid) = amc.id() {
                let _ = Command::new("kill").args(["-TERM", &pid.to_string()]).status();
            }
            amc.wait().await?
        }
    };
    std::process::exit(status.code().unwrap_or(1))
}

/// Guest image, pinned by digest (nixos/nix 2.35.4, multi-arch index).
const IMAGE: &str =
    "docker.io/nixos/nix@sha256:f159a58f390677f41e8e900a74b6e2f9c7f27b364122ae6acf7a652ce6c551c0";

/// The non-fork sandbox: pinned image, flakes on, secrets proxied to GitHub only.
// Network policy is left at microsandbox's default (Public: public egress only; private, LAN and host denied).
fn builder(pool: &Pool, spec: &Spec, name: &str) -> SandboxBuilder {
    let mut b = Sandbox::builder(name)
        .image_with(|i| i.oci(IMAGE).root_disk(16.gib()))
        .cpus(spec.cpus.unwrap_or(2))
        .memory(spec.memory_mib.unwrap_or(2048))
        // ponytail: cache.nixos.org only; add the fleet Attic read-only once a guest-safe token exists
        .env("NIX_CONFIG", "experimental-features = nix-command flakes");
    for key in pool.secret_files.keys() {
        // ponytail: every secret is released only to GitHub hosts; add per-secret hosts to pool.json for other providers
        b = b.secret(|s| {
            s.env(key)
                .source(SecretSource::env(format!("MUTINY_SECRET_{key}")))
                .allow("github.com")
                .allow("api.github.com")
                .substitute_in_header_fields(["Authorization"])
                .require_tls_identity(true)
        });
    }
    b
}

/// `host boot` (runs inside the AMC unit): boot the VM, set it up, hold it until it exits.
pub async fn boot(pool: &Pool, spec_json: &str) -> Result<()> {
    let spec: Spec = serde_json::from_str(spec_json)?;
    spec.validate()?;
    let name = spec.name.clone().context("spec has no name")?;
    let sandbox = if let Some(src) = &spec.fork_of {
        Sandbox::get(src)
            .await?
            .connect()
            .await?
            .fork(&name)
            .fork()
            .await?
    } else {
        builder(pool, &spec, &name).create().await?
    };
    if spec.fork_of.is_none() {
        let (repo, git_ref) = (
            spec.repo.as_deref().unwrap_or_default(),
            spec.git_ref.as_deref().unwrap_or_default(),
        );
        let auth = if pool.secret_files.contains_key("GITHUB_TOKEN") {
            // Git smart-HTTP on github.com takes only Basic auth (Bearer is REST-only and gets a
            // 401); the proxy substitutes inside the base64 Basic credentials. Global, so later
            // fetches and pushes in the sandbox authenticate too.
            r#"git config --global credential.helper '!f() { echo username=x-access-token; echo "password=$GITHUB_TOKEN"; }; f'; "#
        } else {
            ""
        };
        let script = format!(
            "set -e; command -v git >/dev/null || {{ echo 'guest image has no git' >&2; exit 127; }}; {auth}git clone -q https://github.com/{repo}.git /work/repo; cd /work/repo; git checkout -q {git_ref}"
        );
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
    check_name(name)?;
    let out = connect(name)
        .await?
        .exec_with("sh", |e| e.args(["-c", command]).cwd("/work/repo"))
        .await?;
    println!(
        "{}",
        json!({
            "exitCode": out.status().code,
            "stdout": String::from_utf8_lossy(out.stdout_bytes()),
            "stderr": String::from_utf8_lossy(out.stderr_bytes()),
        })
    );
    Ok(())
}

pub async fn read(name: &str, path: &str) -> Result<()> {
    check_name(name)?;
    use std::io::Write;
    std::io::stdout().write_all(&connect(name).await?.fs().read(path).await?)?;
    Ok(())
}

pub async fn write(name: &str, path: &str) -> Result<()> {
    check_name(name)?;
    let mut data = Vec::new();
    std::io::Read::read_to_end(&mut std::io::stdin(), &mut data)?;
    connect(name).await?.fs().write(path, data).await?;
    Ok(())
}

pub async fn snapshot(name: &str) -> Result<()> {
    check_name(name)?;
    let snap = format!("{name}-{}", suffix());
    Sandbox::get(name).await?.snapshot(&snap).await?;
    println!("{}", json!({ "snapshot": snap }));
    Ok(())
}

pub async fn destroy(name: &str) -> Result<()> {
    check_name(name)?;
    Sandbox::get(name).await?.destroy().await?;
    Ok(())
}

pub async fn list() -> Result<()> {
    let host = hostname();
    let (mut items, mut cursor) = (Vec::new(), None::<String>);
    loop {
        let page = Sandbox::list_with(|l| {
            let l = l.limit(MAX_SANDBOX_LIST_LIMIT);
            match &cursor {
                Some(c) => l.cursor(c.clone()),
                None => l,
            }
        })
        .await?;
        items.extend(page.sandboxes.iter().filter(|s| s.name().starts_with(PREFIX)).map(|s| {
            json!({ "id": format!("{host}:{}", s.name()), "status": format!("{:?}", s.status_snapshot()) })
        }));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    println!("{}", serde_json::to_string(&items)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(repo: &str, git_ref: &str) -> Spec {
        Spec {
            name: None,
            repo: Some(repo.into()),
            git_ref: Some(git_ref.into()),
            cpus: None,
            memory_mib: None,
            fork_of: None,
        }
    }

    #[test]
    fn validation() {
        assert!(spec("caniko/mutiny", "trunk").validate().is_ok());
        for bad in ["../x", "a/b/c", "-x", "a/..", "./b", "a/", "/b", "nobar"] {
            assert!(spec(bad, "trunk").validate().is_err(), "repo {bad}");
        }
        let named = |name: &str, fork_of: Option<&str>| Spec {
            name: Some(name.into()),
            fork_of: fork_of.map(Into::into),
            ..spec("caniko/mutiny", "trunk")
        };
        assert!(named("mu-ok", Some("mu-src")).validate().is_ok());
        assert!(named("other", None).validate().is_err(), "name needs mu-");
        assert!(
            named("mu-ok", Some("src")).validate().is_err(),
            "fork_of needs mu-"
        );
        assert!(check_name("mu-a;rm").is_err());
    }

    #[tokio::test]
    #[ignore = "needs /dev/kvm and MSB_PATH/MSB_LIBKRUNFW_PATH"]
    async fn boot_and_tools() {
        let pool = Pool {
            user: String::new(),
            local_host: String::new(),
            contract: String::new(),
            hosts: Vec::new(),
            secret_files: Default::default(),
            path: PathBuf::new(),
        };
        let name = format!("{PREFIX}test-{}", suffix());
        let sb = builder(&pool, &spec("caniko/mutiny", "trunk"), &name)
            .create()
            .await
            .unwrap();
        let out = sb.shell("git --version && nix --version").await;
        let _ = sb.destroy().await;
        let out = out.unwrap();
        assert!(out.status().success, "{}", out.stderr().unwrap_or_default());
    }
}
