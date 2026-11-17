//! CSTool: switch away from CS2 when you die and back when the next round starts.
//! CSTool：死亡时切出 CS2，新回合开始时切回。
#![windows_subsystem = "windows"]

mod audio;
mod config;
mod engine;
mod gsi;
mod input_release;
mod ui;
mod win;

use config::{Config, WindowSel};
use engine::{Engine, Platform, Snapshot};
use input_release::Releaser;
use std::sync::atomic::{AtomicPtr, Ordering};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Controls::{InitCommonControlsEx, ICC_BAR_CLASSES, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const APP_NAME: &str = "CSTool 死亡切屏";
const CLASS_NAME: PCWSTR = w!("CSToolMain");
const WM_APP_GSI: u32 = WM_APP + 1;
const WM_APP_TRAY: u32 = WM_APP + 2;
const WM_APP_SHOW: u32 = WM_APP + 3;
const TIMER_SWITCH_OUT: usize = 1;
const TIMER_RETURN: usize = 2;
const TIMER_RESTORE: usize = 3;
const MENU_ENABLE: usize = 1;
const MENU_SETTINGS: usize = 2;
const MENU_EXIT: usize = 3;

pub struct App {
    pub hwnd: HWND,
    pub hinstance: HINSTANCE,
    pub cfg: Config,
    pub engine: Engine,
    pub releaser: Releaser,
    /// The port actually listened on; None if no port could be bound.
    /// 实际监听的端口；无法绑定任何端口时为 None。
    pub port: Option<u16>,
    pub ui: ui::UiState,
    taskbar_created: u32,
}

static APP: AtomicPtr<App> = AtomicPtr::new(std::ptr::null_mut());

/// The application state. Only ever touched from the UI thread.
/// 应用状态。只在界面线程中访问。
pub fn app() -> &'static mut App {
    unsafe { &mut *APP.load(Ordering::Relaxed) }
}

/// Connects the state machine to real Windows calls and timers.
/// 把状态机连接到真实的 Windows 调用和定时器。
struct Plat<'a> {
    hwnd: HWND,
    releaser: &'a mut Releaser,
    release_input: bool,
}

impl Platform for Plat<'_> {
    fn schedule_switch_out(&mut self, ms: u32) {
        unsafe { SetTimer(self.hwnd, TIMER_SWITCH_OUT, ms, None) };
    }
    fn start_return_timer(&mut self, ms: u32) {
        unsafe { SetTimer(self.hwnd, TIMER_RETURN, ms, None) };
    }
    fn stop_return_timer(&mut self) {
        unsafe {
            let _ = KillTimer(self.hwnd, TIMER_RETURN);
        }
    }
    fn schedule_restore_game(&mut self) {
        // A short pause lets the previous step (such as closing a window) finish first.
        // 稍作停顿，让上一步（例如关闭窗口）先完成。
        unsafe { SetTimer(self.hwnd, TIMER_RESTORE, 100, None) };
    }
    fn restore_game_now(&mut self) {
        if win::find_cs2_window().is_some() {
            self.prepare_input_switch();
            win::restore_cs2();
        }
    }
    fn minimize_game(&mut self) {
        win::minimize_cs2();
    }
    fn prepare_input_switch(&mut self) -> bool {
        if !self.release_input {
            return true;
        }
        match self.releaser.release() {
            Ok(()) => true,
            Err(message) => {
                balloon(self.hwnd, message);
                false
            }
        }
    }
    fn resolve_window(&mut self, sel: &WindowSel, rebind: bool) -> Result<Option<WindowSel>, ()> {
        Ok(win::resolve_window(sel, rebind))
    }
    fn activate_window(&mut self, window: &WindowSel) -> bool {
        win::activate_window(window)
    }
    fn close_window(&mut self, window: &WindowSel) {
        win::close_window(window);
    }
    fn send_media_key(&mut self) {
        win::send_media_key();
    }
    fn is_audio_playing(&mut self, app_exe: Option<&str>) -> bool {
        audio::is_audio_playing(app_exe)
    }
    fn open_url(&mut self, url: &str) {
        win::shell_open(url);
    }
    fn launch_app(&mut self, path: &str) -> bool {
        std::path::Path::new(path).exists() && win::shell_open(path)
    }
    fn app_exe_name(&mut self, path: &str) -> String {
        win::app_exe_name(path)
    }
    fn activate_browser(&mut self) -> bool {
        win::activate_browser()
    }
    fn minimize_browser(&mut self, app_exe: Option<&str>) {
        win::minimize_browser(app_exe);
    }
    fn kill_browsers(&mut self) {
        win::kill_browsers();
    }
    fn kill_app(&mut self, exe: &str) {
        win::kill_app(exe);
    }
    fn notify_failure(&mut self, message: &str) {
        balloon(self.hwnd, message);
    }
}

