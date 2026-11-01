//! pool.json, per-host capacity readings and placement.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf, process::Stdio, time::Duration};
use tokio::{io::AsyncWriteExt, process::Command, task::JoinSet};

/// A host admits a sandbox only with this much free AMC budget and MemAvailable.
const MIN_FREE_MIB: u64 = 2048;
/// ...and at most this memory "full" PSI avg10 (percent).
const MAX_PSI: f64 = 10.0;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pool {
    pub user: String,
    pub local_host: String,
    pub contract: String,
    pub hosts: Vec<PoolHost>,
    pub sccache_dir: PathBuf,
    pub secret_files: BTreeMap<String, PathBuf>,
    #[serde(skip)]
    pub path: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolHost {
    pub name: String,
    pub address: String,
    pub ssh_port: u16,
    #[serde(default)]
    pub priority: u8,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStatus {
    pub host: String,
    pub eligible: bool,
    #[serde(rename = "freeBudgetMiB")]
    pub free_budget_mib: Option<u64>,
    #[serde(rename = "memAvailableMiB")]
    pub mem_available_mib: Option<u64>,
    pub mem_full_psi: Option<f64>,
    pub on_ac: Option<bool>,
    /// From pool.json, not reported by the host.
    #[serde(skip)]
    pub priority: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl HostStatus {
    fn down(host: &str, why: impl ToString) -> Self {
        Self {
            host: host.into(),
            eligible: false,
            free_budget_mib: None,
            mem_available_mib: None,
            mem_full_psi: None,
            on_ac: None,
            priority: 0,
            reason: Some(why.to_string()),
        }
    }
}

#[derive(Debug)]
pub struct NoHostAdmits;
impl std::fmt::Display for NoHostAdmits {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("no pool host is eligible to admit a sandbox")
    }
}
impl std::error::Error for NoHostAdmits {}

/// Highest (priority, free budget) among eligible hosts; ties go to `local`.
// ponytail: greedy placement, no reservation between status and run; AMC queue is the backstop; add a two-phase claim if races appear
pub fn place<'a>(hosts: &'a [HostStatus], local: &str) -> Result<&'a str, NoHostAdmits> {
    hosts
        .iter()
        .filter(|h| h.eligible)
        .max_by_key(|h| (h.priority, h.free_budget_mib.unwrap_or(0), h.host == local))
        .map(|h| h.host.as_str())
        .ok_or(NoHostAdmits)
}

impl Pool {
    pub fn load(path: Option<PathBuf>) -> Result<Self> {
        let path = match path {
            Some(p) => p,
            None => match std::env::var_os("XDG_CONFIG_HOME") {
                Some(x) => PathBuf::from(x),
                None => PathBuf::from(std::env::var_os("HOME").context("HOME unset")?).join(".config"),
            }
            .join("mutiny/pool.json"),
        };
        let text = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let mut pool: Pool = serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        pool.path = path;
        Ok(pool)
    }

