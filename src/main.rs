//! CSTool: switch away from CS2 when you die and back when the next round starts.
//! CSTool：死亡时切出 CS2，新回合开始时切回。
#![windows_subsystem = "windows"]

mod audio;
mod config;
mod engine;
mod gsi;
mod input_release;
mod log;
mod media;
mod ui;
mod update;
mod win;

use config::{Config, WindowSel};
use engine::{Engine, Platform, Snapshot};
use input_release::Releaser;
use std::cell::{Cell, RefCell};
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
const WM_APP_UPDATE: u32 = WM_APP + 4;
/// Sent by the tray icon when the user clicks a notification. 用户点击通知时由托盘图标发来。
const NIN_BALLOONUSERCLICK_: u32 = 0x405;
const TIMER_SWITCH_OUT: usize = 1;
const TIMER_RETURN: usize = 2;
const TIMER_RESTORE: usize = 3;
const TIMER_SWITCH_OUT_DONE: usize = 4;
const TIMER_LOG_AUDIO: usize = 5;
const TIMER_MEDIA_CHECK: usize = 6;
const MENU_ENABLE: usize = 1;
const MENU_SETTINGS: usize = 2;
const MENU_EXIT: usize = 3;
const MENU_CHECK_UPDATES: usize = 4;
const MENU_DOWNLOAD: usize = 5;

pub struct App {
    pub hwnd: HWND,
    pub hinstance: HINSTANCE,
    pub cfg: RefCell<Config>,
    pub engine: RefCell<Engine>,
    pub releaser: RefCell<Releaser>,
    /// The port actually listened on; None if no port could be bound.
    /// 实际监听的端口；无法绑定任何端口时为 None。
    pub port: Option<u16>,
    pub ui: ui::UiState,
    /// Tag of a newer release, once the update check has found one.
    /// 更新检查发现新版本后，这里保存它的标签。
    pub update: RefCell<Option<String>>,
    /// True if the notification on screen is the update notice. 屏幕上的通知是更新提示时为 true。
    update_notice_shown: Cell<bool>,
    taskbar_created: u32,
}

static APP: AtomicPtr<App> = AtomicPtr::new(std::ptr::null_mut());

/// The application state. Only ever touched from the UI thread.
/// 应用状态。只在界面线程中访问。
pub fn app() -> &'static App {
    unsafe { &*APP.load(Ordering::Relaxed) }
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
        log::write(&format!("death detected, switch-out in {ms} ms"));
        unsafe { SetTimer(self.hwnd, TIMER_SWITCH_OUT, ms, None) };
    }
    fn poll_release(&mut self) {
        unsafe { SetTimer(self.hwnd, TIMER_SWITCH_OUT, 50, None) };
    }
    fn input_held(&self) -> bool {
        self.release_input && Releaser::held().next().is_some()
    }
    fn start_return_timer(&mut self, ms: u32) {
        log::write(&format!("return conditions met, returning in {ms} ms"));
        unsafe { SetTimer(self.hwnd, TIMER_RETURN, ms, None) };
    }
    fn stop_return_timer(&mut self) {
        unsafe {
            let _ = KillTimer(self.hwnd, TIMER_RETURN);
        }
    }
    fn schedule_restore_game(&mut self) {
        log::write("returning to the game");
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
        log::write("minimizing the game");
        win::minimize_cs2();
    }
    fn prepare_input_switch(&mut self) -> bool {
        if !self.release_input {
            return true;
        }
        if let Err(error) = self.releaser.release() {
            log::write(&format!("held input release failed: {error}"));
            return false;
        }
        true
    }
    fn resolve_window(&mut self, sel: &WindowSel, rebind: bool) -> Result<Option<WindowSel>, ()> {
        Ok(win::resolve_window(sel, rebind))
    }
    fn activate_window(&mut self, window: &WindowSel) -> bool {
        let activated = win::activate_window(window);
        log::write(&format!("activate target window: {activated}; foreground is now {}", win::foreground_exe()));
        activated
    }
    fn close_window(&mut self, window: &WindowSel) {
        win::close_window(window);
    }
    fn send_media_key(&mut self) {
        log::write(&format!("pressing the play/pause key; foreground is {}", win::foreground_exe()));
        win::send_media_key();
    }
    fn media_command(&mut self, play: bool, exe: Option<&str>) -> bool {
        let (handled, sessions) = media::command(play, exe);
        log::write(&format!("direct media play={play} for {exe:?}: handled={handled} :: {sessions}"));
        handled
    }
    fn game_in_front(&self) -> bool { win::foreground_exe() == "cs2.exe" }
    fn schedule_media_check(&mut self, ms: u32) {
        unsafe { SetTimer(self.hwnd, TIMER_MEDIA_CHECK, ms, None) };
    }
    fn is_audio_playing(&mut self, app_exe: Option<&str>) -> bool {
        let playing = audio::is_audio_playing(app_exe);
        if log::enabled() {
            log::write(&format!("audio check for {app_exe:?}: playing={playing} :: {}", audio::describe_sessions()));
        }
        playing
    }
    fn browser_exe(&self, url: &str) -> Option<String> { win::browser_exe(url) }
    fn open_url(&mut self, url: &str) -> bool {
        win::shell_open(url)
    }
    fn launch_app(&mut self, path: &str) -> bool {
        std::path::Path::new(path).exists() && win::shell_open(path)
    }
    fn app_exe_name(&mut self, path: &str) -> String {
        win::app_exe_name(path)
    }
    fn activate_browser(&mut self, exe: &str) -> bool {
        win::activate_browser(exe)
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
        balloon(self.hwnd, message, true);
    }
}

