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