    /// Run `mutiny <args>` on `host` (this binary locally, ssh elsewhere) and return stdout.
    pub async fn run_on(&self, host: &str, args: &[&str], stdin: Option<&str>) -> Result<String> {
        let mut cmd = if host == self.local_host {
            let mut c = Command::new(std::env::current_exe()?);
            c.arg("--pool").arg(&self.path).args(args);
            c
        } else {
            let h = self.hosts.iter().find(|h| h.name == host).with_context(|| format!("unknown host {host}"))?;
            let mut c = Command::new("ssh");
            c.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5", "-p"])
                .arg(h.ssh_port.to_string())
                .arg(format!("{}@{}", self.user, h.address))
                .arg(format!("mutiny {}", args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")));
            c
        };
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        let mut child = cmd.spawn()?;
        let mut pipe = child.stdin.take().context("stdin")?;
        if let Some(s) = stdin {
            pipe.write_all(s.as_bytes()).await?;
        }
        drop(pipe);
        let out = child.wait_with_output().await?;
        ensure!(out.status.success(), "{host}: {}", String::from_utf8_lossy(&out.stderr).trim());
        Ok(String::from_utf8(out.stdout)?)
    }

    /// Query every host in parallel; unreachable or slow hosts come back ineligible.
    pub async fn gather(self: &std::sync::Arc<Self>) -> Vec<HostStatus> {
        let mut set = JoinSet::new();
        for h in &self.hosts {
            let (pool, name, priority) = (self.clone(), h.name.clone(), h.priority);
            set.spawn(async move {
                let got = async {
                    let out = pool.run_on(&name, &["host", "status", "--json"], None).await?;
                    Ok::<_, anyhow::Error>(serde_json::from_str::<HostStatus>(&out)?)
                };
                match got.await {
                    Ok(mut s) => {
                        s.host = name;
                        s.priority = priority;
                        s
                    }
                    Err(e) => HostStatus::down(&name, e),
                }
            });
        }
        let mut out = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(12), async {
            while let Some(Ok(s)) = set.join_next().await {
                out.push(s);
            }
        })
        .await;
        for h in &self.hosts {
            if !out.iter().any(|s| s.host == h.name) {
                out.push(HostStatus::down(&h.name, "unreachable"));
            }
        }
        out
    }
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// This host's capacity. Any missing or duplicate reading makes it ineligible (fail closed).
pub fn local_status() -> HostStatus {
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default().trim().to_string();
    let mut why = Vec::new();
    let free = ok(amc_free_mib(), &mut why);
    let mem = ok(meminfo_mib(), &mut why);
    let psi = ok(memory_full_psi(), &mut why);
    let ac = ok(on_ac(), &mut why);
    let mut s = HostStatus { host, eligible: false, free_budget_mib: free, mem_available_mib: mem, mem_full_psi: psi, on_ac: ac, priority: 0, reason: None };
    if free.is_some_and(|f| f < MIN_FREE_MIB) {
        why.push(format!("free AMC budget below {MIN_FREE_MIB} MiB"));
    }
    if mem.is_some_and(|m| m < MIN_FREE_MIB) {
        why.push(format!("MemAvailable below {MIN_FREE_MIB} MiB"));
    }
    if psi.is_some_and(|p| !(0.0..=MAX_PSI).contains(&p)) {
        why.push(format!("memory full PSI above {MAX_PSI}"));
    }
    if ac == Some(false) {
        why.push("not on AC power".into());
    }
    s.eligible = why.is_empty();
    s.reason = (!why.is_empty()).then(|| why.join("; "));
    s
}

fn ok<T>(r: Result<T>, why: &mut Vec<String>) -> Option<T> {
    r.map_err(|e| why.push(e.to_string())).ok()
}

#[derive(Deserialize)]
struct AmcStatus {
    budget_bytes: u64,
    committed_bytes: u64,
}

fn amc_free_mib() -> Result<u64> {
    let out = std::process::Command::new("amc").args(["admission", "status", "--json"]).output().context("run amc")?;
    ensure!(out.status.success(), "amc admission status failed");
    parse_amc(&out.stdout)
}

fn parse_amc(json: &[u8]) -> Result<u64> {
    let s: AmcStatus = serde_json::from_slice(json).context("parse amc status")?;
    Ok(s.budget_bytes.saturating_sub(s.committed_bytes) >> 20)
}

/// The single line starting with `prefix`; zero or several lines is an error.
fn single<'a>(text: &'a str, prefix: &str) -> Result<&'a str> {
    let mut lines = text.lines().filter_map(|l| l.strip_prefix(prefix));
    match (lines.next(), lines.next()) {
        (Some(l), None) => Ok(l),
        _ => bail!("missing or duplicate {prefix:?} reading"),
    }
}

fn meminfo_mib() -> Result<u64> {
    let text = std::fs::read_to_string("/proc/meminfo")?;
    let mut f = single(&text, "MemAvailable:")?.split_whitespace();
    let kib: u64 = f.next().context("MemAvailable value")?.parse()?;
    ensure!(f.next() == Some("kB"), "MemAvailable unit");
    Ok(kib >> 10)
}

fn memory_full_psi() -> Result<f64> {
    let text = std::fs::read_to_string("/proc/pressure/memory")?;
    let line = single(&text, "full ")?;
    let v = line.split_whitespace().find_map(|w| w.strip_prefix("avg10=")).context("avg10")?;
    Ok(v.parse()?)
}

/// Any online Mains supply; no battery at all (desktop) counts as AC.
fn on_ac() -> Result<bool> {
    let (mut mains, mut battery) = (false, false);
    for e in std::fs::read_dir("/sys/class/power_supply")? {
        let p = e?.path();
        match std::fs::read_to_string(p.join("type"))?.trim() {
            "Mains" => mains |= std::fs::read_to_string(p.join("online"))?.trim() == "1",
            "Battery" => battery = true,
            _ => {}
        }
    }
    Ok(mains || !battery)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(host: &str, eligible: bool, free: u64) -> HostStatus {
        HostStatus { eligible, free_budget_mib: Some(free), ..HostStatus::down(host, "") }
    }

    #[test]
    fn placement() {
        let hosts = [h("a", false, 90_000), h("b", true, 1000), h("c", true, 5000)];
        assert_eq!(place(&hosts, "b").unwrap(), "c", "ineligible never chosen, most budget wins");
        let tie = [h("a", true, 5000), h("b", true, 5000)];
        assert_eq!(place(&tie, "b").unwrap(), "b", "tie prefers local");
        assert_eq!(place(&tie, "a").unwrap(), "a");
        assert!(place(&[h("a", false, 1)], "a").is_err());
        let p = |host, eligible, free, priority| HostStatus { priority, ..h(host, eligible, free) };
        let prio = [p("atlas", true, 9000, 0), p("camel", true, 1000, 1)];
        assert_eq!(place(&prio, "atlas").unwrap(), "camel", "priority beats budget");
        let down = [p("atlas", true, 9000, 0), p("camel", false, 99_000, 1)];
        assert_eq!(place(&down, "atlas").unwrap(), "atlas", "ineligible priority host never chosen");
    }

    #[test]
    fn amc_json() {
        let j = br#"{"budget_bytes":51539607552,"reserve_bytes":17179869184,"committed_bytes":1073741824,"entries":[],"waiting":{},"unreconciled":[]}"#;
        assert_eq!(parse_amc(j).unwrap(), 48 * 1024 - 1024);
    }
}
