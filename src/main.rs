mod config;
mod models;
mod strategy;
mod watcher;
mod pair;
mod collection;
mod network;
mod mcp_setup;

use clap::{Parser, Subcommand};
use config::{Config, DEFAULT_BASE_URL};
use network::Sender;
use collection::CollectionManager;
use pair::{PairManager, PairManagerAdapter};
use watcher::{FileWatcher, ClaudeLogWatcher, CopilotLogWatcher, OffsetStore};
use strategy::PathStrategy;
use strategy::log::{ClaudeStrategy, CopilotStrategy};
use std::sync::Arc;
use std::path::PathBuf;
use tracing_subscriber;
use walkdir::WalkDir;

#[derive(Parser)]
#[command(name = "aiprism-local")]
#[command(about = "Local AI Prism - Capture Claude/Copilot conversations")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Configuration file path (default: ~/.aiprism/config.json)
    #[arg(long)]
    config: Option<String>,

    /// Additional watch path (can be used multiple times)
    #[arg(long)]
    watch: Vec<String>,

    /// Quiet period in seconds (default: 30)
    #[arg(long)]
    quiet_period: Option<u64>,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize configuration
    Init {
        /// API token
        #[arg(long)]
        token: String,

        /// Base URL (default: https://aiprism.dsj.co.kr)
        #[arg(long)]
        base_url: Option<String>,
    },
    /// Add a project root path and write MCP config files
    Add {
        /// Project root path to add
        path: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Initialize logging
    let level = if cli.verbose {
        tracing_subscriber::filter::LevelFilter::DEBUG
    } else {
        tracing_subscriber::filter::LevelFilter::INFO
    };

    tracing_subscriber::fmt()
        .with_max_level(level)
        .with_target(false)
        .compact()
        .init();

    // Handle add command
    if let Some(Command::Add { path }) = &cli.command {
        let mut config = Config::load()?;

        let project_root = match std::path::PathBuf::from(path).canonicalize() {
            Ok(p) => p,
            Err(_) => {
                eprintln!("Error: path '{}' does not exist or is not accessible", path);
                std::process::exit(1);
            }
        };

        config.add_source_root(project_root.clone())?;
        println!("Added '{}' to source_roots", project_root.display());

        mcp_setup::write_claude_mcp(&project_root, &config.base_url, &config.api_token)?;
        println!("Written: {}/.mcp.json", project_root.display());

        mcp_setup::write_copilot_mcp(&project_root, &config.base_url, &config.api_token)?;
        println!("Written: {}/.vscode/mcp.json", project_root.display());

        println!("Note: restart the aiprism daemon to start watching the new path");
        return Ok(());
    }

    // Handle init command
    if let Some(Command::Init { token, base_url }) = cli.command {
        let config = Config {
            api_token: token,
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            source_roots: vec![],
            quiet_period_secs: 30,
        };

        config.validate()?;
        config.save()?;
        println!("Configuration saved to ~/.aiprism/config.json");
        return Ok(());
    }

    tracing::info!("Starting AI PRISM Local");

    // 1. Select PathStrategy by OS at compile time
    #[cfg(target_os = "macos")]
    let path_strategy: Arc<dyn PathStrategy> = Arc::new(strategy::path::MacOSPathStrategy);
    #[cfg(target_os = "linux")]
    let path_strategy: Arc<dyn PathStrategy> = Arc::new(strategy::path::LinuxPathStrategy);
    #[cfg(target_os = "windows")]
    let path_strategy: Arc<dyn PathStrategy> = Arc::new(strategy::path::WindowsPathStrategy);

    // 2. Load config
    let mut config = Config::load()?;

    // Override with CLI args
    if let Some(quiet) = cli.quiet_period {
        config.quiet_period_secs = quiet;
    }
    if !cli.watch.is_empty() {
        config.source_roots.extend(
            cli.watch
                .iter()
                .map(|p| PathBuf::from(p)),
        );
    }

    tracing::info!(
        "Config loaded: base_url={}, source_roots={:?}, quiet_period={}s",
        config.base_url,
        config.source_roots,
        config.quiet_period_secs
    );

    // 3. Create components using PathStrategy for paths
    let registry_path = path_strategy.registry_store_path();
    let offset_path = path_strategy.offset_store_path();

    let sender = Arc::new(Sender::new(config.base_url.clone(), config.api_token.clone()));
    let collection_manager = Arc::new(
        CollectionManager::new(registry_path, sender).await?,
    );
    let pair_manager = Arc::new(PairManager::new(collection_manager, config.quiet_period_secs, config.source_roots.clone()));

    // 4. Create file change handler adapter
    let (handler, handler_task) = pair::FileChangeHandlerAdapter::new(pair_manager.clone());
    tokio::spawn(handler_task);

    // 5. Start file watcher
    let fw = FileWatcher::new(config.source_roots.clone(), Arc::new(handler));
    tokio::spawn(async move {
        if let Err(e) = fw.run().await {
            tracing::error!("File watcher error: {}", e);
        }
    });

    tracing::info!("File watcher started for {:?}", config.source_roots);

    // 6. Start log watchers via PathStrategy
    start_all_watchers(pair_manager, path_strategy, offset_path).await?;

    // Keep alive
    tokio::signal::ctrl_c().await?;
    tracing::info!("Shutting down");

    Ok(())
}

