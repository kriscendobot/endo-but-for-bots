//! Unified Endo binary.
//!
//! A single executable that can act as the top-level endo daemon
//! (the capability bus that routes envelopes among its children),
//! a worker, or a standalone archive runner. The subcommand picks
//! the role; the `-e` / `--engine` flag picks which engine runs
//! inside this process. XS is the default engine for every
//! child-facing subcommand.
//!
//!   endor daemon                # daemon / capability bus (foreground)
//!   endor start                 # detached daemon
//!   endor stop                  # graceful shutdown
//!   endor ping                  # liveness check
//!
//!   endor worker  [-e xs]       # supervised worker child
//!   endor run     [-e xs] [-i] <archive.zip>
//!                               # standalone archive runner
//!
//! The manager is hosted in-process by `endor daemon` on a
//! dedicated `std::thread`; there is no separate `manager`
//! subcommand. Set `ENDO_MANAGER_NODE=1` to fall back to the
//! legacy Node.js daemon child for one release.
//!
//! The optional `-e <engine>` flag makes the engine explicit. Future
//! engines (`-e wasm`, `-e native`) slot in without changing the
//! subcommand vocabulary.
//!
//! The optional `-i` / `--interactive` flag selects the TUI host
//! mode for `endor run`. Without it, the archive runs as a
//! conventional UNIX process: stdout is the program's output,
//! stderr is for diagnostics, and there is no curses-style
//! repaint. With `-i`, `endor` opens the interactive TUI host
//! defined by `designs/endor-bus-tui.md` and surfaces an
//! "inspector" pane that captures worker `console.*`, telemetry,
//! and (someday) a stepping debugger. The Endo platform never
//! treats `console` as a stdout writer; logs flow through a
//! dedicated capability into the inspector. See
//! `packages/tui/index.js`.

use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::ExitCode;

use endo::error::EndoError;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let subcommand = match args.get(1) {
        Some(s) => s.as_str(),
        None => {
            print_help();
            return ExitCode::from(2);
        }
    };
    let rest = &args[2..];

    // All child-facing subcommands default to the XS engine;
    // `-e <engine>` makes it explicit.
    let engine = parse_engine(rest).unwrap_or("xs");

    match subcommand {
        "daemon" => match engine {
            "xs" => result_to_exit("endor", cmd_daemon()),
            other => unknown_engine(other),
        },
        "manager" => {
            // The `manager` subcommand has been removed. The daemon
            // now hosts the manager in-process on a dedicated
            // thread. Keep a one-line deprecation message for the
            // benefit of stale scripts; delete this arm in the next
            // release.
            eprintln!(
                "endor: the `manager` subcommand has been removed; \
                 `endor daemon` now hosts the manager in-process"
            );
            ExitCode::SUCCESS
        }
        "worker" => match engine {
            "xs" => xsnap_result_to_exit(unsafe { xsnap::run_xs_worker() }),
            other => unknown_engine(other),
        },
        "run" => {
            let cas_hash = parse_flag_value(rest, "--cas");
            let no_cas = rest.iter().any(|a| a == "--no-cas");
            let mode = parse_tui_mode(rest);
            let path = parse_positional_path(rest);
            match engine {
                "xs" => {
                    if mode == TuiMode::Interactive {
                        // The interactive TUI host is stubbed: see
                        // designs/endor-bus-tui.md and
                        // packages/tui/. The Rust ratatui/crossterm
                        // wiring lands in a follow-up; for now the
                        // flag is parsed and rejected with a clear
                        // diagnostic so callers can start adopting
                        // it without surprise.
                        eprintln!(
                            "endor: --interactive TUI mode is not yet \
                             implemented (stub); see designs/endor-bus-tui.md"
                        );
                        return ExitCode::from(78);
                    }
                    if let Some(hash) = cas_hash {
                        // Run from CAS root hash.
                        result_to_exit("endor", cmd_run_from_cas(&hash))
                    } else if let Some(ref p) = path {
                        if no_cas {
                            xsnap_result_to_exit(xsnap::run_xs_archive(p))
                        } else {
                            result_to_exit("endor", cmd_run_with_cas(p))
                        }
                    } else {
                        eprintln!("usage: endor run [-e xs] [-i|--interactive] [--cas <hash>] [--no-cas] <archive.zip>");
                        ExitCode::from(2)
                    }
                }
                other => unknown_engine(other),
            }
        }
        "start" => result_to_exit("endor", cmd_start()),
        "stop" => result_to_exit("endor", cmd_stop()),
        "ping" => result_to_exit("endor", cmd_ping()),
        "gc" => result_to_exit("endor", cmd_gc()),
        "-h" | "--help" => {
            print_help();
            ExitCode::SUCCESS
        }
        "help" => {
            match rest.first().map(|s| s.as_str()) {
                Some(sub) => print_subcommand_help(sub),
                None => print_help(),
            }
            ExitCode::SUCCESS
        }
        _ => {
            print_help();
            ExitCode::from(2)
        }
    }
}

