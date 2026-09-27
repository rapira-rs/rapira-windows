use std::io;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows_sys::Win32::System::Console::{
    AllocConsole, CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent, GetConsoleProcessList,
    SetConsoleCtrlHandler,
};
use windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;
use windows_sys::core::BOOL;

const PROBE: &str = "RAPIRA_E2E_CONSOLE_PROBE";

pub fn prepare() {
    static READY: OnceLock<()> = OnceLock::new();
    READY.get_or_init(|| {
        let mut pid = 0;
        if unsafe { GetConsoleProcessList(&mut pid, 1) } == 0 {
            assert_ne!(
                unsafe { AllocConsole() },
                0,
                "AllocConsole: {}",
                io::Error::last_os_error()
            );
        }
        let dir = super::harness::scratch_dir();
        let marker = dir.join("console.ready");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "console::probe_child", "--nocapture"])
            .env(PROBE, &marker)
            .creation_flags(CREATE_NEW_PROCESS_GROUP)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let result = (|| {
            let deadline = Instant::now() + Duration::from_secs(15);
            while !marker.exists() {
                if child.try_wait()?.is_some() || Instant::now() >= deadline {
                    return Err(io::Error::other("console probe did not become ready"));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.id()) } == 0 {
                return Err(io::Error::last_os_error());
            }
            loop {
                if let Some(status) = child.try_wait()? {
                    return if status.success() {
                        Ok(())
                    } else {
                        Err(io::Error::other(format!("console probe: {status}")))
                    };
                }
                if Instant::now() >= deadline {
                    return Err(io::Error::other("Ctrl+Break did not reach the child"));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        })();
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(dir);
        result.expect("console delivery preflight");
    });
}

pub fn send_ctrl_break(pid: u32) {
    assert_ne!(
        unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid) },
        0,
        "Ctrl+Break to {pid}: {}",
        io::Error::last_os_error()
    );
}

#[test]
fn probe_child() {
    let Some(marker) = std::env::var_os(PROBE) else {
        return;
    };
    static SEEN: AtomicBool = AtomicBool::new(false);
    unsafe extern "system" fn handler(event: u32) -> BOOL {
        if event == CTRL_BREAK_EVENT {
            SEEN.store(true, Ordering::Release);
            1
        } else {
            0
        }
    }
    assert_ne!(unsafe { SetConsoleCtrlHandler(Some(handler), 1) }, 0);
    std::fs::write(marker, "ready").unwrap();
    let end = Instant::now() + Duration::from_secs(15);
    while !SEEN.load(Ordering::Acquire) {
        assert!(Instant::now() < end, "no console event");
        std::thread::sleep(Duration::from_millis(10));
    }
}