async fn start_all_watchers(
    pair_manager: Arc<PairManager>,
    path_strategy: Arc<dyn PathStrategy>,
    offset_store_path: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    // Load persisted offsets
    let offsets = OffsetStore::load_from_file(&offset_store_path)?;
    tracing::info!("Offset store loaded from {:?}", offset_store_path);

    let log_dirs = path_strategy.log_directories();
    tracing::info!("Log directories to watch: {:?}", log_dirs.iter().map(|(n, _)| n).collect::<Vec<_>>());

    for (agent_name, dir_path) in log_dirs {
        if !dir_path.exists() {
            continue;
        }

        // Walk directory recursively for .jsonl files
        let jsonl_files: Vec<PathBuf> = WalkDir::new(&dir_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter(|e| e.path().extension().map(|x| x == "jsonl").unwrap_or(false))
            .map(|e| e.path().to_path_buf())
            .collect();

        tracing::info!("Found {} .jsonl files for agent '{}'", jsonl_files.len(), agent_name);

        for file_path in jsonl_files {
            // Create per-file adapter
            let (adapter, adapter_task) = PairManagerAdapter::new(pair_manager.clone());
            tokio::spawn(adapter_task);

            match agent_name.as_str() {
                "claudecode" => {
                    let strategy = Arc::new(ClaudeStrategy);
                    let mut watcher = ClaudeLogWatcher::new(
                        file_path.clone(),
                        strategy,
                        Arc::new(adapter),
                        offsets.clone(),
                    );

                    let offset_path_clone = offset_store_path.clone();
                    tokio::spawn(async move {
                        if let Err(e) = watcher.run(&offset_path_clone).await {
                            tracing::error!("Claude log watcher error for {:?}: {}", file_path, e);
                        }
                    });
                }
                "GitHub Copilot" => {
                    let strategy = Arc::new(CopilotStrategy);
                    let mut watcher = CopilotLogWatcher::new(
                        file_path.clone(),
                        strategy,
                        Arc::new(adapter),
                        offsets.clone(),
                    );

                    let offset_path_clone = offset_store_path.clone();
                    tokio::spawn(async move {
                        if let Err(e) = watcher.run(&offset_path_clone).await {
                            tracing::error!("Copilot log watcher error for {:?}: {}", file_path, e);
                        }
                    });
                }
                other => {
                    tracing::warn!("Unknown agent: {}, skipping", other);
                }
            }
        }
    }

    Ok(())
}
