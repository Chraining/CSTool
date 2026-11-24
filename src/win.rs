//! Windows helpers: finding, switching, minimizing and closing windows and processes.
//! Windows 辅助函数：查找、切换、最小化和关闭窗口与进程。

use crate::config::WindowSel;
use std::path::Path;
use windows::core::{Interface, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, WPARAM};
use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_ALL, STGM_READ};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW,
    TerminateProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP};
use windows::Win32::UI::Shell::{IShellLinkW, ShellExecuteExW, ShellLink, SEE_MASK_FLAG_NO_UI, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const BROWSERS: [&str; 10] = [
    "msedge.exe", "chrome.exe", "firefox.exe", "360se.exe", "iexplore.exe",
    "sogouexplorer.exe", "browser.exe", "yandex.exe", "opera.exe", "brave.exe",
];
/// Browsers the "close" action terminates, as in the original.
/// “关闭”动作会终止的浏览器，与原版一致。
const KILL_BROWSERS: [&str; 6] = ["msedge.exe", "chrome.exe", "firefox.exe", "360se.exe", "iexplore.exe", "sogouexplorer.exe"];

pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn from_wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

pub fn file_name(path: &str) -> String {
    Path::new(path).file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()
}

pub fn process_path(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = vec![0u16; 32768];
        let mut size = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut size);
        let _ = CloseHandle(handle);
        result.ok().map(|_| String::from_utf16_lossy(&buffer[..size as usize]))
    }
}

fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

fn window_title(hwnd: HWND) -> String {
    unsafe {
        let length = GetWindowTextLengthW(hwnd);
        if length <= 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; length as usize + 1];
        let copied = GetWindowTextW(hwnd, &mut buffer);
        String::from_utf16_lossy(&buffer[..copied.max(0) as usize])
    }
}

/// Calls `visit` for every top-level window until it returns false.
/// 对每个顶层窗口调用 `visit`，直到它返回 false。
fn each_window(mut visit: impl FnMut(HWND) -> bool) -> bool {
    unsafe extern "system" fn thunk(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let visit = &mut *(lparam.0 as *mut &mut dyn FnMut(HWND) -> bool);
        BOOL::from(visit(hwnd))
    }
    let mut object: &mut dyn FnMut(HWND) -> bool = &mut visit;
    // EnumWindows also reports failure when the callback stops early, so the result is not an error signal here.
    // 回调提前停止时 EnumWindows 也会返回失败，所以这里不能把返回值当作错误信号。
    unsafe { EnumWindows(Some(thunk), LPARAM(&mut object as *mut _ as isize)).is_ok() }
}

/// The visible top-level window owned by cs2.exe; the title depends on the Steam language.
/// cs2.exe 拥有的可见顶层窗口；窗口标题会随 Steam 语言变化，所以按进程查找。
pub fn find_cs2_window() -> Option<HWND> {
    let mut found = None;
    each_window(|hwnd| unsafe {
        if IsWindowVisible(hwnd).as_bool()
            && GetWindowTextLengthW(hwnd) > 0
            && process_path(window_pid(hwnd)).is_some_and(|p| file_name(&p) == "cs2.exe")
        {
            found = Some(hwnd);
            return false;
        }
        true
    });
    found
}

/// Open application windows the user can choose from (excluding CS2 and this program).
/// 用户可以选择的已打开应用窗口（不含 CS2 和本程序）。
pub fn list_windows() -> Vec<WindowSel> {
    let own = unsafe { GetCurrentProcessId() };
    let mut windows = Vec::new();
    each_window(|hwnd| {
        let title = if unsafe { IsWindowVisible(hwnd).as_bool() } { window_title(hwnd) } else { String::new() };
        let pid = window_pid(hwnd);
        if title.is_empty() || pid == own {
            return true;
        }
        if let Some(path) = process_path(pid) {
            if file_name(&path) != "cs2.exe" {
                windows.push(WindowSel { hwnd: hwnd.0 as isize, pid, path, title });
            }
        }
        true
    });
    windows
}

