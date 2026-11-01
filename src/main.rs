mod mcp;
mod pool;
mod sandbox;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "Self-hosted microVM sandboxes for coding agents")]
struct Cli {
    /// pool.json (default: $XDG_CONFIG_HOME/mutiny/pool.json)
    #[arg(long, global = true)]
    pool: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Operate on this host
    #[command(subcommand)]
    Host(Host),
    /// Stdio MCP server
    Mcp,
}

#[derive(Subcommand)]
enum Host {
    /// This host's capacity as JSON
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Start a sandbox here and hold its AMC lease; prints {"id"}
    Run {
        #[arg(long)]
        spec: String,
    },
    #[command(hide = true)]
    Admit {
        #[arg(long)]
        spec: String,
    },
    #[command(hide = true)]
    Boot {
        #[arg(long)]
        spec: String,
    },
    Exec { name: String, command: String },
    Read { name: String, path: String },
    /// Content comes from stdin
    Write { name: String, path: String },
    Fork { name: String },
    Snapshot { name: String },
    Destroy { name: String },
    List,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let pool = pool::Pool::load(cli.pool)?;
    match cli.cmd {
        Cmd::Mcp => mcp::serve(pool).await,
        Cmd::Host(h) => match h {
            Host::Status { json } => {
                let s = pool::local_status();
                if json {
                    println!("{}", serde_json::to_string(&s)?);
                } else {
                    println!("{}: {}", s.host, s.reason.as_deref().unwrap_or("eligible"));
                }
                Ok(())
            }
            Host::Run { spec } => sandbox::run(&pool, &spec).await,
            Host::Admit { spec } => sandbox::admit(&pool, &spec).await,
            Host::Boot { spec } => sandbox::boot(&pool, &spec).await,
            Host::Exec { name, command } => sandbox::exec(&name, &command).await,
            Host::Read { name, path } => sandbox::read(&name, &path).await,
            Host::Write { name, path } => sandbox::write(&name, &path).await,
            Host::Fork { name } => sandbox::run(&pool, &serde_json::json!({ "forkOf": name }).to_string()).await,
            Host::Snapshot { name } => sandbox::snapshot(&name).await,
            Host::Destroy { name } => sandbox::destroy(&name).await,
            Host::List => sandbox::list().await,
        },
    }
}