/// Runs one state-machine step with the real platform.
/// 用真实平台执行状态机的一步。
pub fn with_engine(step: impl FnOnce(&mut Engine, &Config, &mut dyn Platform)) {
    let a = app();
    let mut platform = Plat { hwnd: a.hwnd, releaser: &mut a.releaser, release_input: a.cfg.release_input };
    step(&mut a.engine, &a.cfg, &mut platform);
}

fn copy_text(target: &mut [u16], text: &str) {
    let source: Vec<u16> = text.encode_utf16().take(target.len() - 1).collect();
    target[..source.len()].copy_from_slice(&source);
    target[source.len()] = 0;
}

fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: 1, ..Default::default() }
}

pub fn load_icon(size: i32) -> HICON {
    let handle = unsafe { LoadImageW(app().hinstance, PCWSTR(1 as _), IMAGE_ICON, size, size, LR_DEFAULTCOLOR) };
    HICON(handle.map(|h| h.0).unwrap_or(std::ptr::null_mut()))
}

/// Adds or refreshes the tray icon and its tooltip.
/// 添加或刷新托盘图标及其提示文字。
pub fn update_tray(add: bool) {
    let a = app();
    let mut data = tray_data(a.hwnd);
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = WM_APP_TRAY;
    data.hIcon = load_icon(unsafe { GetSystemMetrics(SM_CXSMICON) });
    let state = if a.cfg.enabled { "已启用" } else { "已停用" };
    copy_text(&mut data.szTip, &format!("{APP_NAME}（{state}）"));
    unsafe {
        let _ = Shell_NotifyIconW(if add { NIM_ADD } else { NIM_MODIFY }, &data);
    }
}