/// Finds the selected window again; with `rebind`, falls back to the same program's only matching window.
/// 重新找到所选窗口；`rebind` 为真时，可退而匹配同一程序唯一符合的窗口。
pub fn resolve_window(sel: &WindowSel, rebind: bool) -> Option<WindowSel> {
    pick_window(&list_windows(), sel, rebind).cloned()
}

pub fn pick_window<'a>(windows: &'a [WindowSel], sel: &WindowSel, rebind: bool) -> Option<&'a WindowSel> {
    let matches: Vec<&WindowSel> = windows.iter().filter(|w| w.path.to_lowercase() == sel.path.to_lowercase()).collect();
    if let Some(exact) = matches.iter().find(|w| w.hwnd == sel.hwnd && w.pid == sel.pid) {
        return Some(exact);
    }
    if !rebind {
        return None;
    }
    let titled: Vec<&WindowSel> = matches.iter().filter(|w| w.title == sel.title).copied().collect();
    if titled.len() == 1 {
        return Some(titled[0]);
    }
    (matches.len() == 1).then(|| matches[0])
}

fn hwnd_of(window: &WindowSel) -> HWND {
    HWND(window.hwnd as *mut _)
}

pub fn activate_window(window: &WindowSel) -> bool {
    let hwnd = hwnd_of(window);
    unsafe { bring_to_front(hwnd, false) }
}

/// Restores a minimized window and keeps asserting foreground focus briefly so CS2's
/// asynchronous minimize cannot steal focus back.
/// 恢复已最小化的窗口，并在短时间内持续置前，防止 CS2 异步最小化把焦点抢回。
unsafe fn bring_to_front(hwnd: HWND, accept_same_process: bool) -> bool {
    if IsIconic(hwnd).as_bool() {
        let mut placement = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
        let _ = GetWindowPlacement(hwnd, &mut placement);
        let show = if placement.flags.0 & WPF_RESTORETOMAXIMIZED.0 != 0 { SW_SHOWMAXIMIZED } else { SW_RESTORE };
        let _ = ShowWindow(hwnd, show);
    }
    let target_pid = window_pid(hwnd);
    let in_front = || {
        let fg = GetForegroundWindow();
        fg == hwnd || (accept_same_process && !fg.0.is_null() && target_pid != 0 && window_pid(fg) == target_pid)
    };
    force_foreground(hwnd);
    let mut stable = 0;
    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_millis(30));
        if in_front() {
            stable += 1;
            if stable >= 4 {
                break;
            }
        } else {
            stable = 0;
            force_foreground(hwnd);
        }
    }
    in_front()
}

/// Brings a window forward even when another process owns the foreground.
/// 即使前台属于其他进程，也把窗口调到前台。
unsafe fn force_foreground(hwnd: HWND) {
    let current = GetCurrentThreadId();
    let foreground = GetForegroundWindow();
    let foreground_thread = if foreground.0.is_null() { 0 } else { GetWindowThreadProcessId(foreground, None) };
    let attach = foreground_thread != 0 && foreground_thread != current;
    if attach {
        let _ = AttachThreadInput(current, foreground_thread, true);
    }
    let _ = SetForegroundWindow(hwnd);
    if attach {
        let _ = AttachThreadInput(current, foreground_thread, false);
    }
}

