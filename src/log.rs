//! An optional diagnostic log, written only when started with `--log`, at %LOCALAPPDATA%\\CSTool\\debug.log.
//! It records switching steps only, never keystrokes or window contents.
//! 可选的诊断日志，仅在以 `--log` 启动时写入，位于 %LOCALAPPDATA%\\CSTool\\debug.log 的小型诊断日志。
//! 只记录切屏步骤，绝不记录按键或窗口内容。

use std::io::Write;
use windows::Win32::System::SystemInformation::GetLocalTime;

const MAX_SIZE: u64 = 512 * 1024;

/// True if the program was started with `--log`. 程序是否以 `--log` 启动。
pub fn enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::args().any(|argument| argument == "--log"))
}

pub fn write(message: &str) {
    if !enabled() {
        return;
    }
    let path = crate::config::Config::path().with_file_name("debug.log");
    // Start over when the file grows large. 文件变大后重新开始。
    let fresh = std::fs::metadata(&path).map(|meta| meta.len() > MAX_SIZE).unwrap_or(false);
    let file = std::fs::OpenOptions::new().create(true).append(!fresh).write(true).truncate(fresh).open(&path);
    if let Ok(mut file) = file {
        let now = unsafe { GetLocalTime() };
        let _ = writeln!(file, "{:02}:{:02}:{:02}.{:03} {message}", now.wHour, now.wMinute, now.wSecond, now.wMilliseconds);
    }
}
