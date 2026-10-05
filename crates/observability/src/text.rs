use std::fmt::Write as _;

use crate::stats::{PoolStats, STATES};

#[derive(Debug)]
pub struct Build {
    pub version: &'static str,
    pub php_version: String,
}

pub(crate) const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

type PoolFamily = (
    &'static str,
    &'static str,
    &'static str,
    fn(&PoolStats) -> u64,
);

/// Fixed pool names and version strings need no label escaping. https://prometheus.io/docs/instrumenting/exposition_formats/#prometheus-text-format
pub(crate) fn render(pools: &[PoolStats], build: &Build) -> String {
    let mut out = String::new();
    family(&mut out, "rapira_workers", "gauge", "Workers per state.");
    for pool in pools {
        for (&(_, state), &count) in STATES.iter().zip(&pool.states) {
            sample(
                &mut out,
                "rapira_workers",
                &[("pool", pool.name), ("state", state)],
                count,
            );
        }
    }
    let families: [PoolFamily; 6] = [
        (
            "rapira_workers_configured",
            "gauge",
            "The processes value of the pool config.",
            |p| p.configured as u64,
        ),
        (
            "rapira_requests_total",
            "counter",
            "Units of work that the workers finished in any way.",
            |p| p.requests,
        ),
        (
            "rapira_requests_failed_total",
            "counter",
            "Units of work that the host could not complete.",
            |p| p.failed,
        ),
        (
            "rapira_requests_failed_on_full_queue_total",
            "counter",
            "Units of work that found the worker queue full and never entered it.",
            |p| p.failed_on_full_queue,
        ),
        (
            "rapira_requests_queued",
            "gauge",
            "Units of work that wait for a PHP thread.",
            |p| p.queued,
        ),
        (
            "rapira_script_restarts_total",
            "counter",
            "Entrypoint restarts in interpreter threads.",
            |p| p.script_restarts,
        ),
    ];
    for (name, kind, help, value) in families {
        family(&mut out, name, kind, help);
        for pool in pools {
            sample(&mut out, name, &[("pool", pool.name)], value(pool));
        }
    }
    family(
        &mut out,
        "rapira_build_info",
        "gauge",
        "The versions of rapira and of the linked PHP.",
    );
    sample(
        &mut out,
        "rapira_build_info",
        &[
            ("version", build.version),
            ("php_version", &build.php_version),
        ],
        1,
    );
    out
}

fn family(out: &mut String, name: &str, kind: &str, help: &str) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} {kind}");
}

fn sample(out: &mut String, name: &str, labels: &[(&str, &str)], value: u64) {
    out.push_str(name);
    out.push('{');
    for (i, (key, val)) in labels.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(out, "{key}=\"{val}\"");
    }
    let _ = writeln!(out, "}} {value}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_each_family_as_one_group() {
        let pools = [
            PoolStats {
                name: "http",
                configured: 2,
                states: [0, 1, 1, 0],
                requests: 23,
                failed: 3,
                failed_on_full_queue: 4,
                queued: 2,
                script_restarts: 1,
            },
            PoolStats {
                name: "grpc",
                configured: 1,
                states: [1, 0, 0, 0],
                requests: 8,
                failed: 1,
                failed_on_full_queue: 0,
                queued: 6,
                script_restarts: 2,
            },
        ];
        let build = Build {
            version: "0.8.1",
            php_version: "8.5.10".into(),
        };
        let expected = r#"# HELP rapira_workers Workers per state.
# TYPE rapira_workers gauge
rapira_workers{pool="http",state="starting"} 0
rapira_workers{pool="http",state="idle"} 1
rapira_workers{pool="http",state="active"} 1
rapira_workers{pool="http",state="draining"} 0
rapira_workers{pool="grpc",state="starting"} 1
rapira_workers{pool="grpc",state="idle"} 0
rapira_workers{pool="grpc",state="active"} 0
rapira_workers{pool="grpc",state="draining"} 0
# HELP rapira_workers_configured The processes value of the pool config.
# TYPE rapira_workers_configured gauge
rapira_workers_configured{pool="http"} 2
rapira_workers_configured{pool="grpc"} 1
# HELP rapira_requests_total Units of work that the workers finished in any way.
# TYPE rapira_requests_total counter
rapira_requests_total{pool="http"} 23
rapira_requests_total{pool="grpc"} 8
# HELP rapira_requests_failed_total Units of work that the host could not complete.
# TYPE rapira_requests_failed_total counter
rapira_requests_failed_total{pool="http"} 3
rapira_requests_failed_total{pool="grpc"} 1
# HELP rapira_requests_failed_on_full_queue_total Units of work that found the worker queue full and never entered it.
# TYPE rapira_requests_failed_on_full_queue_total counter
rapira_requests_failed_on_full_queue_total{pool="http"} 4
rapira_requests_failed_on_full_queue_total{pool="grpc"} 0
# HELP rapira_requests_queued Units of work that wait for a PHP thread.
# TYPE rapira_requests_queued gauge
rapira_requests_queued{pool="http"} 2
rapira_requests_queued{pool="grpc"} 6
# HELP rapira_script_restarts_total Entrypoint restarts in interpreter threads.
# TYPE rapira_script_restarts_total counter
rapira_script_restarts_total{pool="http"} 1
rapira_script_restarts_total{pool="grpc"} 2
# HELP rapira_build_info The versions of rapira and of the linked PHP.
# TYPE rapira_build_info gauge
rapira_build_info{version="0.8.1",php_version="8.5.10"} 1
"#;
        assert_eq!(render(&pools, &build), expected);
    }
}