fn print_help() {
    eprintln!("Usage: endor <command> [options]");
    eprintln!();
    eprintln!("Daemon commands (the daemon is the capability bus):");
    eprintln!("  daemon              Run the daemon in foreground");
    eprintln!("  start               Spawn the daemon in a detached session");
    eprintln!("  stop                Gracefully stop a running daemon");
    eprintln!("  ping                Verify daemon responsiveness");
    eprintln!();
    eprintln!("Child-facing commands (XS engine by default):");
    eprintln!("  worker  [-e xs]                Run a supervised worker child");
    eprintln!("  run     [-e xs] [-i] <archive.zip>");
    eprintln!("                                 Run a compartment-map archive");
    eprintln!("                                 (-i / --interactive opens the TUI host)");
    eprintln!();
    eprintln!("Maintenance:");
    eprintln!("  gc                             Garbage-collect the CAS");
}

fn print_subcommand_help(sub: &str) {
    match sub {
        "daemon" => {
            eprintln!("Usage: endor daemon");
            eprintln!();
            eprintln!("Run the Endo daemon in the foreground.");
            eprintln!();
            eprintln!("The daemon is the capability bus that routes envelopes among");
            eprintln!("workers. It hosts the JS manager in-process on a dedicated");
            eprintln!("thread (set ENDO_MANAGER_NODE=1 to use the legacy Node.js");
            eprintln!("daemon child instead).");
            eprintln!();
            eprintln!("Environment:");
            eprintln!("  ENDO_WORKER_THREADS   Tokio worker thread count (default: 4)");
            eprintln!("  ENDO_MANAGER_NODE     If set, use Node.js manager child");
            eprintln!("  ENDO_DAEMON_PATH      Path to Node.js daemon script (with ENDO_MANAGER_NODE)");
            eprintln!("  ENDO_DEFAULT_PLATFORM Default worker platform: separate, shared, node");
            eprintln!("  ENDO_TRACE            Enable trace logging");
        }
        "start" => {
            eprintln!("Usage: endor start");
            eprintln!();
            eprintln!("Spawn the daemon in a detached session.");
            eprintln!();
            eprintln!("If a daemon is already running (PID file exists and process is");
            eprintln!("alive), this is a no-op. Otherwise, forks a new daemon process");
            eprintln!("with setsid and waits up to 10 seconds for the Unix socket to");
            eprintln!("accept connections.");
        }
        "stop" => {
            eprintln!("Usage: endor stop");
            eprintln!();
            eprintln!("Gracefully stop a running daemon.");
            eprintln!();
            eprintln!("Sends SIGINT to the daemon process and waits up to 5 seconds");
            eprintln!("for it to exit.");
        }
        "ping" => {
            eprintln!("Usage: endor ping");
            eprintln!();
            eprintln!("Verify daemon responsiveness.");
            eprintln!();
            eprintln!("Connects to the daemon's Unix socket and immediately");
            eprintln!("disconnects. Prints \"pong\" on success; exits non-zero if");
            eprintln!("the daemon is not running or the socket is unreachable.");
        }
        "worker" => {
            eprintln!("Usage: endor worker [-e xs]");
            eprintln!();
            eprintln!("Run a supervised worker child.");
            eprintln!();
            eprintln!("This is not invoked directly — the daemon spawns worker");
            eprintln!("processes as needed. The worker communicates with the daemon");
            eprintln!("over fd 3 (read) and fd 4 (write) using CBOR-framed envelopes.");
            eprintln!();
            eprintln!("Options:");
            eprintln!("  -e, --engine <engine>  Engine to use (default: xs)");
        }
        "run" => {
            eprintln!("Usage: endor run [-e xs] [-i|--interactive] <archive.zip>");
            eprintln!();
            eprintln!("Run a compartment-map archive standalone.");
            eprintln!();
            eprintln!("Executes the given .zip archive in an XS machine without a");
            eprintln!("running daemon. Useful for testing and one-off execution.");
            eprintln!();
            eprintln!("Without -i, the archive runs as a conventional UNIX process:");
            eprintln!("stdout is the program's output, stderr is for diagnostics, and");
            eprintln!("there is no curses-style repaint.");
            eprintln!();
            eprintln!("With -i / --interactive, endor opens the TUI host (see");
            eprintln!("designs/endor-bus-tui.md) and exposes an inspector pane that");
            eprintln!("captures worker console.* output, telemetry, and (someday) a");
            eprintln!("stepping debugger. The Endo platform never treats console as");
            eprintln!("a stdout writer; logs flow through a dedicated capability.");
            eprintln!();
            eprintln!("Options:");
            eprintln!("  -e, --engine <engine>  Engine to use (default: xs)");
            eprintln!("  -i, --interactive      Open the interactive TUI host (stub)");
        }
        "gc" => {
            eprintln!("Usage: endor gc");
            eprintln!();
            eprintln!("Garbage-collect the content-addressed store.");
            eprintln!();
            eprintln!("Removes CAS entries that have zero retained references and");
            eprintln!("are not transitively referenced by any live tree root.");
            eprintln!("Safe to run while the daemon is stopped.");
        }
        "help" => {
            eprintln!("Usage: endor help [command]");
            eprintln!();
            eprintln!("Show help for a command. Without arguments, prints general usage.");
        }
        _ => {
            eprintln!("endor: unknown command '{sub}'");
            eprintln!();
            print_help();
        }
    }
}

