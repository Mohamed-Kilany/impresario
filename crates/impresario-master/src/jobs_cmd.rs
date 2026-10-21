//! Scriptable job commands and the supervisor entry point (TASK-2 3.5).
//!
//! These commands are deliberately plain: they work with no TTY, never prompt
//! and use the same stable exit codes as `run`. `attach` is the human view;
//! `jobs --json`, `wait` and `report` are for scripts.

use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::Path;
use std::time::{Duration, Instant};

use clap::Args;
use impresario_core::home::Home;

use crate::cli::{EXIT_FAILURE, EXIT_INFRA, EXIT_INTERRUPTED, EXIT_OK, EXIT_USAGE};
use crate::jobs::{self, JobRecord, JobStatus, JobStore};
use crate::reporters;

const POLL: Duration = Duration::from_millis(250);

/// `impresario jobs`.
#[derive(Debug, Args)]
pub struct JobsArgs {
    /// Include finished jobs (default: active only).
    #[arg(long)]
    pub all: bool,
    /// Machine-readable output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct AttachArgs {
    /// Job id (as printed by `run --detach` or `jobs`).
    pub id: String,
    /// On Ctrl-C stop the job instead of detaching from it.
    #[arg(long)]
    pub stop: bool,
}

#[derive(Debug, Args)]
pub struct LogsArgs {
    /// Job id.
    pub id: String,
    /// Follow while the job runs.
    #[arg(short, long)]
    pub follow: bool,
    /// Print `events.jsonl` instead of the human console log.
    #[arg(long)]
    pub events: bool,
}

#[derive(Debug, Args)]
pub struct StopArgs {
    /// Job id.
    pub id: String,
}

#[derive(Debug, Args)]
pub struct RemoveArgs {
    /// Job id.
    pub id: String,
}

#[derive(Debug, Args)]
pub struct WaitArgs {
    /// Job id.
    pub id: String,
}

#[derive(Debug, Args)]
pub struct ReportArgs {
    /// Job id.
    pub id: String,
    /// Print the raw report JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DaemonArgs {
    /// Internal: start the detached process (the public command behaves the same).
    #[arg(long, hide = true)]
    pub serve: bool,
    /// Exit after this many seconds with no jobs or clients (0 = never).
    #[arg(long, default_value_t = 30, value_name = "SECONDS")]
    pub idle_seconds: u64,
}

pub fn run_jobs(home: &Home, args: &JobsArgs) -> i32 {
    let store = JobStore::new(home);
    store.recover_interrupted();
    let records: Vec<JobRecord> = store
        .list()
        .into_iter()
        .filter(|record| args.all || !record.status.terminal())
        .collect();
    if args.json {
        match serde_json::to_string_pretty(&records) {
            Ok(text) => {
                println!("{text}");
                EXIT_OK
            }
            Err(error) => {
                eprintln!("jobs: cannot serialize jobs: {error}");
                EXIT_INFRA
            }
        }
    } else if records.is_empty() {
        println!("no {}jobs", if args.all { "" } else { "active " });
        EXIT_OK
    } else {
        println!(
            "{:<22} {:<12} {:<10} {:<24} PLAYS",
            "JOB", "STATUS", "DURATION", "DEVICES"
        );
        for record in records {
            let duration = jobs_duration(&record);
            let devices = if record.devices.is_empty() {
                "-".to_string()
            } else {
                record.devices.join(",")
            };
            let plays = if record.plays.is_empty() {
                "-".to_string()
            } else {
                record.plays.join(",")
            };
            println!(
                "{:<22} {:<12} {:<10} {:<24} {}",
                record.id,
                record.status.label(),
                duration,
                devices,
                plays
            );
            if let Some(position) = record.queue_position {
                println!("  queued (position {position})");
            }
            if let Some(message) = &record.message {
                if record.status.terminal() {
                    println!("  {message}");
                }
            }
        }
        EXIT_OK
    }
}

fn jobs_duration(record: &JobRecord) -> String {
    if let Some(duration) = record.duration_ms {
        format_duration(duration)
    } else if record.status == JobStatus::Running {
        "running".to_string()
    } else {
        "-".to_string()
    }
}

