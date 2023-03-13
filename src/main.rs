mod quay_configurator;
use chrono::Utc;
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{
    generate,
    shells::{Bash, Elvish, Fish, PowerShell, Zsh},
};
use core::{panic};
use env_logger::{fmt::Color, Env, Target};
use std::io;
use tokio::{time::Instant, signal::{self, unix::SignalKind}, sync::mpsc};
use tokio::signal::unix::{signal};
//use console_subscriber;
use crate::quay_configurator::quay_config_reader::QuayYamlConfig;
use env_logger;
use log::{error, info, Level};
use memory_stats::memory_stats;
use std::io::Write;

#[derive(Parser)]
#[command(author, version, about="Mass Quay Configurator", long_about = None)]
#[command(help_template(
    "\
{before-help}{about-with-newline}{name} {version} - {author-with-newline}
{usage-heading} {usage}

{all-args}{after-help}
"
))]

struct Cli {
    #[command(subcommand)]
    command: SubCommands,

    #[arg(short, long)]
    /// Global max requests per second. Default to 50
    max_requests_per_second: Option<u32>,

    #[arg(short, long)]
    /// Quay yaml directory. Default to 'yaml-files'
    dir: Option<String>,

    #[arg(short, long)]
    /// Log level. Accepted log level: info, debug. Default to info.
    log_level: Option<log::Level>,

    #[arg(short, long)]
    /// Log verbosity. Accepted values: 0,5,10. Default to 0, 5 is the only implemented value so far.
    verbosity: Option<u8>,

    #[arg(long)]
    /// Connection timeout in seconds. Default to 5
    timeout: Option<u64>,

    #[arg(long)]
    /// Verify Quay tls certificate. Default to true
    tls_verify: Option<bool>,

    #[arg(long, short)]
    /// Connection retries. Default to 3
    retries: Option<u32>,

    #[arg(long, short)]
    /// Jitter (ms). Default to 100 ms
    jitter: Option<u64>,
}

#[derive(Subcommand)]
enum SubCommands {
    /// Create all Quay organizations
    Create(Create),
    /// Delete all Quay organizations
    Delete(Delete),
    /// Check all Quay organizations yaml files
    Check(Check),
    /// Login to detected Quay organizations
    Login(Login),
    /// Generate completion script
    Completion(Completion),
}
#[derive(Args)]
struct Completion {
    shell: Shells,
}

#[derive(Subcommand, Clone, ValueEnum)]
enum Shells {
    Bash,
    Zsh,
    Elvish,
    Fish,
    PowerShell,
}

#[derive(Args)]
struct Login {}

#[derive(Args)]
struct Create {}

#[derive(Args)]
struct Delete {}

#[derive(Args)]
struct Check {}

fn log_start() {
    let time = Utc::now();
    info!("UTC start time: {:?}", time.to_rfc3339());
}

fn log_end(now: Instant) {
    info!("Execution terminated.");
    info!(
        "Total execution time in seconds: {}",
        now.elapsed().as_secs_f32()
    );
}

fn memory_usage() {
    if let Some(usage) = memory_stats() {
        info!(
            "Current physical memory usage: {} Mib",
            usage.physical_mem / (1024 * 2024)
        );
        info!(
            "Current virtual memory usage: {} Mib",
            usage.virtual_mem / (1024 * 2024)
        );
    } else {
        info!("Couldn't get the current memory usage :(");
    }
}