/// Runs one state-machine step with the real platform.
/// 用真实平台执行状态机的一步。
pub fn with_engine(step: impl FnOnce(&mut Engine, &Config, &mut dyn Platform)) {
    let a = app();
    let cfg = a.cfg.borrow().clone();
    let mut engine = a.engine.borrow_mut();
    let mut releaser = a.releaser.borrow_mut();
    let mut platform = Plat { hwnd: a.hwnd, releaser: &mut releaser, release_input: cfg.release_input };
    step(&mut engine, &cfg, &mut platform);
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
    let handle = unsafe { LoadImageW(app().hinstance, PCWSTR(1 as _), IMAGE_ICON, size, size, LR_DEFAULTCOLOR | LR_SHARED) };
    HICON(handle.map(|h| h.0).unwrap_or(std::ptr::null_mut()))
}

/// Adds or refreshes the tray icon and its tooltip.
/// 添加或刷新托盘图标及其提示文字。
pub fn update_tray(add: bool) {
    let a = app();
    let enabled = a.cfg.borrow().enabled;
    let mut data = tray_data(a.hwnd);
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = WM_APP_TRAY;
    data.hIcon = load_icon(unsafe { GetSystemMetrics(SM_CXSMICON) });
    let state = if enabled { "已启用" } else { "已停用" };
    copy_text(&mut data.szTip, &format!("{APP_NAME}（{state}）"));
    unsafe {
        let _ = Shell_NotifyIconW(if add { NIM_ADD } else { NIM_MODIFY }, &data);
    }
}