pub fn close_window(window: &WindowSel) {
    unsafe {
        let _ = PostMessageW(hwnd_of(window), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

pub fn minimize_cs2() {
    let Some(hwnd) = find_cs2_window() else { return };
    unsafe {
        // Release the cursor lock first; needed in exclusive fullscreen.
        // 先解除鼠标锁定；独占全屏下需要这样做。
        let _ = ClipCursor(None);
        let _ = PostMessageW(hwnd, WM_SYSCOMMAND, WPARAM(SC_MINIMIZE as usize), LPARAM(0));
        let _ = ShowWindow(hwnd, SW_SHOWMINNOACTIVE);
    }
}

pub fn restore_cs2() {
    let Some(hwnd) = find_cs2_window() else { return };
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let mut placement = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
            let _ = GetWindowPlacement(hwnd, &mut placement);
            let show = if placement.flags.0 & WPF_RESTORETOMAXIMIZED.0 != 0 { SW_SHOWMAXIMIZED } else { SW_RESTORE };
            let _ = ShowWindow(hwnd, show);
        }
        force_foreground(hwnd);
        if GetForegroundWindow() != hwnd {
            // Windows may refuse to hand over the foreground; a simulated Alt press lifts that restriction.
            // Windows 可能拒绝让出前台；模拟按一次 Alt 键可以解除这个限制。
            keybd_event(0x12, 0, KEYBD_EVENT_FLAGS(0), 0);
            keybd_event(0x12, 0, KEYEVENTF_KEYUP, 0);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

/// File name of the program that owns the foreground window, for the diagnostic log.
/// 拥有前台窗口的程序的文件名，供诊断日志使用。
pub fn foreground_exe() -> String {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return "none".into();
    }
    process_path(window_pid(hwnd)).map(|path| file_name(&path)).unwrap_or_else(|| "unknown".into())
}

/// Presses the Play/Pause media key.
/// 按下“播放/暂停”媒体键。
pub fn send_media_key() {
    unsafe {
        keybd_event(0xB3, 0, KEYBD_EVENT_FLAGS(0), 0);
        keybd_event(0xB3, 0, KEYEVENTF_KEYUP, 0);
    }
}

pub fn is_browser_exe(name: &str) -> bool {
    let name = file_name(name);
    BROWSERS.contains(&name.as_str())
}

fn valid_browser_exe(raw_path: &str) -> Option<String> {
    let exe = file_name(raw_path);
    const NON_BROWSERS: [&str; 9] = [
        "explorer.exe", "openwith.exe", "rundll32.exe", "dllhost.exe",
        "launchwinapp.exe", "applicationframehost.exe", "cmd.exe", "conhost.exe", "cs2.exe",
    ];
    (exe.ends_with(".exe") && !NON_BROWSERS.contains(&exe.as_str())).then_some(exe)
}

/// Default handler executable for the URL's scheme.
/// 网址协议对应的默认浏览器可执行文件名。
pub fn browser_exe(url: &str) -> Option<String> {
    use windows::Win32::UI::Shell::{AssocQueryStringW, ASSOCF_IS_PROTOCOL, ASSOCSTR_EXECUTABLE};
    let protocol = windows::core::HSTRING::from(url.split_once(':')?.0);
    let mut buffer = vec![0u16; 32768];
    let mut len = buffer.len() as u32;
    unsafe {
        AssocQueryStringW(ASSOCF_IS_PROTOCOL, ASSOCSTR_EXECUTABLE, &protocol, windows::core::w!("open"),
            windows::core::PWSTR(buffer.as_mut_ptr()), &mut len).ok().ok()?;
    }
    valid_browser_exe(&String::from_utf16_lossy(buffer.get(..len.checked_sub(1)? as usize)?))
}

/// Brings the first real window of `exe` to the front, regardless of whether `exe` is in `BROWSERS`.
/// 把 `exe` 的第一个真实窗口调到前台，无需局限于 `BROWSERS` 列表。
pub fn activate_browser(exe: &str) -> bool {
    let Some(target) = valid_browser_exe(exe) else { return false };
    let mut candidate = None;
    each_window(|hwnd| unsafe {
        if !IsWindowVisible(hwnd).as_bool()
            || GetWindowTextLengthW(hwnd) <= 0
            || GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0
            || !process_path(window_pid(hwnd)).is_some_and(|path| file_name(&path) == target)
        {
            return true;
        }
        candidate = Some(hwnd);
        false
    });
    candidate.is_some_and(|hwnd| unsafe { bring_to_front(hwnd, true) })
}

/// Minimizes the target program's windows, or all browser windows if `app_exe` is None.
/// 最小化目标程序的窗口；如果 `app_exe` 为 None，则最小化所有浏览器窗口。
pub fn minimize_browser(app_exe: Option<&str>) {
    let target = app_exe.map(file_name).filter(|exe| !exe.is_empty());
    each_window(|hwnd| unsafe {
        if IsWindowVisible(hwnd).as_bool() && GetWindowTextLengthW(hwnd) > 0 {
            let name = process_path(window_pid(hwnd)).map(|p| file_name(&p)).unwrap_or_default();
            let matched = match target.as_deref() {
                Some(exe) => name == exe,
                None => is_browser_exe(&name),
            };
            if matched {
                let _ = PostMessageW(hwnd, WM_SYSCOMMAND, WPARAM(SC_MINIMIZE as usize), LPARAM(0));
            }
        }
        true
    });
}

/// Terminates every process whose image name is in `names` (lower case).
/// 终止映像名在 `names`（小写）中的所有进程。
fn kill_processes(names: &[&str]) {
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return };
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            if names.contains(&from_wide(&entry.szExeFile).to_lowercase().as_str()) {
                if let Ok(process) = OpenProcess(PROCESS_TERMINATE, false, entry.th32ProcessID) {
                    let _ = TerminateProcess(process, 1);
                    let _ = CloseHandle(process);
                }
            }
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
}

pub fn kill_browsers() {
    kill_processes(&KILL_BROWSERS);
}

pub fn kill_app(exe: &str) {
    let exe = file_name(exe);
    if exe.ends_with(".exe") && !matches!(exe.as_str(), "explorer.exe" | "cs2.exe") {
        kill_processes(&[exe.as_str()]);
    }
}

fn shortcut_exe_name(fallback: &str, resolved: Option<String>) -> String {
    resolved.map(|target| file_name(&target)).filter(|s| !s.is_empty()).unwrap_or_else(|| fallback.to_string())
}

/// Lower-case file name of a program; a .lnk shortcut is resolved to its target.
/// 程序的小写文件名；.lnk 快捷方式会解析为它指向的目标。
pub fn app_exe_name(path: &str) -> String {
    let name = file_name(path);
    if !name.ends_with(".lnk") {
        return name;
    }
    let resolved = unsafe {
        (|| -> windows::core::Result<String> {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_ALL)?;
            link.cast::<IPersistFile>()?.Load(PCWSTR(wide(path).as_ptr()), STGM_READ)?;
            let mut buffer = [0u16; 1024];
            link.GetPath(&mut buffer, std::ptr::null_mut::<WIN32_FIND_DATAW>(), 0)?;
            Ok(from_wide(&buffer))
        })()
    };
    shortcut_exe_name(&name, resolved.ok())
}

/// Opens a URL, file or program with its default handler without showing modal error dialogs.
/// 用默认处理程序打开网址、文件或程序，失败时不弹出模态错误框。
pub fn shell_open(target: &str) -> bool {
    let target = wide(target);
    let verb = wide("open");
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_FLAG_NO_UI,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(target.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info).is_ok() }
}

