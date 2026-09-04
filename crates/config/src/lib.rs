use anyhow::{Context, bail};
use serde::Deserialize;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

mod http;
mod listen;
mod log;
mod pool;
mod supervisor;

pub use http::{
    HttpSettings, MiddlewareSettings, StaticSettings, UnsafeFieldNames, UploadSettings,
};
pub use listen::{Listen, ListenParseError};
pub use log::{LogFormat, LogLevel, LogSettings};
pub use pool::{PoolSettings, RunMode};
pub use supervisor::SupervisorSettings;

use http::{HttpSection, resolve_middleware, resolve_static, resolve_uploads};
use log::{LogSection, resolve_log};
use pool::{PoolSection, resolve_pool};
use supervisor::{SupervisorSection, resolve_supervisor};

#[derive(Debug, Default)]
pub struct Overrides {
    pub listen: Option<Listen>,
    pub processes: Option<usize>,
    pub mode: Option<RunMode>,
    pub entrypoint: Option<PathBuf>,
}

#[derive(Debug)]
pub struct Settings {
    pub http: HttpSettings,
    pub pool: PoolSettings,
    pub supervisor: SupervisorSettings,
    pub log: LogSettings,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    #[serde(default)]
    http: HttpSection,
    #[serde(default)]
    pool: PoolSection,
    #[serde(default)]
    supervisor: SupervisorSection,
    #[serde(default)]
    log: LogSection,
}

fn default_listen() -> Listen {
    Listen::Tcp(SocketAddr::from((Ipv4Addr::LOCALHOST, 8000)))
}

pub fn resolve(config_path: Option<&Path>, cli: Overrides) -> anyhow::Result<Settings> {
    let (file, config_dir) = match config_path {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading config file {}", path.display()))?;
            let file = load_str(&text)
                .with_context(|| format!("parsing config file {}", path.display()))?;
            (file, path.parent().map(Path::to_owned))
        }
        None => (FileConfig::default(), None),
    };
    merge(file, cli, config_dir.as_deref())
}

fn load_str(text: &str) -> anyhow::Result<FileConfig> {
    Ok(toml::from_str(text)?)
}

fn merge(file: FileConfig, cli: Overrides, config_dir: Option<&Path>) -> anyhow::Result<Settings> {
    let listen = match &cli.listen {
        Some(l) => l.clone(),
        None => match file.http.listen.as_deref() {
            Some(s) => s
                .parse::<Listen>()
                .with_context(|| format!("invalid http.listen `{s}`"))?,
            None => default_listen(),
        },
    };

    let server_port = match file.http.server_port {
        Some(p) => p,
        None => match &listen {
            Listen::Tcp(addr) => addr.port(),
        },
    };

    let max_body_size_mb = file.http.max_body_size_mb.unwrap_or(8);
    if max_body_size_mb == 0 {
        bail!("http.max_body_size_mb must be at least 1");
    }
    let max_body_size = max_body_size_mb
        .checked_mul(1024 * 1024)
        .ok_or_else(|| anyhow::anyhow!("http.max_body_size_mb {max_body_size_mb} is too large"))?;

    let write_timeout_secs = file.http.write_timeout_secs.unwrap_or(30);
    if write_timeout_secs == 0 {
        bail!("http.write_timeout_secs must be at least 1");
    }
    let write_timeout = capped_timeout("http", "write_timeout_secs", write_timeout_secs)?;

    let keepalive_timeout_secs = file.http.keepalive_timeout_secs.unwrap_or(60);
    if keepalive_timeout_secs == 0 {
        bail!("http.keepalive_timeout_secs must be at least 1");
    }
    let keepalive_timeout =
        capped_timeout("http", "keepalive_timeout_secs", keepalive_timeout_secs)?;

    let sendfile_root = match file.http.sendfile.root.filter(|r| !r.is_empty()) {
        Some(r) => Some(config_relative(config_dir, &r)?),
        None => None,
    };

    let static_files = match file.http.r#static {
        Some(section) => Some(resolve_static(section, config_dir)?),
        None => None,
    };

    let pool = resolve_pool(file.pool, &cli, config_dir, "pool")?;
    if file.http.uploads.is_some() && pool.mode != RunMode::Dispatcher {
        bail!(
            "http.uploads applies to dispatcher mode only (pool.mode = \"{}\")",
            pool.mode.as_str()
        );
    }
    let uploads = resolve_uploads(file.http.uploads.unwrap_or_default(), config_dir)?;
    let supervisor = resolve_supervisor(file.supervisor, config_dir)?;
    let log = resolve_log(file.log)?;
    let middleware = resolve_middleware(file.http.middleware, static_files)?;

    Ok(Settings {
        http: HttpSettings {
            listen,
            server_name: file
                .http
                .server_name
                .unwrap_or_else(|| "localhost".to_owned()),
            server_port,
            max_body_size,
            write_timeout,
            keepalive_timeout,
            unsafe_field_names: file.http.unsafe_field_names.unwrap_or_default(),
            uploads,
            sendfile_root,
            middleware,
        },
        pool,
        supervisor,
        log,
    })
}