fn format_duration(millis: u64) -> String {
    let seconds = millis / 1_000;
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    }
}

pub fn attach(home: &Home, args: &AttachArgs) -> i32 {
    let store = JobStore::new(home);
    let Some(record) = read_or_report(&store, &args.id) else {
        return EXIT_USAGE;
    };
    if record.status.terminal() {
        print_tail(&store.console_path(&args.id), 0);
        print_finish(&record);
        return record.exit_code.unwrap_or(EXIT_INFRA);
    }
    follow_job(
        home,
        &args.id,
        FollowMode::Attach {
            stop_on_interrupt: args.stop,
        },
    )
}

pub fn wait(home: &Home, args: &WaitArgs) -> i32 {
    follow_job(home, &args.id, FollowMode::Wait)
}

enum FollowMode {
    Attach { stop_on_interrupt: bool },
    Wait,
    Logs { events: bool },
}

fn follow_job(home: &Home, id: &str, mode: FollowMode) -> i32 {
    let store = JobStore::new(home);
    let Some(_record) = read_or_report(&store, id) else {
        return EXIT_USAGE;
    };
    let mut printed_status = String::new();
    let mut offset = 0u64;
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("jobs: cannot start the async runtime: {error}");
            return EXIT_INFRA;
        }
    };
    runtime.block_on(async {
        loop {
            let console = match mode {
                FollowMode::Logs { events: true } => store.events_path(id),
                FollowMode::Logs { events: false } | FollowMode::Attach { .. } | FollowMode::Wait => {
                    store.console_path(id)
                }
            };
            print_tail(&console, offset);
            if let Ok(metadata) = std::fs::metadata(&console) {
                offset = metadata.len();
            }
            let Some(record) = store.read(id).ok().flatten() else {
                eprintln!("jobs: job '{id}' disappeared");
                return EXIT_INFRA;
            };
            let status = record.status.label().to_string();
            if status != printed_status {
                if matches!(mode, FollowMode::Attach { .. }) {
                    if let Some(position) = record.queue_position {
                        eprintln!("job {id}: {} (queue position {position})", record.status.label());
                    } else {
                        eprintln!("job {id}: {}", record.status.label());
                    }
                }
                printed_status = status;
            }
            if record.status.terminal() {
                if matches!(mode, FollowMode::Attach { .. } | FollowMode::Wait) {
                    print_finish(&record);
                }
                return record.exit_code.unwrap_or(EXIT_INFRA);
            }
            tokio::select! {
                signal = tokio::signal::ctrl_c() => {
                    if signal.is_err() {
                        eprintln!("jobs: cannot listen for Ctrl-C");
                    }
                    match mode {
                        FollowMode::Attach { stop_on_interrupt } => {
                            if stop_on_interrupt {
                                if let Err(error) = store.request_stop(id) {
                                    eprintln!("jobs: {error}");
                                    return EXIT_INFRA;
                                }
                                eprintln!("job {id}: stop requested");
                                return EXIT_INTERRUPTED;
                            }
                            eprintln!("detached from job {id}; it keeps running (`impresario attach {id}`)");
                            return EXIT_OK;
                        }
                        FollowMode::Wait => {
                            eprintln!("stopped waiting for job {id}; it keeps running");
                            return EXIT_INTERRUPTED;
                        }
                        FollowMode::Logs { .. } => return EXIT_OK,
                    }
                }
                () = tokio::time::sleep(POLL) => {}
            }
        }
    })
}

fn print_tail(path: &Path, offset: u64) {
    let Ok(mut file) = std::fs::File::open(path) else {
        return;
    };
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return;
    }
    let mut text = String::new();
    if file.read_to_string(&mut text).is_ok() && !text.is_empty() {
        print!("{text}");
        if !text.ends_with('\n') {
            println!();
        }
        let _ = std::io::stdout().flush();
    }
}

fn print_finish(record: &JobRecord) {
    if let Some(message) = &record.message {
        eprintln!("job {}: {}", record.id, message);
    } else {
        eprintln!("job {}: {}", record.id, record.status.label());
    }
}

