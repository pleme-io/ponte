//! ponte CLI — runs the three-Job pipeline (context-assemble ->
//! invoke-openwiki -> route) against one target repo, prints a
//! receipt-shaped summary, and exits non-zero on any deadlettered Job.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use clap::Parser;
use shigoto_dag::Dag;
use shigoto_emit::AuditFileEmitter;
use shigoto_retry::RetryPolicy;
use shigoto_scheduler::{InProcessScheduler, Scheduler};
use shigoto_types::{ErasedJob, Job, JobKindId, JobPhase, RecordingJob};
use tokio::sync::Mutex as AsyncMutex;
use zoekt_mcp::client::ZoektClient;

use ponte::context::ContextAssembleJob;
use ponte::openwiki::InvokeOpenWikiJob;
use ponte::route::RouteJob;
use ponte::state::repo_slug;

/// Bridges LangChain OpenWiki into the pleme-io fleet.
#[derive(Parser)]
#[command(name = "ponte")]
struct Cli {
    /// Path to the target repo. Init vs update is auto-detected by
    /// whether `openwiki/` already exists there.
    #[arg(long)]
    repo: PathBuf,

    /// Path to the packaged `openwiki` binary.
    #[arg(long, env = "PONTE_OPENWIKI_BIN", default_value = "openwiki")]
    openwiki_bin: PathBuf,

    /// Zoekt base URL (defaults to `ZOEKT_URL`, then `http://localhost:6070`).
    #[arg(long, env = "ZOEKT_URL")]
    zoekt_url: Option<String>,

    /// Outer timeout for the whole openwiki subprocess invocation.
    #[arg(long, default_value = "600")]
    openwiki_timeout_secs: u64,

    /// Max scheduler ticks before giving up (each tick is spaced by a
    /// short sleep so a retry's backoff window has a chance to elapse).
    #[arg(long, default_value = "30")]
    max_ticks: u32,
}

fn ponte_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".ponte")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let repo_path = cli
        .repo
        .canonicalize()
        .context("target --repo does not exist")?;
    let repo_name = repo_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".into());

    let state_dir = ponte_home().join("state");
    let audit_dir = ponte_home().join("audit");
    std::fs::create_dir_all(&audit_dir)?;
    let audit_path = audit_dir.join(format!("{}.jsonl", repo_slug(&repo_path)));
    let emitter: Arc<dyn shigoto_emit::TransitionEmitter> =
        Arc::new(AuditFileEmitter::new(&audit_path)?);

    let zoekt = match &cli.zoekt_url {
        Some(url) => ZoektClient::new(url.clone()),
        None => ZoektClient::from_env(),
    };

    // Shared cells carrying each Job's typed Output to the next one —
    // shigoto v0.1 enforces DAG *ordering* (AllUpstreamsTerminal) but
    // doesn't wire inter-Job data passing yet, so this is the minimal
    // correct bridge. See ContextAssembleJob::outcome_cell.
    let context_cell = Arc::new(AsyncMutex::new(None));
    let openwiki_cell = Arc::new(AsyncMutex::new(None));

    let context_job = Arc::new(ContextAssembleJob {
        repo_path: repo_path.clone(),
        repo_name: repo_name.clone(),
        zoekt,
        state_dir,
        outcome_cell: context_cell.clone(),
        output_sink: None,
    });
    let openwiki_job = Arc::new(InvokeOpenWikiJob {
        repo_path: repo_path.clone(),
        repo_name: repo_name.clone(),
        openwiki_binary: cli.openwiki_bin.clone(),
        timeout: Duration::from_secs(cli.openwiki_timeout_secs),
        upstream_context: context_cell.clone(),
        own_outcome: openwiki_cell.clone(),
        output_sink: None,
    });
    let route_job = Arc::new(RouteJob {
        repo_path: repo_path.clone(),
        repo_name: repo_name.clone(),
        upstream_outcome: openwiki_cell.clone(),
        output_sink: None,
    });

    let context_id = <ContextAssembleJob as Job>::id(&context_job);
    let openwiki_id = <InvokeOpenWikiJob as Job>::id(&openwiki_job);
    let route_id = <RouteJob as Job>::id(&route_job);

    let mut dag = Dag::new();
    dag.add_edge(context_id.clone(), openwiki_id.clone());
    dag.add_edge(openwiki_id.clone(), route_id.clone());

    let scheduler = InProcessScheduler::new("ponte").with_emitter(emitter);
    scheduler
        .register_job(context_job.clone() as Arc<dyn ErasedJob>)
        .await;
    scheduler
        .register_job(openwiki_job.clone() as Arc<dyn ErasedJob>)
        .await;
    scheduler
        .register_job(route_job.clone() as Arc<dyn ErasedJob>)
        .await;

    // openwiki is a paid, non-deterministic LLM call — one retry on
    // transient failure (network blip), not unbounded. shigoto v0.1's
    // scheduler doesn't classify Declarative-vs-Transient failures yet
    // (every FailureRecord defaults to Transient), so a bad API key
    // costs one wasted retry rather than looping forever — an accepted
    // tradeoff until that classification lands upstream.
    scheduler
        .register_retry_policy(
            JobKindId::new(<InvokeOpenWikiJob as RecordingJob>::KIND),
            RetryPolicy::Fixed {
                attempts: 2,
                delay_ms: 5_000,
            },
        )
        .await;

    for i in 0..cli.max_ticks {
        scheduler.tick(&mut dag).await?;
        let snapshot = scheduler.snapshot(&dag).await;
        let done = snapshot.phases.values().all(|p| {
            matches!(
                p,
                JobPhase::Succeeded | JobPhase::Skipped(_) | JobPhase::Deadlettered
            )
        });
        if done {
            tracing::info!(ticks = i + 1, "pipeline reached a terminal state");
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    let snapshot = scheduler.snapshot(&dag).await;
    let mut deadlettered = 0u32;
    for id in [&context_id, &openwiki_id, &route_id] {
        let phase = snapshot
            .phases
            .get(id)
            .cloned()
            .unwrap_or(JobPhase::Pending);
        println!("{:<24} {:?}", id.kind.0, phase);
        if matches!(phase, JobPhase::Deadlettered) {
            deadlettered += 1;
        }
    }
    println!(
        "ponte: {repo_name} — {} deadlettered. Audit log: {}",
        deadlettered,
        audit_path.display()
    );

    if deadlettered > 0 {
        anyhow::bail!("{deadlettered} job(s) deadlettered");
    }
    Ok(())
}