/// Caps every `*_secs` key so deadline arithmetic cannot overflow.
const MAX_TIMEOUT_SECS: u64 = 86_400;

fn capped_timeout(table: &str, key: &str, secs: u64) -> anyhow::Result<Duration> {
    if secs > MAX_TIMEOUT_SECS {
        bail!("{table}.{key} {secs} is too large (max {MAX_TIMEOUT_SECS})");
    }
    Ok(Duration::from_secs(secs))
}

fn config_relative(config_dir: Option<&Path>, value: &str) -> std::io::Result<PathBuf> {
    std::path::absolute(config_dir.unwrap_or_else(|| Path::new(".")).join(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_cli_over_file_over_default() {
        let file = load_str(
            r#"
            [http]
            listen = "0.0.0.0:9000"
            [pool]
            processes = 2
            entrypoint = "app.php"
        "#,
        )
        .unwrap();
        let cli = Overrides {
            listen: Some("127.0.0.1:1234".parse().unwrap()),
            processes: Some(7),
            mode: None,
            entrypoint: Some(PathBuf::from("cli.php")),
        };
        let s = merge(file, cli, Some(Path::new("/etc/rapira"))).unwrap();
        assert_eq!(s.http.listen.to_string(), "127.0.0.1:1234");
        assert_eq!(s.pool.processes, 7);
        assert!(s.pool.entrypoint.is_absolute());
        assert!(s.pool.entrypoint.ends_with("cli.php"));
    }

    #[test]
    fn server_port_derives_from_listen_and_mb_converts() {
        let file = load_str(
            "[http]\nlisten = \":9000\"\nmax_body_size_mb = 2\n[pool]\nentrypoint = \"a.php\"\n",
        )
        .unwrap();
        let s = merge(file, Overrides::default(), Some(Path::new("/w"))).unwrap();
        assert_eq!(s.http.server_port, 9000);
        assert_eq!(s.http.max_body_size, 2 * 1024 * 1024);

        let file =
            load_str("[http]\nlisten = \"unix:/run/r.sock\"\n[pool]\nentrypoint = \"a.php\"\n")
                .unwrap();
        let err = merge(file, Overrides::default(), Some(Path::new("/w"))).unwrap_err();
        assert!(format!("{err:#}").contains("use an IP address with a port or :port"));
    }

    #[test]
    fn unsafe_field_names_parses_and_defaults_to_drop() {
        for (text, want) in [
            ("drop", UnsafeFieldNames::Drop),
            ("reject", UnsafeFieldNames::Reject),
        ] {
            let file = load_str(&format!(
                "[http]\nunsafe_field_names = \"{text}\"\n[pool]\nentrypoint = \"a.php\"\n"
            ))
            .unwrap();
            let s = merge(file, Overrides::default(), Some(Path::new("/w"))).unwrap();
            assert_eq!(s.http.unsafe_field_names, want, "{text}");
        }

        let file = load_str("[pool]\nentrypoint = \"a.php\"\n").unwrap();
        let s = merge(file, Overrides::default(), Some(Path::new("/w"))).unwrap();
        assert_eq!(s.http.unsafe_field_names, UnsafeFieldNames::Drop);
    }

    /// `allow` is invalid because this check cannot be disabled.
    #[test]
    fn unknown_unsafe_field_names_value_is_rejected() {
        for value in ["dorp", "allow"] {
            assert!(
                load_str(&format!(
                    "[http]\nunsafe_field_names = \"{value}\"\n[pool]\nentrypoint = \"a.php\"\n"
                ))
                .is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn file_entrypoint_is_config_dir_relative() {
        let file = load_str("[pool]\nentrypoint = \"public/index.php\"\n").unwrap();
        let s = merge(file, Overrides::default(), Some(Path::new("/srv/app"))).unwrap();
        assert_eq!(
            s.pool.entrypoint,
            std::path::absolute("/srv/app/public/index.php").unwrap()
        );
    }

    #[test]
    fn entrypoint_is_required() {
        let err = merge(FileConfig::default(), Overrides::default(), None).unwrap_err();
        assert!(err.to_string().contains("entrypoint"));

        let file = load_str("[pool]\nentrypoint = \"\"\n").unwrap();
        let err = merge(file, Overrides::default(), Some(Path::new("/srv/app"))).unwrap_err();
        assert!(err.to_string().contains("no entrypoint"));
    }

    #[test]
    fn max_body_size_overflow_is_rejected() {
        let file =
            load_str("[http]\nmax_body_size_mb = 17592186044416\n[pool]\nentrypoint = \"a.php\"\n")
                .unwrap();
        let err = merge(file, Overrides::default(), Some(Path::new("/w"))).unwrap_err();
        assert!(err.to_string().contains("too large"));
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(load_str("[pool]\nbogus = 1\n").is_err());
        assert!(load_str("[nope]\nx = 1\n").is_err());
        assert!(load_str("[supervisor]\nbogus = 1\n").is_err());
        assert!(load_str("[pool]\nthreads = 1\n").is_err());
        assert!(load_str("[pool]\nclassic = true\n").is_err());
        assert!(load_str("[pm]\nmode = \"static\"\n").is_err());
        assert!(load_str("[pool]\npidfile = \"r.pid\"\n").is_err());
        assert!(load_str("[supervisor]\nmax_requests = 1\n").is_err());
        assert!(load_str("[log]\nbogus = 1\n").is_err());
        assert!(load_str("[log]\nlevel = \"verbose\"\n").is_err());
        assert!(load_str("[log]\nformat = \"pretty\"\n").is_err());
        assert!(load_str("[http.static]\nbogus = 1\n").is_err());
    }

    #[test]
    fn removed_pool_keys_are_unknown_fields() {
        for (key, value) in [
            ("scaling", "\"static\""),
            ("min_spare", "1"),
            ("max_spare", "2"),
            ("process_idle_timeout_secs", "10"),
            ("request_terminate_timeout_secs", "30"),
        ] {
            let err = load_str(&format!(
                "[pool]\nentrypoint = \"a.php\"\n{key} = {value}\n"
            ))
            .unwrap_err()
            .to_string();
            assert!(
                err.contains(&format!("unknown field `{key}`")),
                "{key}: {err}"
            );
        }
    }

    #[test]
    fn timeout_caps_name_the_key_that_broke() {
        for (toml, key) in [
            (
                "[pool]\nentrypoint = \"a.php\"\n[supervisor]\nprocess_control_timeout_secs = 100000\n",
                "supervisor.process_control_timeout_secs",
            ),
            (
                "[http]\nwrite_timeout_secs = 100000\n[pool]\nentrypoint = \"a.php\"\n",
                "http.write_timeout_secs",
            ),
            (
                "[http]\nkeepalive_timeout_secs = 100000\n[pool]\nentrypoint = \"a.php\"\n",
                "http.keepalive_timeout_secs",
            ),
        ] {
            let err = merge(
                load_str(toml).unwrap(),
                Overrides::default(),
                Some(Path::new("/w")),
            )
            .unwrap_err()
            .to_string();
            assert!(
                err.contains(key) && err.contains("too large"),
                "{key}: {err}"
            );
        }

        let file = load_str(
            "[pool]\nentrypoint = \"a.php\"\n[supervisor]\nprocess_control_timeout_secs = 86400\n",
        )
        .unwrap();
        assert!(merge(file, Overrides::default(), Some(Path::new("/w"))).is_ok());
    }

    /// The drain requires a positive stop budget.
    #[test]
    fn supervisor_control_timeout_zero_is_rejected() {
        let file = load_str(
            "[pool]\nentrypoint = \"a.php\"\n[supervisor]\nprocess_control_timeout_secs = 0\n",
        )
        .unwrap();
        let err = merge(file, Overrides::default(), Some(Path::new("/w")))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("supervisor.process_control_timeout_secs must be at least 1"),
            "{err}"
        );
    }

    /// Validation runs after precedence. Therefore, a CLI value of 0 is invalid even when the file contains a valid value.
    #[test]
    fn pool_processes_zero_is_rejected_from_either_layer() {
        let file = load_str("[pool]\nprocesses = 0\nentrypoint = \"a.php\"\n").unwrap();
        let err = merge(file, Overrides::default(), Some(Path::new("/w")))
            .unwrap_err()
            .to_string();
        assert!(err.contains("pool.processes must be at least 1"), "{err}");

        let file = load_str("[pool]\nprocesses = 4\nentrypoint = \"a.php\"\n").unwrap();
        let err = merge(
            file,
            Overrides {
                processes: Some(0),
                ..Default::default()
            },
            Some(Path::new("/w")),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("pool.processes must be at least 1"), "{err}");
    }

    /// The resolver converts all units and resolves the directory relative to the configuration file. A zero `max_files` value would reject every file part after a successful start.
    #[test]
    fn http_uploads_resolve_and_reject_zero_files() {
        let file = load_str(
            "[pool]\nentrypoint = \"a.php\"\n[http.uploads]\ndir = \"spool\"\n\
             max_file_size_mb = 3\nmax_field_size_kb = 7\nmax_files = 4\n\
             max_parts = 9\nmax_part_headers = 5\n",
        )
        .unwrap();
        let s = merge(file, Overrides::default(), Some(Path::new("/w"))).unwrap();
        let u = &s.http.uploads;
        assert_eq!(u.dir, std::path::absolute("/w/spool").unwrap());
        assert_eq!(u.max_file_size, 3 * 1024 * 1024);
        assert_eq!(u.max_field_size, 7 * 1024);
        assert_eq!(u.max_files, 4);
        assert_eq!(u.max_parts, 9);
        assert_eq!(u.max_part_headers, 5);

        let file =
            load_str("[pool]\nentrypoint = \"a.php\"\n[http.uploads]\nmax_files = 0\n").unwrap();
        let err = merge(file, Overrides::default(), Some(Path::new("/w")))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("http.uploads.max_files must be at least 1"),
            "{err}"
        );
    }

    #[test]
    fn pool_max_requests_resolves_for_worker_and_dispatcher() {
        for mode in ["worker", "dispatcher"] {
            for max_requests in [0, 500] {
                let file = load_str(&format!(
                    "[pool]\nentrypoint = \"a.php\"\nmode = \"{mode}\"\nmax_requests = {max_requests}\n"
                ))
                .unwrap();
                let s = merge(file, Overrides::default(), Some(Path::new("/w"))).unwrap();
                assert_eq!(s.pool.max_requests, max_requests, "{mode}");
            }
        }
    }

    /// The CLI mode has precedence over the file mode in all cases.
    #[test]
    fn pool_run_mode_resolves_with_cli_precedence() {
        for (key, want) in [
            ("classic", RunMode::Classic),
            ("worker", RunMode::Worker),
            ("dispatcher", RunMode::Dispatcher),
        ] {
            let file = load_str(&format!(
                "[pool]\nentrypoint = \"a.php\"\nmode = \"{key}\"\n"
            ))
            .unwrap();
            let s = merge(file, Overrides::default(), Some(Path::new("/w"))).unwrap();
            assert_eq!(s.pool.mode, want, "{key}");
        }

        let file = load_str("[pool]\nentrypoint = \"a.php\"\n").unwrap();
        let s = merge(file, Overrides::default(), Some(Path::new("/w"))).unwrap();
        assert_eq!(s.pool.mode, RunMode::Dispatcher, "default");

        let file = load_str("[pool]\nentrypoint = \"a.php\"\nmode = \"classic\"\n").unwrap();
        let s = merge(
            file,
            Overrides {
                mode: Some(RunMode::Dispatcher),
                ..Default::default()
            },
            Some(Path::new("/w")),
        )
        .unwrap();
        assert_eq!(s.pool.mode, RunMode::Dispatcher, "CLI beats file");

        assert!(load_str("[pool]\nentrypoint = \"a.php\"\nmode = \"async\"\n").is_err());
    }

    /// The table is valid only in dispatcher mode.
    #[test]
    fn http_uploads_require_dispatcher_mode() {
        for mode in ["classic", "worker"] {
            let file = load_str(&format!(
                "[pool]\nentrypoint = \"a.php\"\nmode = \"{mode}\"\n[http.uploads]\nmax_files = 4\n"
            ))
            .unwrap();
            let err = merge(file, Overrides::default(), Some(Path::new("/w")))
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("dispatcher mode only") && err.contains(mode),
                "{err}"
            );
        }

        let file = load_str("[pool]\nentrypoint = \"a.php\"\n[http.uploads]\n").unwrap();
        assert!(merge(file, Overrides::default(), Some(Path::new("/w"))).is_ok());

        let file = load_str("[pool]\nentrypoint = \"a.php\"\nmode = \"classic\"\n").unwrap();
        assert!(merge(file, Overrides::default(), Some(Path::new("/w"))).is_ok());
    }

    #[test]
    fn supervisor_pidfile_resolves_against_config_dir() {
        let file =
            load_str("[pool]\nentrypoint = \"a.php\"\n[supervisor]\npidfile = \"rapira.pid\"\n")
                .unwrap();
        let s = merge(file, Overrides::default(), Some(Path::new("/etc/rapira"))).unwrap();
        assert_eq!(
            s.supervisor.pidfile,
            Some(std::path::absolute("/etc/rapira/rapira.pid").unwrap())
        );
    }

    /// The root resolves relative to the configuration file. The default `forbid` value prevents access to PHP source files.
    #[test]
    fn http_static_resolves_with_defaults() {
        let file = load_str(
            "[pool]\nentrypoint = \"a.php\"\n[http]\nmiddleware = [\"static\"]\n[http.static]\nroot = \"public\"\n",
        )
        .unwrap();
        let s = merge(file, Overrides::default(), Some(Path::new("/w"))).unwrap();
        let MiddlewareSettings::Static(st) = &s.http.middleware[0];
        assert_eq!(st.root, std::path::absolute("/w/public").unwrap());
        assert_eq!(st.forbid, vec![".php".to_owned()]);

        let file = load_str("[pool]\nentrypoint = \"a.php\"\n").unwrap();
        let s = merge(file, Overrides::default(), Some(Path::new("/w"))).unwrap();
        assert!(s.http.middleware.is_empty());

        let file = load_str(
            "[pool]\nentrypoint = \"a.php\"\n[http]\nmiddleware = [\"static\"]\n[http.static]\nroot = \"/srv/pub\"\n",
        )
        .unwrap();
        let s = merge(file, Overrides::default(), Some(Path::new("/w"))).unwrap();
        let MiddlewareSettings::Static(st) = &s.http.middleware[0];
        assert_eq!(st.root, std::path::absolute("/srv/pub").unwrap());
    }

    /// The list activates middleware. Each configured section must be listed. Each listed name must be known, configured, and unique.
    #[test]
    fn http_middleware_list_validates() {
        for (toml, needle) in [
            (
                "[http]\nmiddleware = [\"staticc\"]\n",
                "\"staticc\" is unknown",
            ),
            (
                "[http]\nmiddleware = [\"static\"]\n",
                "[http.static] is missing",
            ),
            (
                "[http]\nmiddleware = [\"static\", \"static\"]\n[http.static]\nroot = \"p\"\n",
                "twice",
            ),
            ("[http.static]\nroot = \"p\"\n", "does not list \"static\""),
        ] {
            let file = load_str(&format!("[pool]\nentrypoint = \"a.php\"\n{toml}")).unwrap();
            let err = merge(file, Overrides::default(), Some(Path::new("/w")))
                .unwrap_err()
                .to_string();
            assert!(err.contains(needle), "{toml}: {err}");
        }
    }

    #[test]
    fn http_static_requires_root() {
        for toml in [
            "[pool]\nentrypoint = \"a.php\"\n[http.static]\n",
            "[pool]\nentrypoint = \"a.php\"\n[http.static]\nroot = \"\"\n",
        ] {
            let err = merge(
                load_str(toml).unwrap(),
                Overrides::default(),
                Some(Path::new("/w")),
            )
            .unwrap_err()
            .to_string();
            assert!(err.contains("http.static.root"), "{err}");
        }
    }

    /// The resolver validates only the format. The middleware constructor converts the value to lowercase.
    #[test]
    fn http_static_forbid_validates() {
        let file = load_str(
            "[pool]\nentrypoint = \"a.php\"\n[http]\nmiddleware = [\"static\"]\n[http.static]\nroot = \"p\"\nforbid = [\".PHP\", \".Phtml\"]\n",
        )
        .unwrap();
        let s = merge(file, Overrides::default(), Some(Path::new("/w"))).unwrap();
        let MiddlewareSettings::Static(st) = &s.http.middleware[0];
        assert_eq!(st.forbid, vec![".PHP".to_owned(), ".Phtml".to_owned()]);

        let file = load_str(
            "[pool]\nentrypoint = \"a.php\"\n[http]\nmiddleware = [\"static\"]\n[http.static]\nroot = \"p\"\nforbid = []\n",
        )
        .unwrap();
        let s = merge(file, Overrides::default(), Some(Path::new("/w"))).unwrap();
        let MiddlewareSettings::Static(st) = &s.http.middleware[0];
        assert!(st.forbid.is_empty());

        for entry in ["php", "", ".", ".php ", "./php"] {
            let file = load_str(&format!(
                "[pool]\nentrypoint = \"a.php\"\n[http.static]\nroot = \"p\"\nforbid = [\"{entry}\"]\n"
            ))
            .unwrap();
            let err = merge(file, Overrides::default(), Some(Path::new("/w")))
                .unwrap_err()
                .to_string();
            assert!(err.contains("http.static.forbid"), "{entry}: {err}");
        }
    }

    /// The filter string is assembled from these keys, so a key carrying filter syntax would inject directives (`"php=trace,tokio" = "debug"` reads as two).
    #[test]
    fn log_target_names_that_would_corrupt_the_filter_are_rejected() {
        for entry in [
            "\"\" = \"info\"",
            "\"php=trace,tokio\" = \"info\"",
            "\"a b\" = \"info\"",
            "\"a/b\" = \"info\"",
            "\"a\\u001Bb\" = \"info\"",
            "\"http[request]\" = \"info\"",
            "\".php\" = \"info\"",
        ] {
            let file = load_str(&format!(
                "[pool]\nentrypoint = \"a.php\"\n[log.targets]\n{entry}\n"
            ))
            .unwrap();
            assert!(
                merge(file, Overrides::default(), Some(Path::new("/w"))).is_err(),
                "{entry}"
            );
        }
    }
}
