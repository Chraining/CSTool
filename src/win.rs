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
use windows::Win32::UI::Shell::{IShellLinkW, ShellExecuteW, ShellLink};
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
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        if !SetForegroundWindow(hwnd).as_bool() {
            return false;
        }
        // Minimizing the game makes Windows activate the next window a moment later, which can
        // steal the foreground back. Watch briefly and re-assert until the target stays in front.
        // 最小化游戏后，Windows 会稍后激活下一个窗口，可能把前台抢走。
        // 这里短暂观察并重新置前，直到目标窗口稳定在前台。
        let mut stable = 0;
        for _ in 0..10 {
            std::thread::sleep(std::time::Duration::from_millis(30));
            if GetForegroundWindow() == hwnd {
                stable += 1;
                if stable >= 4 {
                    break;
                }
            } else {
                stable = 0;
                force_foreground(hwnd);
            }
        }
        true
    }
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
        let _ = SetForegroundWindow(hwnd);
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = SetForegroundWindow(hwnd);
        // Windows may refuse to hand over the foreground; a simulated Alt press lifts that restriction.
        // Windows 可能拒绝让出前台；模拟按一次 Alt 键可以解除这个限制。
        keybd_event(0x12, 0, KEYBD_EVENT_FLAGS(0), 0);
        keybd_event(0x12, 0, KEYEVENTF_KEYUP, 0);
        let _ = SetForegroundWindow(hwnd);
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
    BROWSERS.iter().any(|b| name.contains(b))
}

fn is_browser_window(hwnd: HWND) -> bool {
    let visible = unsafe { IsWindowVisible(hwnd).as_bool() && GetWindowTextLengthW(hwnd) > 0 };
    visible && process_path(window_pid(hwnd)).is_some_and(|p| is_browser_exe(&file_name(&p)))
}

/// Brings the first real browser window to the front.
/// 把第一个真正的浏览器窗口调到前台。
pub fn activate_browser() -> bool {
    let mut found = false;
    each_window(|hwnd| unsafe {
        if !is_browser_window(hwnd) || GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
            return true;
        }
        let mut placement = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
        let _ = GetWindowPlacement(hwnd, &mut placement);
        // Keep a maximized window maximized. 原本最大化的窗口保持最大化。
        let maximized = placement.showCmd == SW_SHOWMAXIMIZED.0 as u32 || placement.flags.0 & WPF_RESTORETOMAXIMIZED.0 != 0;
        let show = if maximized { SW_SHOWMAXIMIZED } else { SW_RESTORE };
        let _ = ShowWindow(hwnd, show);
        force_foreground(hwnd);
        found = true;
        false
    });
    found
}

/// Minimizes browser windows and, if given, the windows of the launched program.
/// 最小化浏览器窗口；如果指定了启动的程序，也最小化它的窗口。
pub fn minimize_browser(app_exe: Option<&str>) {
    each_window(|hwnd| unsafe {
        if IsWindowVisible(hwnd).as_bool() && GetWindowTextLengthW(hwnd) > 0 {
            let name = process_path(window_pid(hwnd)).map(|p| file_name(&p)).unwrap_or_default();
            if is_browser_exe(&name) || app_exe.is_some_and(|exe| !exe.is_empty() && name.contains(exe)) {
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
    if exe.ends_with(".exe") {
        kill_processes(&[exe]);
    }
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
    resolved.map(|target| file_name(&target)).unwrap_or(name)
}

/// Opens a URL, file or program with its default handler.
/// 用默认处理程序打开网址、文件或程序。
pub fn shell_open(target: &str) -> bool {
    let target = wide(target);
    let verb = wide("open");
    let result = unsafe {
        ShellExecuteW(None, PCWSTR(verb.as_ptr()), PCWSTR(target.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL)
    };
    result.0 as usize > 32
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
