//! Plays or pauses a specific program's media through Windows' media session API.
//! Unlike the play/pause key, which is a toggle sent to whichever program Windows picks,
//! this addresses one program and states the wanted result.
//! 通过 Windows 的媒体会话接口播放或暂停指定程序的媒体。
//! 播放/暂停键是开关，而且由 Windows 决定发给哪个程序；这里则是指定某个程序并明确要求结果。

use crate::win::BROWSERS;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

/// Returns whether a media session of the target was found and now is in the wanted state,
/// plus a description of all sessions for the diagnostic log.
/// `exe` is the target program; None means any known browser.
/// 返回是否找到了目标的媒体会话并已处于期望状态，以及供诊断日志使用的所有会话描述。
/// `exe` 是目标程序；None 表示任意已知浏览器。
pub fn command(play: bool, exe: Option<&str>) -> (bool, String) {
    let names: Vec<String> = match exe {
        Some(exe) => vec![exe.trim_end_matches(".exe").to_string()],
        None => BROWSERS.iter().map(|b| b.trim_end_matches(".exe").to_string()).collect(),
    };
    // The API waits for results, which must not happen on the window thread.
    // 这个接口需要等待结果，不能在窗口线程上进行。
    let worker = std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let result = run(play, &names).unwrap_or_else(|error| (false, format!("failed: {error}")));
        CoUninitialize();
        result
    });
    worker.join().unwrap_or((false, "worker panicked".into()))
}

fn run(play: bool, names: &[String]) -> windows::core::Result<(bool, String)> {
    let sessions = Manager::RequestAsync()?.get()?.GetSessions()?;
    let mut handled = false;
    let mut description = String::new();
    for session in sessions {
        let id = session.SourceAppUserModelId()?.to_string();
        let status = session.GetPlaybackInfo()?.PlaybackStatus()?;
        let is_target = names.iter().any(|name| id.to_lowercase().contains(name));
        let mut outcome = "";
        if is_target {
            let playing = status == Status::Playing;
            let ok = match (play, playing) {
                (true, true) | (false, false) => true,
                (true, false) => session.TryPlayAsync()?.get()?,
                (false, true) => session.TryPauseAsync()?.get()?,
            };
            handled |= ok;
            outcome = if ok { " -> ok" } else { " -> refused" };
        }
        description += &format!("{id}[status={}{outcome}] ", status.0);
    }
    Ok((handled, description))
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore]
    fn print_sessions() {
        // A session that matches nothing, so this only lists and changes no playback.
        // 用一个不会匹配任何会话的名字，所以只列出会话，不改变任何播放状态。
        println!("{:?}", super::command(false, Some("no-such-program.exe")));
    }
}
