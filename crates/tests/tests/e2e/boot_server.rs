use rapira_sapi::Mode;
use serde_json::{Value, json};
use tests::wire::submit;
use tests::{drain, fixture, req};

use crate::harness::Spawn;

/// The boot run gets the process environment and entrypoint paths of the PHP CLI. The entrypoint overrides an environment variable with the same name.
/// https://github.com/php/php-src/blob/php-8.5.11/sapi/cli/php_cli.c#L316-L347
#[test]
fn boot_server_follows_the_cli() -> anyhow::Result<()> {
    struct Case {
        name: &'static str,
        mode: Mode,
        fixture: &'static str,
        env: &'static [(&'static str, &'static str)],
        ini: &'static str,
        argv: bool,
    }
    let cases = [
        Case {
            name: "worker mode",
            mode: Mode::Worker,
            fixture: "boot_server/worker.php",
            env: &[("BOOT_PROBE", "from-env")],
            ini: "",
            argv: true,
        },
        Case {
            name: "null $argv before the first $_SERVER read",
            mode: Mode::Worker,
            fixture: "boot_server/worker-argv-null.php",
            env: &[("BOOT_PROBE", "from-env")],
            ini: "",
            argv: true,
        },
        Case {
            name: "dispatcher mode",
            mode: Mode::Dispatcher,
            fixture: "boot_server/dispatcher.php",
            env: &[("BOOT_PROBE", "from-env")],
            ini: "",
            argv: true,
        },
        Case {
            name: "variables_order without E still imports the environment into $_SERVER",
            mode: Mode::Dispatcher,
            fixture: "boot_server/dispatcher.php",
            env: &[("BOOT_PROBE", "from-env")],
            ini: "variables_order = \"GPCS\"",
            argv: true,
        },
        Case {
            name: "the entrypoint overrides an environment variable with the same name",
            mode: Mode::Dispatcher,
            fixture: "boot_server/dispatcher.php",
            env: &[
                ("BOOT_PROBE", "from-env"),
                ("SCRIPT_FILENAME", "C:/from/env"),
            ],
            ini: "",
            argv: true,
        },
        Case {
            name: "register_argc_argv enabled",
            mode: Mode::Dispatcher,
            fixture: "boot_server/dispatcher.php",
            env: &[("BOOT_PROBE", "from-env")],
            ini: "register_argc_argv = 1",
            argv: true,
        },
        Case {
            name: "register_argc_argv disabled",
            mode: Mode::Dispatcher,
            fixture: "boot_server/dispatcher.php",
            env: &[("BOOT_PROBE", "from-env")],
            ini: "register_argc_argv = 0",
            argv: true,
        },
    ];

    let shared = std::fs::read_to_string(fixture("ini/shared/php.ini"))?;
    for case in &cases {
        let mut spawn = Spawn::http(case.mode, fixture(case.fixture))
            .php_ini(&format!("{shared}\n{}\n", case.ini));
        for &(key, value) in case.env {
            spawn = spawn.env(key, value);
        }
        let srv = spawn.spawn();
        let name = case.fixture.rsplit('/').next().expect("file name");
        let entrypoint = std::path::absolute(srv.dir.join("http").join(name))?
            .display()
            .to_string();
        let want = json!({
            "BOOT_PROBE": "from-env",
            "PHP_SELF": entrypoint,
            "SCRIPT_NAME": entrypoint,
            "SCRIPT_FILENAME": entrypoint,
            "PATH_TRANSLATED": entrypoint,
            "DOCUMENT_ROOT": "",
            "argv": case.argv.then(|| vec![&entrypoint]),
            "argc": case.argv.then_some(1),
        });
        for _ in 0..2 {
            let (status, body) = drain(submit(srv.addr, req("/"))?);
            assert_eq!(status, 200, "{}: {body}", case.name);
            let got: Value = serde_json::from_str(&body)?;
            assert_eq!(got, want, "{}", case.name);
        }
    }
    Ok(())
}
