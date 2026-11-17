//! Detects whether a browser (or the launched program) is really producing sound.
//! 检测浏览器（或启动的程序）是否真的在输出声音。

use crate::win::{file_name, process_path, BROWSERS};
use windows::core::Interface;
use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, MMDeviceEnumerator,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

pub fn is_audio_playing(app_exe: Option<&str>) -> bool {
    unsafe { check(app_exe).unwrap_or(false) }
}

unsafe fn check(app_exe: Option<&str>) -> windows::core::Result<bool> {
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?;
    let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
    let sessions = manager.GetSessionEnumerator()?;
    for index in 0..sessions.GetCount()? {
        let Ok(session) = sessions.GetSession(index) else { continue };
        let Ok(control) = session.cast::<IAudioSessionControl2>() else { continue };
        let Some(path) = control.GetProcessId().ok().and_then(process_path) else { continue };
        let name = file_name(&path);
        // Only browsers and the chosen program count; chat apps and system sounds are ignored.
        // 只统计浏览器和所选程序；聊天软件和系统提示音不算。
        if !BROWSERS.contains(&name.as_str()) && app_exe != Some(name.as_str()) {
            continue;
        }
        let peak = session.cast::<IAudioMeterInformation>().and_then(|meter| meter.GetPeakValue()).unwrap_or(0.0);
        if peak > 0.001 {
            return Ok(true);
        }
    }
    Ok(false)
}

/// One line listing every audio session with its state and loudness, for the diagnostic log.
/// 列出每个音频会话及其状态和音量的一行文字，供诊断日志使用。
pub fn describe_sessions() -> String {
    unsafe fn list() -> windows::core::Result<String> {
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?;
        let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
        let sessions = manager.GetSessionEnumerator()?;
        let mut line = String::new();
        for index in 0..sessions.GetCount()? {
            let Ok(session) = sessions.GetSession(index) else { continue };
            let Ok(control) = session.cast::<IAudioSessionControl2>() else { continue };
            let Some(path) = control.GetProcessId().ok().and_then(process_path) else { continue };
            let peak = session.cast::<IAudioMeterInformation>().and_then(|meter| meter.GetPeakValue()).unwrap_or(-1.0);
            let state = session.GetState().map(|state| state.0).unwrap_or(-1);
            line += &format!("{}[state={state} peak={peak:.3}] ", file_name(&path));
        }
        Ok(line)
    }
    unsafe { list().unwrap_or_else(|error| format!("failed: {error}")) }
}