fn read_or_report(store: &JobStore, id: &str) -> Option<JobRecord> {
    match store.read(id) {
        Ok(Some(record)) => Some(record),
        Ok(None) => {
            eprintln!("jobs: unknown job '{id}'");
            None
        }
        Err(error) => {
            eprintln!("jobs: {error}");
            None
        }
    }
}

pub fn logs(home: &Home, args: &LogsArgs) -> i32 {
    let store = JobStore::new(home);
    let Some(record) = read_or_report(&store, &args.id) else {
        return EXIT_USAGE;
    };
    if args.follow && !record.status.terminal() {
        return follow_job(
            home,
            &args.id,
            FollowMode::Logs {
                events: args.events,
            },
        );
    }
    let path = if args.events {
        store.events_path(&args.id)
    } else {
        store.console_path(&args.id)
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            print!("{text}");
            EXIT_OK
        }
        Err(error) => {
            eprintln!("jobs: cannot read {}: {error}", path.display());
            EXIT_INFRA
        }
    }
}

pub fn stop(home: &Home, args: &StopArgs) -> i32 {
    let store = JobStore::new(home);
    match store.request_stop(&args.id) {
        Ok(true) => {
            println!("stop requested for job {}", args.id);
            EXIT_OK
        }
        Ok(false) => {
            println!("job {} is already finished", args.id);
            EXIT_OK
        }
        Err(error) => {
            eprintln!("jobs: {error}");
            EXIT_USAGE
        }
    }
}

pub fn remove(home: &Home, args: &RemoveArgs) -> i32 {
    let store = JobStore::new(home);
    match store.remove(&args.id) {
        Ok(()) => {
            println!("removed job {}", args.id);
            EXIT_OK
        }
        Err(error) => {
            eprintln!("jobs: {error}");
            EXIT_FAILURE
        }
    }
}

pub fn report(home: &Home, args: &ReportArgs) -> i32 {
    let store = JobStore::new(home);
    let Some(record) = read_or_report(&store, &args.id) else {
        return EXIT_USAGE;
    };
    if !record.status.terminal() {
        eprintln!(
            "jobs: job {} is {}; wait for it before reading the report",
            args.id,
            record.status.label()
        );
        return EXIT_INFRA;
    }
    match jobs::report(home, &args.id) {
        Ok(report) => {
            if args.json {
                match serde_json::to_string_pretty(&report) {
                    Ok(text) => println!("{text}"),
                    Err(error) => {
                        eprintln!("jobs: cannot serialize the report: {error}");
                        return EXIT_INFRA;
                    }
                }
            } else {
                print!("{}", reporters::console::render(&report, false));
            }
            EXIT_OK
        }
        Err(error) => {
            eprintln!("jobs: {error}");
            EXIT_INFRA
        }
    }
}

pub fn daemon(home: &Home, args: &DaemonArgs) -> i32 {
    let _ = args.serve;
    match jobs::serve(home, Duration::from_secs(args.idle_seconds)) {
        Ok(()) => EXIT_OK,
        Err(error) => {
            eprintln!("daemon: {error}");
            EXIT_INFRA
        }
    }
}

/// A tiny helper for the TUI: newest active job.
#[must_use]
pub fn active_jobs(home: &Home) -> Vec<JobRecord> {
    JobStore::new(home)
        .list()
        .into_iter()
        .filter(|record| !record.status.terminal())
        .collect()
}

/// The age of a job's creation timestamp is not parsed here; the TUI shows the
/// wall-clock string, which is stable and locale-free.
#[must_use]
pub fn age_label(record: &JobRecord) -> String {
    let created = record.created_at.clone();
    if record.status.terminal() {
        created
    } else {
        format!("{created} (active)")
    }
}

/// Poll interval exposed for tests.
#[must_use]
pub fn poll_interval() -> Duration {
    POLL
}

/// Helper used by tests to wait for a file/terminal state without a TUI.
#[must_use]
pub fn wait_until<F: FnMut() -> bool>(mut predicate: F, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(POLL);
    }
    predicate()
}