/// Shows a warning notification from the tray icon.
/// 从托盘图标弹出一条警告通知。
fn balloon(hwnd: HWND, message: &str) {
    let mut data = tray_data(hwnd);
    data.uFlags = NIF_INFO;
    data.dwInfoFlags = NIIF_WARNING;
    copy_text(&mut data.szInfoTitle, APP_NAME);
    copy_text(&mut data.szInfo, message);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

pub fn set_enabled(enabled: bool) {
    let a = app();
    a.cfg.enabled = enabled;
    a.cfg.save();
    update_tray(false);
    ui::sync_enabled();
}

fn tray_menu(hwnd: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let checked = if app().cfg.enabled { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(menu, MF_STRING | checked, MENU_ENABLE, w!("启用死亡切屏"));
        let _ = AppendMenuW(menu, MF_STRING, MENU_SETTINGS, w!("设置…"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, MENU_EXIT, w!("退出"));
        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        // Required so the menu closes when the user clicks elsewhere.
        // 必须调用，否则点击别处时菜单不会关闭。
        let _ = SetForegroundWindow(hwnd);
        let command = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, point.x, point.y, 0, hwnd, None);
        let _ = DestroyMenu(menu);
        match command.0 as usize {
            MENU_ENABLE => set_enabled(!app().cfg.enabled),
            MENU_SETTINGS => ui::show(),
            MENU_EXIT => {
                let _ = DestroyWindow(hwnd);
            }
            _ => {}
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if APP.load(Ordering::Relaxed).is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    match message {
        WM_APP_GSI => {
            let snapshot = Box::from_raw(lparam.0 as *mut Snapshot);
            with_engine(|engine, cfg, platform| engine.on_snapshot(&snapshot, cfg, platform));
        }
        WM_TIMER => {
            let _ = KillTimer(hwnd, wparam.0);
            match wparam.0 {
                TIMER_SWITCH_OUT => with_engine(|engine, cfg, platform| engine.on_switch_out_timer(cfg, platform)),
                TIMER_RETURN => with_engine(|engine, cfg, platform| engine.on_return_timer(cfg, platform)),
                TIMER_RESTORE => with_engine(|_, _, platform| platform.restore_game_now()),
                _ => {}
            }
        }
        WM_APP_TRAY => match lparam.0 as u32 {
            WM_LBUTTONUP | WM_LBUTTONDBLCLK => ui::show(),
            WM_RBUTTONUP => tray_menu(hwnd),
            _ => {}
        },
        WM_APP_SHOW => ui::show(),
        WM_DESTROY => {
            app().releaser.close();
            let _ = Shell_NotifyIconW(NIM_DELETE, &tray_data(hwnd));
            PostQuitMessage(0);
        }
        // Explorer restarted: the tray icon has to be added again.
        // 资源管理器重启后需要重新添加托盘图标。
        other if other == app().taskbar_created => update_tray(true),
        _ => return DefWindowProcW(hwnd, message, wparam, lparam),
    }
    LRESULT(0)
}

fn main() {
    unsafe {
        // Single instance: a second launch just opens the first one's settings window.
        // 单实例：再次启动只会打开已运行实例的设置窗口。
        let _mutex = CreateMutexW(None, true, w!("CSTool.SingleInstance"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(existing) = FindWindowW(CLASS_NAME, PCWSTR::null()) {
                let _ = PostMessageW(existing, WM_APP_SHOW, WPARAM(0), LPARAM(0));
            }
            return;
        }

        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let controls = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_BAR_CLASSES | ICC_STANDARD_CLASSES,
        };
        let _ = InitCommonControlsEx(&controls);

        let hinstance: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let mut cfg = Config::load();
        if gsi::cfg_status(&cfg.cs2_path, cfg.port) == gsi::CfgStatus::NoGame {
            if let Some(found) = win::steam_path().and_then(|steam| gsi::find_cs2(&steam)) {
                cfg.cs2_path = found;
                cfg.save();
            }
        }

        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance,
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        RegisterClassExW(&class);
        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE(0), CLASS_NAME, w!("CSTool"), WS_OVERLAPPED, 0, 0, 0, 0, None, None, hinstance, None,
        ) else {
            return;
        };

        let bound = gsi::bind(cfg.port);
        let port = bound.as_ref().map(|(_, port)| *port);
        if let Some(port) = port {
            // Keep the game's config pointing at the port we really listen on.
            // 让游戏里的配置始终指向我们实际监听的端口。
            if !matches!(gsi::cfg_status(&cfg.cs2_path, port), gsi::CfgStatus::Ready | gsi::CfgStatus::NoGame) {
                gsi::write_cfg(&cfg.cs2_path, port);
            }
        }

        let state = Box::new(App {
            hwnd,
            hinstance,
            cfg,
            engine: Engine::default(),
            releaser: Releaser::default(),
            port,
            ui: ui::UiState::default(),
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
        });
        APP.store(Box::into_raw(state), Ordering::Relaxed);

        if let Some((listener, _)) = bound {
            let target = hwnd.0 as isize;
            std::thread::spawn(move || {
                gsi::serve(listener, |snapshot| {
                    let pointer = Box::into_raw(Box::new(snapshot));
                    if PostMessageW(HWND(target as *mut _), WM_APP_GSI, WPARAM(0), LPARAM(pointer as isize)).is_err() {
                        drop(Box::from_raw(pointer));
                    }
                })
            });
        }

        update_tray(true);
        if !std::env::args().any(|argument| argument == "--tray") {
            ui::show();
        }

        let mut message = MSG::default();
        while GetMessageW(&mut message, HWND::default(), 0, 0).0 > 0 {
            let settings = app().ui.hwnd;
            if !settings.0.is_null() && IsDialogMessageW(settings, &message).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