fn result_to_exit(prog: &str, result: Result<(), EndoError>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{prog}: {e}");
            ExitCode::from(1)
        }
    }
}

fn xsnap_result_to_exit(result: Result<(), xsnap::XsnapError>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("endor: {e}");
            ExitCode::from(1)
        }
    }
}

fn unknown_engine(engine: &str) -> ExitCode {
    eprintln!("endor: unknown engine: {engine}");
    ExitCode::from(2)
}

/// Returns the value of `-e <engine>` / `--engine=<engine>` if present
/// in `args`, else None. The flag may appear anywhere after the
/// subcommand.
fn parse_engine(args: &[String]) -> Option<&str> {
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "-e" || a == "--engine" {
            if i + 1 < args.len() {
                return Some(args[i + 1].as_str());
            }
            return None;
        }
        if let Some(rest) = a.strip_prefix("--engine=") {
            return Some(rest);
        }
        if let Some(rest) = a.strip_prefix("-e=") {
            return Some(rest);
        }
        i += 1;
    }
    None
}

/// Returns the first positional argument (non-flag, non-flag-value).
fn parse_positional_path(args: &[String]) -> Option<PathBuf> {
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "-e" || a == "--engine" || a == "--cas" || a == "--cas-dir" {
            // skip the flag value as well
            i += 2;
            continue;
        }
        if a.starts_with("--engine=") || a.starts_with("-e=")
            || a.starts_with("--cas=") || a.starts_with("--cas-dir=")
        {
            i += 1;
            continue;
        }
        if a.starts_with('-') {
            i += 1;
            continue;
        }
        return Some(PathBuf::from(a));
    }
    None
}

/// TUI host mode for `endor run`. `endor` decides between an
/// interactive curses-style TUI (with the inspector pane defined in
/// `designs/endor-bus-tui.md`) and conventional UNIX output. This
/// mirrors the JS-side mode contract in `packages/tui/index.js`.
#[derive(Debug, PartialEq, Eq)]
pub enum TuiMode {
    /// No TUI; stdout is the program's output, stderr is for
    /// diagnostics. This is the default and matches the behavior
    /// of any other UNIX tool.
    Unix,
    /// Open the interactive TUI host with the inspector pane.
    /// Worker `console.*` is captured by a dedicated logging
    /// capability; the platform never treats `console` as a stdout
    /// writer. Selected by `-i` or `--interactive`.
    Interactive,
}

/// Returns `Interactive` if `-i` or `--interactive` is anywhere in
/// `args`, else `Unix`.
fn parse_tui_mode(args: &[String]) -> TuiMode {
    if args.iter().any(|a| a == "-i" || a == "--interactive") {
        TuiMode::Interactive
    } else {
        TuiMode::Unix
    }
}

/// Parse a flag with a value (e.g., --cas <hash>).
fn parse_flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == flag {
            if i + 1 < args.len() {
                return Some(&args[i + 1]);
            }
            return None;
        }
        let prefix = format!("{flag}=");
        if let Some(rest) = args[i].strip_prefix(&prefix) {
            return Some(rest);
        }
        i += 1;
    }
    None
}

