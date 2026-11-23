//! Plays or pauses a specific program's media through Windows' media session API.
//! Unlike the play/pause key, which is a toggle sent to whichever program Windows picks,
//! this addresses one program and states the wanted result.
//! 通过 Windows 的媒体会话接口播放或暂停指定程序的媒体。
//! 播放/暂停键是开关，而且由 Windows 决定发给哪个程序；这里则是指定某个程序并明确要求结果。

use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

/// Returns whether a media session of the target was found and now is in the wanted state,
/// plus a description of all sessions for the diagnostic log.
/// `exe` is the target program; None selects no session.
/// 返回是否找到了目标的媒体会话并已处于期望状态，以及供诊断日志使用的所有会话描述。
/// `exe` 是目标程序；None 表示不选择会话。
pub fn command(play: bool, exe: Option<&str>) -> (bool, String) {
    let target = exe.map(str::to_lowercase);
    // The API waits for results, which must not happen on the window thread.
    // 这个接口需要等待结果，不能在窗口线程上进行。
    let worker = std::thread::spawn(move || unsafe {
        let com = CoInitializeEx(None, COINIT_MULTITHREADED);
        let result = run(play, target.as_deref()).unwrap_or_else(|error| (false, format!("failed: {error}")));
        if com.is_ok() {
            CoUninitialize();
        }
        result
    });
    worker.join().unwrap_or((false, "worker panicked".into()))
}

fn matches_app(id: &str, exe: &str) -> bool {
    let id = id.to_ascii_lowercase();
    let exe = exe.to_ascii_lowercase();
    let stem = exe.strip_suffix(".exe").unwrap_or(&exe);
    let tail = id.rsplit(['\\', '/']).next().unwrap_or(&id);
    let tail_stem = tail.strip_suffix(".exe").unwrap_or(tail);
    if tail == exe || tail_stem == stem {
        return true;
    }
    // Packaged and browser AUMIDs look like "MSEdge.UserData" or "Vendor.AppName_hash!App".
    // 打包应用和浏览器的 AUMID 形如 "MSEdge.UserData" 或 "Vendor.AppName_hash!App"。
    let base = tail.split('!').next().unwrap_or(tail);
    let base = base.split('_').next().unwrap_or(base);
    base.split('.').any(|part| part == stem || (stem == "msedge" && part == "microsoftedge"))
}

fn run(play: bool, target: Option<&str>) -> windows::core::Result<(bool, String)> {
    let sessions = Manager::RequestAsync()?.get()?.GetSessions()?;
    let mut selected = None;
    let mut description = String::new();
    for session in sessions {
        let Ok(id) = session.SourceAppUserModelId().map(|s| s.to_string()) else { continue };
        let Ok(status) = session.GetPlaybackInfo().and_then(|info| info.PlaybackStatus()) else { continue };
        description += &format!("{id}[status={}] ", status.0);
        if target.is_some_and(|exe| matches_app(&id, exe)) {
            if selected.is_some() {
                return Ok((true, format!("{description}ambiguous sessions; skipped")));
            }
            selected = Some((session, status));
        }
    }
    let Some((session, status)) = selected else { return Ok((false, description)) };
    let handled = match (play, status == Status::Playing) {
        (true, false) => session.TryPlayAsync()?.get()?,
        (false, true) => session.TryPauseAsync()?.get()?,
        _ => true,
    };
    Ok((handled, description))
}

#[cfg(test)]
mod tests {
    use super::matches_app;

    #[test]
    fn matches_exe_names_and_browser_aumids() {
        assert!(matches_app("msedge.exe", "msedge.exe"));
        assert!(matches_app("MSEdge", "msedge.exe"));
        assert!(matches_app("MSEdge.UserData", "msedge.exe"));
        assert!(matches_app("Microsoft.MicrosoftEdge.Stable_8wekyb3d8bbwe!App", "msedge.exe"));
        assert!(matches_app("Chrome", "chrome.exe"));
        assert!(matches_app("C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe", "chrome.exe"));
        assert!(!matches_app("308046B0AF4A39CB", "firefox.exe"));
        assert!(!matches_app("obs-browser.exe", "browser.exe"));
        assert!(!matches_app("whatsapp.exe", "app.exe"));
    }

    #[test]
    #[ignore]
    fn print_sessions() {
        // A session that matches nothing, so this only lists and changes no playback.
        // 用一个不会匹配任何会话的名字，所以只列出会话，不改变任何播放状态。
        println!("{:?}", super::command(false, Some("no-such-program.exe")));
    }
}
