use std::{path::PathBuf, process::ExitCode, sync::Arc};

use control_plane::admin_http::{start_admin_server, stop_admin_server};
use naughtian_kallisto::{KallistoCore, event::worker::WorkerPool, server::http_handler::AppState};

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

const DEFAULT_DB_PATH: &str = "/tmp/kallisto_server_bench";
const DEFAULT_WORKERS: usize = 2;
const DEFAULT_HTTP_PORT: u16 = 8200;
const DEFAULT_ADMIN_PORT: u16 = 8202;

#[derive(Debug)]
struct Config {
    db_path: PathBuf,
    workers: usize,
    http_port: u16,
    admin_port: u16,
    wipe: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            db_path: std::env::var_os("KALLISTO_DB_PATH")
                .map_or_else(|| PathBuf::from(DEFAULT_DB_PATH), PathBuf::from),
            workers: DEFAULT_WORKERS,
            http_port: DEFAULT_HTTP_PORT,
            admin_port: DEFAULT_ADMIN_PORT,
            wipe: false,
        }
    }
}

const USAGE: &str = "\
kallisto-server

  --db-path=PATH      storage directory (env: KALLISTO_DB_PATH)
  --workers=N         data-plane workers, one runtime per core
  --http-port=PORT    data API port
  --admin-port=PORT   admin API port
  --wipe              delete the storage directory before opening it
  -h, --help          show this message
";

/// Parses `--flag=value` and `--flag value`.
///
/// Every one of these flags was previously accepted and silently discarded:
/// the binary hardcoded its port, its worker count and its storage path. The
/// benchmark scripts have been passing `--workers` and `--http-port` all along,
/// so their reported worker counts did not describe the process being measured;
/// and because the hardcoded path was also deleted on every startup, the server
/// could not persist across a restart at all.
fn parse_args<I: Iterator<Item = String>>(args: I) -> Result<Config, String> {
    let mut cfg = Config::default();
    let mut args = args.peekable();

    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) => (f.to_string(), Some(v.to_string())),
            None => (arg, None),
        };

        // Only consume the next argument for flags that take a value.
        let mut value = |flag: &str| -> Result<String, String> {
            match inline.clone() {
                Some(v) => Ok(v),
                None => args
                    .next()
                    .ok_or_else(|| format!("{flag} requires a value")),
            }
        };

        match flag.as_str() {
            "--db-path" => cfg.db_path = PathBuf::from(value("--db-path")?),
            "--workers" => {
                let raw = value("--workers")?;
                cfg.workers = raw
                    .parse()
                    .map_err(|_| format!("--workers expects a number, got {raw:?}"))?;
                if cfg.workers == 0 {
                    return Err("--workers must be at least 1".to_string());
                }
            }
            "--http-port" => {
                let raw = value("--http-port")?;
                cfg.http_port = raw
                    .parse()
                    .map_err(|_| format!("--http-port expects a port, got {raw:?}"))?;
            }
            "--admin-port" => {
                let raw = value("--admin-port")?;
                cfg.admin_port = raw
                    .parse()
                    .map_err(|_| format!("--admin-port expects a port, got {raw:?}"))?;
            }
            "--wipe" => cfg.wipe = true,
            "-h" | "--help" => return Err(USAGE.to_string()),
            other => return Err(format!("unrecognised argument {other:?}\n\n{USAGE}")),
        }
    }

    Ok(cfg)
}

fn main() -> ExitCode {
    let cfg = match parse_args(std::env::args().skip(1)) {
        Ok(cfg) => cfg,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };

    // Only ever on an explicit request. This used to happen unconditionally on
    // every startup, which silently made the store non-durable.
    if cfg.wipe && cfg.db_path.exists() {
        if let Err(e) = std::fs::remove_dir_all(&cfg.db_path) {
            eprintln!("failed to wipe {}: {e}", cfg.db_path.display());
            return ExitCode::FAILURE;
        }
    }

    let Some(db_path) = cfg.db_path.to_str() else {
        eprintln!("--db-path must be valid UTF-8: {}", cfg.db_path.display());
        return ExitCode::from(2);
    };

    let core = match KallistoCore::new(db_path) {
        Ok(core) => Arc::new(core),
        Err(e) => {
            eprintln!("failed to open storage at {db_path}: {e:?}");
            return ExitCode::FAILURE;
        }
    };

    let state = AppState {
        registry: core.registry.clone(),
    };

    println!(
        "Starting Kallisto on data port {} (admin {}) with {} worker(s), storage at {}",
        cfg.http_port, cfg.admin_port, cfg.workers, db_path
    );

    let pool = WorkerPool::spawn(cfg.workers, cfg.http_port, state.clone());
    let admin_server = start_admin_server(core.clone(), cfg.admin_port);

    pool.join_all();
    stop_admin_server(admin_server);
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Config, String> {
        parse_args(args.iter().map(|s| (*s).to_string()))
    }

    #[test]
    fn defaults_when_no_arguments() {
        let cfg = parse(&[]).unwrap();
        assert_eq!(cfg.workers, DEFAULT_WORKERS);
        assert_eq!(cfg.http_port, DEFAULT_HTTP_PORT);
        assert_eq!(cfg.admin_port, DEFAULT_ADMIN_PORT);
        assert!(!cfg.wipe, "the store must not be wiped unless asked");
    }

    #[test]
    fn accepts_inline_and_separated_values() {
        let cfg = parse(&["--db-path=/data/k", "--workers", "8", "--http-port=9000"]).unwrap();
        assert_eq!(cfg.db_path, PathBuf::from("/data/k"));
        assert_eq!(cfg.workers, 8);
        assert_eq!(cfg.http_port, 9000);
    }

    #[test]
    fn the_flags_the_benchmark_scripts_pass_are_honoured() {
        // Exactly the invocation in benchmarks/server/run_release_bench.sh.
        let cfg = parse(&[
            "--http-port=8200",
            "--workers=4",
            "--db-path=/tmp/kallisto_release_bench_data",
        ])
        .unwrap();
        assert_eq!(cfg.workers, 4);
        assert_eq!(cfg.http_port, 8200);
        assert_eq!(
            cfg.db_path,
            PathBuf::from("/tmp/kallisto_release_bench_data")
        );
    }

    #[test]
    fn rejects_bad_input_instead_of_ignoring_it() {
        parse(&["--workers=zero"]).unwrap_err();
        parse(&["--workers=0"]).unwrap_err();
        parse(&["--db-path"]).unwrap_err();
        parse(&["--nonsense"]).unwrap_err();
    }

    #[test]
    fn wipe_is_opt_in() {
        assert!(parse(&["--wipe"]).unwrap().wipe);
    }
}