/// Steam's install folder from the registry.
/// 从注册表读取 Steam 安装文件夹。
pub fn steam_path() -> Option<String> {
    let mut buffer = [0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    let key = wide("Software\\Valve\\Steam");
    let value = wide("SteamPath");
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    status.is_ok().then(|| from_wide(&buffer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_exe_filter_accepts_custom_browsers_and_rejects_shell_hosts() {
        assert_eq!(valid_browser_exe("C:\\Program Files\\Vivaldi\\Application\\vivaldi.exe").as_deref(), Some("vivaldi.exe"));
        assert_eq!(valid_browser_exe("D:\\QQBrowser\\QQBrowser.exe").as_deref(), Some("qqbrowser.exe"));
        assert_eq!(valid_browser_exe("C:\\Windows\\explorer.exe"), None);
        assert_eq!(valid_browser_exe("C:\\Windows\\System32\\OpenWith.exe"), None);
        assert_eq!(valid_browser_exe(""), None);
    }

    #[test]
    fn shortcut_resolution_falls_back_when_getpath_returns_empty() {
        assert_eq!(shortcut_exe_name("spotify.lnk", Some(String::new())), "spotify.lnk");
        assert_eq!(shortcut_exe_name("spotify.lnk", None), "spotify.lnk");
        assert_eq!(shortcut_exe_name("player.lnk", Some("C:\\Apps\\PotPlayerMini64.exe".into())), "potplayermini64.exe");
    }
}
