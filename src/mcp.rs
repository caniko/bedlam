//! Stdio MCP server: places sandboxes across the pool and routes calls by id host prefix.
use crate::pool::{Pool, place};
use anyhow::{Context, Result};
use rmcp::{
    ServiceExt, handler::server::wrapper::Parameters, schemars, tool, tool_router, transport::stdio,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Clone)]
struct Bedlam(Arc<Pool>);

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Create {
    /// GitHub repo as owner/name
    repo: String,
    /// Branch, tag or commit
    r#ref: String,
    cpus: Option<u8>,
    memory_mib: Option<u32>,
}
#[derive(Deserialize, schemars::JsonSchema)]
struct Id {
    /// Sandbox id, host:name
    id: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
struct ExecArgs {
    id: String,
    command: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
struct ReadArgs {
    id: String,
    path: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
struct WriteArgs {
    id: String,
    path: String,
    content: String,
}

impl Bedlam {
    /// Run `bedlam host <args>` on the host named by the id prefix.
    async fn on(
        &self,
        id: &str,
        verb: &str,
        rest: &[&str],
        stdin: Option<&str>,
    ) -> Result<String, String> {
        let go = async {
            let (host, name) = id.split_once(':').context("id must be host:name")?;
            let mut args = vec!["host", verb, name];
            args.extend(rest);
            self.0.run_on(host, &args, stdin).await
        };
        go.await.map_err(|e| format!("{e:#}"))
    }
}

#[tool_router(server_handler)]
impl Bedlam {
    #[tool(description = "Per-host sandbox capacity of the pool")]
    async fn pool_status(&self) -> String {
        serde_json::to_string(&self.0.gather().await).unwrap_or_default()
    }

    #[tool(
        description = "Start a microVM sandbox with the repo cloned at ref on the pool host with most free capacity; returns {id}"
    )]
    async fn sandbox_create(&self, Parameters(a): Parameters<Create>) -> Result<String, String> {
        let hosts = self.0.gather().await;
        let host = place(&hosts, &self.0.local_host).map_err(|e| e.to_string())?;
        let spec =
            json!({ "repo": a.repo, "ref": a.r#ref, "cpus": a.cpus, "memoryMiB": a.memory_mib })
                .to_string();
        self.0
            .run_on(host, &["host", "run", "--spec", &spec], None)
            .await
            .map_err(|e| format!("{e:#}"))
    }

    #[tool(
        description = "Run a shell command in the sandbox (cwd /work/repo); returns {exitCode, stdout, stderr}"
    )]
    async fn exec(&self, Parameters(a): Parameters<ExecArgs>) -> Result<String, String> {
        self.on(&a.id, "exec", &[&a.command], None).await
    }

    #[tool(description = "Read a file from the sandbox")]
    async fn read_file(&self, Parameters(a): Parameters<ReadArgs>) -> Result<String, String> {
        self.on(&a.id, "read", &[&a.path], None).await
    }

    #[tool(description = "Write a file in the sandbox")]
    async fn write_file(&self, Parameters(a): Parameters<WriteArgs>) -> Result<String, String> {
        self.on(&a.id, "write", &[&a.path], Some(&a.content))
            .await
            .map(|_| "ok".into())
    }

    #[tool(
        description = "Fork a running sandbox (disk and memory, copy-on-write) into a new sandbox; returns {id}"
    )]
    async fn fork(&self, Parameters(a): Parameters<Id>) -> Result<String, String> {
        self.on(&a.id, "fork", &[], None).await
    }

    #[tool(description = "Snapshot the sandbox disk; returns {snapshot}")]
    async fn snapshot(&self, Parameters(a): Parameters<Id>) -> Result<String, String> {
        self.on(&a.id, "snapshot", &[], None).await
    }

    #[tool(description = "Destroy the sandbox and release its AMC lease")]
    async fn destroy(&self, Parameters(a): Parameters<Id>) -> Result<String, String> {
        self.on(&a.id, "destroy", &[], None)
            .await
            .map(|_| "ok".into())
    }

    #[tool(description = "List sandboxes on every pool host")]
    async fn list(&self) -> String {
        let mut all = Vec::new();
        for h in &self.0.hosts {
            match self.0.run_on(&h.name, &["host", "list"], None).await {
                Ok(s) => all.push(json!({ "host": h.name, "sandboxes": serde_json::from_str::<serde_json::Value>(&s).unwrap_or_default() })),
                Err(e) => all.push(json!({ "host": h.name, "error": format!("{e:#}") })),
            }
        }
        json!(all).to_string()
    }
}

pub async fn serve(pool: Pool) -> Result<()> {
    Bedlam(Arc::new(pool))
        .serve(stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}
