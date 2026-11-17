//! Asks GitHub for the newest CSTool release. This is the only place the program
//! contacts the internet, and it can be switched off from the tray menu.
//! 向 GitHub 查询 CSTool 的最新版本。这是程序唯一会联网的地方，可以在托盘菜单中关闭。

use serde_json::Value;
use std::ffi::c_void;
use windows::core::{w, PCWSTR};
use windows::Win32::Networking::WinHttp::*;

/// The page opened for downloading. It is fixed here and never taken from the network reply.
/// 下载时打开的页面。它固定写在这里，绝不取自网络返回的内容。
pub const RELEASES_URL: &str = "https://github.com/Chraining/CSTool/releases/latest";
const MAX_REPLY: usize = 256 * 1024;

fn parse_version(text: &str) -> Option<Vec<u32>> {
    text.trim().trim_start_matches('v').split('.').map(|part| part.parse().ok()).collect()
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

/// Returns the tag of the newest release if it is newer than this build.
/// 如果最新发布的版本比当前程序新，返回它的标签。
pub fn newer_release() -> Option<String> {
    let reply = unsafe { fetch() }?;
    let value: Value = serde_json::from_slice(&reply).ok()?;
    let tag = value["tag_name"].as_str()?;
    let plain = tag.len() <= 20 && tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '.');
    (plain && is_newer(tag, env!("CARGO_PKG_VERSION"))).then(|| tag.to_string())
}

/// Closes a WinHTTP handle when dropped. 离开作用域时关闭 WinHTTP 句柄。
struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

unsafe fn fetch() -> Option<Vec<u8>> {
    let session = Handle(WinHttpOpen(w!("CSTool"), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0));
    if session.0.is_null() {
        return None;
    }
    WinHttpSetTimeouts(session.0, 5000, 5000, 5000, 5000).ok()?;
    let connection = Handle(WinHttpConnect(session.0, w!("api.github.com"), 443, 0));
    if connection.0.is_null() {
        return None;
    }
    let request = Handle(WinHttpOpenRequest(
        connection.0,
        w!("GET"),
        w!("/repos/Chraining/CSTool/releases/latest"),
        PCWSTR::null(),
        PCWSTR::null(),
        std::ptr::null(),
        WINHTTP_FLAG_SECURE,
    ));
    if request.0.is_null() {
        return None;
    }
    WinHttpSendRequest(request.0, None, None, 0, 0, 0).ok()?;
    WinHttpReceiveResponse(request.0, std::ptr::null_mut()).ok()?;
    let mut reply = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let mut read = 0u32;
        WinHttpReadData(request.0, chunk.as_mut_ptr() as *mut c_void, chunk.len() as u32, &mut read).ok()?;
        if read == 0 {
            return Some(reply);
        }
        reply.extend_from_slice(&chunk[..read as usize]);
        if reply.len() > MAX_REPLY {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn compares_versions_by_number() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("v0.10.0", "0.9.3"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.2.0"));
        assert!(!is_newer("nightly", "0.1.0"));
    }
}