fn cmd_daemon() -> Result<(), EndoError> {
    let worker_threads = std::env::var("ENDO_WORKER_THREADS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(4);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(worker_threads)
        .enable_all()
        .build()
        .map_err(|e| EndoError::Config(format!("tokio runtime: {e}")))?;

    runtime.block_on(async {
        let mut e = endo::endo::Endo::start()?;
        e.serve().await?;
        e.stop().await;
        Ok(())
    })
}

fn cmd_start() -> Result<(), EndoError> {
    let executable = std::env::current_exe()
        .map_err(|e| EndoError::Config(format!("current exe: {e}")))?
        .to_string_lossy()
        .to_string();
    let paths = endo::endo::start_detached(&executable)?;
    eprintln!("endor: started (log {})", paths.log_path.display());
    Ok(())
}

fn cmd_stop() -> Result<(), EndoError> {
    let paths = endo::endo::ensure_running()?;

    if let Ok(conn) = UnixStream::connect(&paths.sock_path) {
        let _ = conn.shutdown(Shutdown::Both);
    }

    let pid = endo::pidfile::read_pid(&paths.ephemeral_path)?;
    if pid != 0 && endo::pidfile::is_process_running(pid) {
        unsafe {
            libc::kill(pid as i32, libc::SIGINT);
        }
        for _ in 0..100 {
            if !endo::pidfile::is_process_running(pid) {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        return Err(EndoError::Timeout(format!(
            "process {pid} did not exit within 5s"
        )));
    }

    Ok(())
}

fn cmd_ping() -> Result<(), EndoError> {
    let paths = endo::endo::ensure_running()?;
    let conn = UnixStream::connect(&paths.sock_path)?;
    let _ = conn.shutdown(Shutdown::Both);
    eprintln!("pong");
    Ok(())
}

fn cmd_run_with_cas(archive_path: &std::path::Path) -> Result<(), EndoError> {
    // Create a temporary CAS for standalone runs.
    let cas_dir = std::env::temp_dir().join("endor-cas");
    let cas = endo::cas::ContentStore::open(&cas_dir)
        .map_err(|e| EndoError::Config(format!("CAS open: {e}")))?;

    let bytes = std::fs::read(archive_path)
        .map_err(|e| EndoError::Config(format!("cannot open {}: {e}", archive_path.display())))?;
    let cursor = std::io::Cursor::new(&bytes);

    let ingested = endo::cas_archive::ingest_archive(&cas, cursor)
        .map_err(|e| EndoError::Config(format!("CAS ingest: {e}")))?;

    eprintln!("endor[run]: archive root {}", ingested.root_hash);

    // Run the archive using the loaded archive (already in memory).
    xsnap::run_xs_archive_loaded(&ingested.archive)
        .map_err(|e| EndoError::Config(format!("run: {e}")))?;
    Ok(())
}

fn cmd_run_from_cas(root_hash: &str) -> Result<(), EndoError> {
    let cas_dir = std::env::temp_dir().join("endor-cas");
    let cas = endo::cas::ContentStore::open(&cas_dir)
        .map_err(|e| EndoError::Config(format!("CAS open: {e}")))?;

    let archive = endo::cas_archive::load_archive_from_cas(&cas, root_hash)
        .map_err(|e| EndoError::Config(format!("CAS load: {e}")))?;

    xsnap::run_xs_archive_loaded(&archive)
        .map_err(|e| EndoError::Config(format!("run: {e}")))?;
    Ok(())
}

fn cmd_gc() -> Result<(), EndoError> {
    let paths = endo::paths::resolve_paths()?;
    let cas_dir = paths.state_path.join("store-sha256");
    if !cas_dir.exists() {
        eprintln!("endor gc: no CAS directory at {}", cas_dir.display());
        return Ok(());
    }
    let cas = endo::cas::ContentStore::open(&cas_dir)?;
    let live_roots = std::collections::HashSet::new();
    let report = cas.gc(&live_roots)?;
    eprintln!(
        "endor gc: freed {} entries ({} bytes)",
        report.freed_count, report.freed_bytes
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_tui_mode_defaults_to_unix() {
        assert_eq!(parse_tui_mode(&args(&["foo.zip"])), TuiMode::Unix);
        assert_eq!(parse_tui_mode(&args(&[])), TuiMode::Unix);
    }

    #[test]
    fn parse_tui_mode_short_flag_selects_interactive() {
        assert_eq!(
            parse_tui_mode(&args(&["-i", "foo.zip"])),
            TuiMode::Interactive
        );
        assert_eq!(
            parse_tui_mode(&args(&["foo.zip", "-i"])),
            TuiMode::Interactive
        );
    }

    #[test]
    fn parse_tui_mode_long_flag_selects_interactive() {
        assert_eq!(
            parse_tui_mode(&args(&["--interactive", "foo.zip"])),
            TuiMode::Interactive
        );
    }

    #[test]
    fn parse_positional_path_skips_interactive_flag() {
        assert_eq!(
            parse_positional_path(&args(&["-i", "foo.zip"])),
            Some(PathBuf::from("foo.zip"))
        );
        assert_eq!(
            parse_positional_path(&args(&["--interactive", "foo.zip"])),
            Some(PathBuf::from("foo.zip"))
        );
    }
}