/// Shows a notification from the tray icon, as a warning or as plain information.
/// 从托盘图标弹出一条通知，可以是警告或普通信息。
fn balloon(hwnd: HWND, message: &str, warning: bool) {
    app().update_notice_shown.set(!warning);
    let mut data = tray_data(hwnd);
    data.uFlags = NIF_INFO;
    data.dwInfoFlags = if warning { NIIF_WARNING } else { NIIF_INFO };
    copy_text(&mut data.szInfoTitle, APP_NAME);
    copy_text(&mut data.szInfo, message);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

pub fn set_enabled(enabled: bool) {
    {
        let mut cfg = app().cfg.borrow_mut();
        cfg.enabled = enabled;
        cfg.save();
    }
    update_tray(false);
    ui::sync_enabled();
}

pub fn open_download() {
    win::shell_open(update::RELEASES_URL);
}

/// Looks for a newer release in the background, so startup is never delayed.
/// 在后台查找新版本，不会拖慢启动。
fn start_update_check(hwnd: HWND) {
    let target = hwnd.0 as isize;
    std::thread::spawn(move || {
        if let Some(tag) = update::newer_release() {
            let pointer = Box::into_raw(Box::new(tag));
            unsafe {
                if PostMessageW(HWND(target as *mut _), WM_APP_UPDATE, WPARAM(0), LPARAM(pointer as isize)).is_err() {
                    drop(Box::from_raw(pointer));
                }
            }
        }
    });
}

fn tray_menu(hwnd: HWND) {
    let (enabled, check_updates) = {
        let cfg = app().cfg.borrow();
        (cfg.enabled, cfg.check_updates)
    };
    let update = app().update.borrow().clone();
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let checked = if enabled { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(menu, MF_STRING | checked, MENU_ENABLE, w!("启用切屏助手"));
        let _ = AppendMenuW(menu, MF_STRING, MENU_SETTINGS, w!("设置…"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        if let Some(tag) = &update {
            let text = win::wide(&format!("下载新版本 {tag}…"));
            let _ = AppendMenuW(menu, MF_STRING, MENU_DOWNLOAD, PCWSTR(text.as_ptr()));
        }
        let checking = if check_updates { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(menu, MF_STRING | checking, MENU_CHECK_UPDATES, w!("启动时检查更新"));
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
            MENU_ENABLE => {
                let enabled = app().cfg.borrow().enabled;
                set_enabled(!enabled);
            }
            MENU_SETTINGS => ui::show(),
            MENU_DOWNLOAD => open_download(),
            MENU_CHECK_UPDATES => {
                let mut cfg = app().cfg.borrow_mut();
                cfg.check_updates = !cfg.check_updates;
                cfg.save();
            }
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
                TIMER_SWITCH_OUT => {
                    with_engine(|engine, cfg, platform| engine.on_switch_out_timer(cfg, platform));
                    SetTimer(hwnd, TIMER_SWITCH_OUT_DONE, 1500, None);
                }
                TIMER_MEDIA_CHECK => with_engine(|engine, cfg, platform| engine.on_media_check_timer(cfg, platform)),
                TIMER_RETURN => with_engine(|engine, cfg, platform| engine.on_return_timer(cfg, platform)),
                TIMER_RESTORE => {
                    with_engine(|engine, _, platform| engine.on_restore_timer(platform));
                    log::write(&format!("game restored; foreground is now {}", win::foreground_exe()));
                    // A later look shows whether the media reacted to the key press.
                    // 稍后再看一次，可以知道媒体是否对按键作出了反应。
                    SetTimer(hwnd, TIMER_LOG_AUDIO, 1500, None);
                }
                TIMER_SWITCH_OUT_DONE | TIMER_LOG_AUDIO if log::enabled() => {
                    let (_, media) = media::command(false, Some("no-such-program.exe"));
                    log::write(&format!("media sessions: {media}"));
                    log::write(&format!("1.5 s later: foreground is {} :: {}", win::foreground_exe(), audio::describe_sessions()));
                }
                _ => {}
            }
        }
        WM_APP_UPDATE => {
            let tag = Box::from_raw(lparam.0 as *mut String);
            balloon(hwnd, &format!("发现新版本 {tag}，点击这里前往下载。"), false);
            *app().update.borrow_mut() = Some(*tag);
            ui::sync_update();
        }
        WM_APP_TRAY => match lparam.0 as u32 {
            NIN_BALLOONUSERCLICK_ if app().update_notice_shown.get() => open_download(),
            WM_LBUTTONUP | WM_LBUTTONDBLCLK => ui::show(),
            WM_RBUTTONUP => tray_menu(hwnd),
            _ => {}
        },
        WM_APP_SHOW => ui::show(),
        WM_DESTROY => {
            app().releaser.borrow_mut().close();
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
            cfg: RefCell::new(cfg),
            engine: RefCell::new(Engine::default()),
            releaser: RefCell::new(Releaser::default()),
            port,
            ui: ui::UiState::default(),
            update: RefCell::new(None),
            update_notice_shown: Cell::new(false),
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
        });
        APP.store(Box::into_raw(state), Ordering::Relaxed);
        if log::enabled() {
            log::write(&format!("CSTool {} diagnostic session started; GSI port: {port:?}", env!("CARGO_PKG_VERSION")));
        }

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
        if app().cfg.borrow().check_updates {
            start_update_check(hwnd);
        }
        if !std::env::args().any(|argument| argument == "--tray") {
            ui::show();
        }

        let mut message = MSG::default();
        while GetMessageW(&mut message, HWND::default(), 0, 0).0 > 0 {
            let settings = app().ui.hwnd.get();
            if !settings.0.is_null() && IsDialogMessageW(settings, &message).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
