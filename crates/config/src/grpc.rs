use anyhow::bail;
use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::listen::resolve_listen;
use crate::pool::{PoolSection, resolve_named_pool};
use crate::{Listen, PoolSettings, RunMode, config_relative};

#[derive(Debug)]
pub struct GrpcSettings {
    pub listen: Listen,
    pub protos: Vec<PathBuf>,
    pub import_paths: Vec<PathBuf>,
    pub reflection: bool,
    pub gzip_responses: bool,
    pub max_request_message_size: usize,
    pub max_response_message_size: usize,
    pub pool: PoolSettings,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GrpcSection {
    listen: Option<String>,
    #[serde(default)]
    protos: Vec<String>,
    #[serde(default)]
    import_paths: Vec<String>,
    reflection: Option<bool>,
    #[serde(default)]
    interceptors: Vec<String>,
    #[serde(default)]
    compression: CompressionSection,
    max_request_message_size_mb: Option<usize>,
    max_response_message_size_mb: Option<usize>,
    #[serde(default)]
    pool: PoolSection,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompressionSection {
    #[serde(default)]
    gzip: GzipSection,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct GzipSection {
    enabled: Option<bool>,
}

pub(crate) fn resolve_grpc(
    section: GrpcSection,
    config_dir: Option<&Path>,
) -> anyhow::Result<GrpcSettings> {
    let listen = resolve_listen(section.listen.as_deref(), "grpc", 9001)?;
    if section.protos.is_empty() {
        bail!("grpc.protos must contain at least one directory");
    }
    let protos = resolve_paths(section.protos, "protos", config_dir)?;
    let import_paths = resolve_paths(section.import_paths, "import_paths", config_dir)?;
    let max_request_message_size = message_size(
        section.max_request_message_size_mb,
        "max_request_message_size_mb",
    )?;
    let max_response_message_size = message_size(
        section.max_response_message_size_mb,
        "max_response_message_size_mb",
    )?;
    if let Some(name) = section.interceptors.first() {
        match name.as_str() {
            "static" => bail!("grpc.interceptors entry \"static\" is not supported for gRPC"),
            _ => bail!("grpc.interceptors entry \"{name}\" is unknown"),
        }
    }
    let pool = resolve_named_pool(
        section.pool,
        &crate::Overrides::default(),
        config_dir,
        "grpc.pool",
    )?;
    if pool.mode != RunMode::Dispatcher {
        bail!("grpc.pool.mode must be \"dispatcher\"");
    }
    Ok(GrpcSettings {
        listen,
        protos,
        import_paths,
        reflection: section.reflection.unwrap_or(true),
        gzip_responses: section.compression.gzip.enabled.unwrap_or(false),
        max_request_message_size,
        max_response_message_size,
        pool,
    })
}

fn resolve_paths(
    paths: Vec<String>,
    key: &str,
    config_dir: Option<&Path>,
) -> anyhow::Result<Vec<PathBuf>> {
    paths
        .into_iter()
        .map(|path| {
            if path.is_empty() {
                bail!("grpc.{key} entries must be nonempty directory paths");
            }
            Ok(config_relative(config_dir, &path)?)
        })
        .collect()
}

fn message_size(value: Option<usize>, key: &str) -> anyhow::Result<usize> {
    let mb = value.unwrap_or(4);
    if mb == 0 {
        bail!("grpc.{key} must be at least 1");
    }
    mb.checked_mul(1024 * 1024)
        .ok_or_else(|| anyhow::anyhow!("grpc.{key} {mb} is too large"))
}

#[cfg(test)]
mod tests {
    use crate::*;
    fn parse(text: &str) -> anyhow::Result<FileConfig> {
        Ok(toml::from_str(text)?)
    }
    #[test]
    fn grpc_configuration_validation() {
        struct Case {
            name: &'static str,
            text: &'static str,
            error: Option<&'static str>,
        }
        let cases = [
            Case {
                name: "grpc_only_dispatcher",
                text: "[grpc]\nprotos = [\"proto\"]\n[grpc.pool]\nentrypoint = \"grpc.php\"\nmode = \"dispatcher\"\n",
                error: None,
            },
            Case {
                name: "both_protocols",
                text: "[pool]\nentrypoint = \"http.php\"\n[grpc]\nprotos = [\"proto\"]\n[grpc.pool]\nentrypoint = \"grpc.php\"\n",
                error: None,
            },
            Case {
                name: "explicit_true",
                text: "[grpc]\nprotos = [\"proto\"]\n[grpc.compression.gzip]\nenabled = true\n[grpc.pool]\nentrypoint = \"grpc.php\"\n",
                error: None,
            },
            Case {
                name: "explicit_false",
                text: "[grpc]\nprotos = [\"proto\"]\n[grpc.compression.gzip]\nenabled = false\n[grpc.pool]\nentrypoint = \"grpc.php\"\n",
                error: None,
            },
            Case {
                name: "invalid_string",
                text: "[grpc]\nprotos = [\"proto\"]\n[grpc.compression.gzip]\nenabled = \"true\"\n[grpc.pool]\nentrypoint = \"grpc.php\"\n",
                error: Some("expected a boolean"),
            },
            Case {
                name: "unknown_compression_key",
                text: "[grpc]\nprotos = [\"proto\"]\n[grpc.compression]\nzstd = {}\n[grpc.pool]\nentrypoint = \"grpc.php\"\n",
                error: Some("unknown field `zstd`"),
            },
            Case {
                name: "unknown_gzip_key",
                text: "[grpc]\nprotos = [\"proto\"]\n[grpc.compression.gzip]\nenabld = true\n[grpc.pool]\nentrypoint = \"grpc.php\"\n",
                error: Some("unknown field `enabld`"),
            },
            Case {
                name: "empty_configuration",
                text: "",
                error: Some("entrypoint"),
            },
            Case {
                name: "missing_protos",
                text: "[grpc.pool]\nentrypoint = \"grpc.php\"\n",
                error: Some("grpc.protos"),
            },
            Case {
                name: "empty_protos",
                text: "[grpc]\nprotos = []\n[grpc.pool]\nentrypoint = \"grpc.php\"\n",
                error: Some("grpc.protos"),
            },
            Case {
                name: "empty_proto_path",
                text: "[grpc]\nprotos = [\"\"]\n[grpc.pool]\nentrypoint = \"grpc.php\"\n",
                error: Some("grpc.protos"),
            },
            Case {
                name: "empty_import_path",
                text: "[grpc]\nprotos = [\"proto\"]\nimport_paths = [\"\"]\n[grpc.pool]\nentrypoint = \"grpc.php\"\n",
                error: Some("grpc.import_paths"),
            },
            Case {
                name: "worker_mode",
                text: "[grpc]\nprotos = [\"proto\"]\n[grpc.pool]\nentrypoint = \"grpc.php\"\nmode = \"worker\"\n",
                error: Some("grpc.pool.mode must be \"dispatcher\""),
            },
            Case {
                name: "missing_entrypoint",
                text: "[grpc]\nprotos = [\"proto\"]\n",
                error: Some("grpc.pool.entrypoint"),
            },
            Case {
                name: "invalid_pool_size",
                text: "[grpc]\nprotos = [\"proto\"]\n[grpc.pool]\nentrypoint = \"grpc.php\"\nprocesses = 0\n",
                error: Some("grpc.pool.processes"),
            },
            Case {
                name: "zero_request_limit",
                text: "[grpc]\nprotos = [\"proto\"]\nmax_request_message_size_mb = 0\n[grpc.pool]\nentrypoint = \"grpc.php\"\n",
                error: Some("grpc.max_request_message_size_mb must be at least 1"),
            },
            Case {
                name: "zero_response_limit",
                text: "[grpc]\nprotos = [\"proto\"]\nmax_response_message_size_mb = 0\n[grpc.pool]\nentrypoint = \"grpc.php\"\n",
                error: Some("grpc.max_response_message_size_mb must be at least 1"),
            },
            Case {
                name: "request_limit_overflow",
                text: "[grpc]\nprotos = [\"proto\"]\nmax_request_message_size_mb = 17592186044416\n[grpc.pool]\nentrypoint = \"grpc.php\"\n",
                error: Some("grpc.max_request_message_size_mb 17592186044416 is too large"),
            },
            Case {
                name: "response_limit_overflow",
                text: "[grpc]\nprotos = [\"proto\"]\nmax_response_message_size_mb = 17592186044416\n[grpc.pool]\nentrypoint = \"grpc.php\"\n",
                error: Some("grpc.max_response_message_size_mb 17592186044416 is too large"),
            },
            Case {
                name: "static_interceptor",
                text: "[grpc]\nprotos = [\"proto\"]\ninterceptors = [\"static\"]\n[grpc.pool]\nentrypoint = \"grpc.php\"\n",
                error: Some("static\" is not supported for gRPC"),
            },
            Case {
                name: "unknown_interceptor",
                text: "[grpc]\nprotos = [\"proto\"]\ninterceptors = [\"auth\"]\n[grpc.pool]\nentrypoint = \"grpc.php\"\n",
                error: Some("grpc.interceptors entry \"auth\" is unknown"),
            },
            Case {
                name: "invalid_listener",
                text: "[grpc]\nlisten = \"localhost:9001\"\nprotos = [\"proto\"]\n[grpc.pool]\nentrypoint = \"grpc.php\"\n",
                error: Some("invalid grpc.listen"),
            },
            Case {
                name: "unknown_grpc_key",
                text: "[grpc]\nprotos = [\"proto\"]\nmax_message_size_mb = 4\n[grpc.pool]\nentrypoint = \"grpc.php\"\n",
                error: Some("unknown field `max_message_size_mb`"),
            },
        ];
        for case in cases {
            let result = parse(case.text)
                .and_then(|file| merge(file, Overrides::default(), Some(Path::new("/srv/app"))));
            match case.error {
                None => assert!(result.is_ok(), "{}: {result:?}", case.name),
                Some(expected) => {
                    let error = result.expect_err(case.name).to_string();
                    assert!(error.contains(expected), "{}: {error}", case.name);
                }
            }
        }
    }

    #[test]
    fn grpc_response_compression_resolves_nested_settings() {
        struct Case {
            name: &'static str,
            compression: &'static str,
            gzip_responses: bool,
        }
        let cases = [
            Case {
                name: "compression_omitted",
                compression: "",
                gzip_responses: false,
            },
            Case {
                name: "empty_compression_table",
                compression: "[grpc.compression]\n",
                gzip_responses: false,
            },
            Case {
                name: "empty_gzip_table",
                compression: "[grpc.compression.gzip]\n",
                gzip_responses: false,
            },
            Case {
                name: "explicit_true",
                compression: "[grpc.compression.gzip]\nenabled = true\n",
                gzip_responses: true,
            },
            Case {
                name: "explicit_false",
                compression: "[grpc.compression.gzip]\nenabled = false\n",
                gzip_responses: false,
            },
        ];
        for case in cases {
            let text = format!(
                "[grpc]\nprotos = [\"proto\"]\n{}[grpc.pool]\nentrypoint = \"grpc.php\"\n",
                case.compression
            );
            let settings = parse(&text)
                .and_then(|file| merge(file, Overrides::default(), Some(Path::new("/srv/app"))))
                .expect(case.name);
            assert_eq!(
                settings.grpc.unwrap().gzip_responses,
                case.gzip_responses,
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn windows_paths_limits_and_protocol_pools() {
        let file = parse(
            r#"
            [pool]
            entrypoint = 'http.php'
            processes = 2
            mode = 'worker'
            [http]
            listen = ':8002'
            [grpc]
            listen = '[::1]:9002'
            protos = ['proto', 'schema\nested']
            import_paths = ['vendor']
            max_request_message_size_mb = 2
            max_response_message_size_mb = 7
            reflection = false
            [grpc.pool]
            entrypoint = 'grpc.php'
            processes = 3
            max_requests = 50
        "#,
        )
        .unwrap();
        let root = Path::new(r"C:\app");
        let settings = merge(
            file,
            Overrides {
                processes: Some(4),
                ..Default::default()
            },
            Some(root),
        )
        .unwrap();
        assert_eq!(settings.http.unwrap().listen.to_string(), "0.0.0.0:8002");
        let http = settings.pool.unwrap();
        assert_eq!(http.processes, 4);
        assert_eq!(http.mode, RunMode::Worker);
        let grpc = settings.grpc.unwrap();
        assert_eq!(grpc.listen.to_string(), "[::1]:9002");
        assert_eq!(
            grpc.protos,
            [root.join("proto"), root.join(r"schema\nested")]
        );
        assert_eq!(grpc.import_paths, [root.join("vendor")]);
        assert_eq!(grpc.pool.entrypoint, root.join("grpc.php"));
        assert_eq!(grpc.pool.processes, 3);
        assert_eq!(grpc.pool.max_requests, 50);
        assert_eq!(grpc.pool.mode, RunMode::Dispatcher);
        assert_eq!(grpc.max_request_message_size, 2_097_152);
        assert_eq!(grpc.max_response_message_size, 7_340_032);
        assert!(!grpc.reflection);
    }

    #[test]
    fn grpc_only_uses_no_http_resources_and_rejects_unix_listeners() {
        let text = "[grpc]\nprotos = ['proto']\n[grpc.pool]\nentrypoint = 'grpc.php'\n";
        let settings = merge(parse(text).unwrap(), Overrides::default(), None).unwrap();
        assert!(settings.http.is_none());
        assert!(settings.pool.is_none());
        let grpc = settings.grpc.unwrap();
        assert_eq!(grpc.listen.to_string(), "127.0.0.1:9001");
        assert_eq!(grpc.max_request_message_size, 4_194_304);
        assert_eq!(grpc.max_response_message_size, 4_194_304);
        assert!(grpc.reflection);
        for extra in [
            "listen = 'unix:grpc.sock'",
            "interceptors = ['static']",
            "interceptors = ['unknown']",
        ] {
            let text = text.replace("[grpc]", &format!("[grpc]\n{extra}"));
            assert!(merge(parse(&text).unwrap(), Overrides::default(), None).is_err());
        }
    }
}