/// qr async main
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    //console_subscriber::init();

    let (shutdown_send, mut shutdown_recv) = mpsc::unbounded_channel();

     tokio::spawn(async move {
        let now = Instant::now();

        let cli = Cli::parse();

        let req_per_seconds: u32;

        
        match cli.max_requests_per_second {
            Some(d) => req_per_seconds = d,
            None => req_per_seconds = 50,
        }

        let dir: String;
        match cli.dir {
            Some(d) => dir = d,
            None => dir = "yaml-files".to_string(),
        }

        let jitter: u64;
        match cli.jitter {
            Some(d) => jitter = d,
            None => jitter = 100,
        }

        let retries: u32;
        match cli.retries {
            Some(d) => retries = d,
            None => retries = 3,
        }

        let log_level: log::Level;
        match cli.log_level {
            Some(ll) => log_level = ll,
            None => log_level = log::Level::Info,
        }

        let log_verbosity: u8;
        match cli.verbosity {
            Some(verbosity) => log_verbosity = verbosity,
            None => log_verbosity = 0,
        }

        let timeout: u64;
        match cli.timeout {
            Some(to) => timeout = to,
            None => timeout = 5,
        }

        let tls_verify: bool;
        match cli.tls_verify {
            Some(verify) => tls_verify = verify,
            None => tls_verify = true,
        }

        // Custom logger
        env_logger::Builder::from_env(Env::default().default_filter_or(log_level.as_str()))
            .target(Target::Stdout)
            .format(|buf, record| {
                let mut level_style = buf.style();

                match record.level() {
                    Level::Info => level_style.set_color(Color::Green).set_bold(true),
                    Level::Debug => level_style.set_color(Color::Blue).set_bold(true),
                    Level::Warn => level_style.set_color(Color::Yellow).set_bold(true),
                    Level::Error => level_style.set_color(Color::Red).set_bold(true),
                    Level::Trace => level_style.set_color(Color::Black).set_bold(true),
                };

                writeln!(
                    buf,
                    "[{} {}]: {}",
                    buf.timestamp(),
                    level_style.value(record.level()),
                    record.args()
                )
            })
            .init();

        // main config
        let mut config: QuayYamlConfig;

        match QuayYamlConfig::new(
            &dir,
            req_per_seconds,
            log_level,
            log_verbosity,
            timeout,
            false,
            tls_verify,
            retries,
            jitter,
        ) {
            Ok(c) => {
                config = c;

                match &cli.command {
                    SubCommands::Completion(_) => {}
                    _ => info!("Basic config successfully loaded"),
                }
            }
            Err(_e) => {
                match &cli.command {
                    SubCommands::Completion(_) => {}
                    _ => {
                        error!("Login config file not found or corrupted. Run mqc login.");
                    }
                }

                match QuayYamlConfig::new(
                    &dir,
                    req_per_seconds,
                    log_level,
                    log_verbosity,
                    timeout,
                    true,
                    tls_verify,
                    retries,
                    jitter,
                ) {
                    Ok(c) => {
                        config = c;
                        match &cli.command {
                            SubCommands::Completion(_) => {}
                            _ => {
                                info!("Dummy login config successfully loaded")
                            }
                        }
                    }
                    Err(e) => {
                        error!("Login config file not found or corrupted. Stopping execution.");
                        panic!("{}", e.to_string());
                    }
                }
            }
        }

        match &cli.command {
            SubCommands::Completion(completion) => {
                // let c= completion.clone();

                match completion.shell {
                    Shells::Bash => generate(Bash, &mut Cli::command(), "mqc", &mut io::stdout()),
                    Shells::Zsh => generate(Zsh, &mut Cli::command(), "mqc", &mut io::stdout()),
                    Shells::Elvish => {
                        generate(Elvish, &mut Cli::command(), "mqc", &mut io::stdout())
                    }
                    Shells::Fish => generate(Fish, &mut Cli::command(), "mqc", &mut io::stdout()),
                    Shells::PowerShell => {
                        generate(PowerShell, &mut Cli::command(), "mqc", &mut io::stdout())
                    }
                }
                shutdown_send.send("exit".to_string()).unwrap();
            }

            SubCommands::Create(_) => {
                log_start();
                info!(
                    "Checking quay configurations file from {} directory...",
                    &dir
                );
                memory_usage();
                config.check_config(true).await.unwrap();

                info!(
                    "Loading quay configurations file from {} directory...",
                    &dir
                );

                config.load_config().await.unwrap();

                info!("Creating quay configurations...");

                config.create_all().await.unwrap();
                memory_usage();
                log_end(now);
                shutdown_send.send("exit".to_string()).unwrap();
            }
            SubCommands::Delete(_) => {
                log_start();
                info!(
                    "Checking quay configurations file from {} directory...",
                    &dir
                );
                memory_usage();

                config.check_config(false).await.unwrap();

                info!(
                    "Loading quay configurations file from {} directory...",
                    &dir
                );

                config.load_config().await.unwrap();

                info!("Creating quay configurations...");

                config.delete_all().await.unwrap();
                memory_usage();
                log_end(now);
                shutdown_send.send("exit".to_string()).unwrap();
            }
            SubCommands::Check(_) => {
                log_start();
                info!(
                    "Checking quay configurations file from {} directory...",
                    &dir
                );
                memory_usage();
                config.check_config(true).await.unwrap();

                info!(
                    "Loading quay configurations file from {} directory...",
                    &dir
                );
                config.load_config().await.unwrap();
                memory_usage();
                log_end(now);
                shutdown_send.send("exit".to_string()).unwrap();
            }
            SubCommands::Login(_) => {
                log_start();
                info!("Creating Quay login info from {} directory...", &dir);
                memory_usage();
                config.check_config(false).await.unwrap();
                config.load_config().await.unwrap();
                config.create_login().await.unwrap();
                memory_usage();
                log_end(now);
                shutdown_send.send("exit".to_string()).unwrap();
            }
        }
    });
    
    
    let mut stream = signal(SignalKind::hangup())?;

    tokio::select! {

        _ = signal::ctrl_c() => {

            println!("ctrc +c detected.");
        },
        received_signal = stream.recv() => {
            
            println!("SIGNAL RECEIVED {:#?}",received_signal.unwrap())
        },
        _ = shutdown_recv.recv() => {},

    }

//    async_task.await.expect("async task failed");
// https://stackoverflow.com/questions/71337098/programmatically-create-and-listen-for-multiple-signals-and-a-ticking-interval-w

    Ok(())
}
