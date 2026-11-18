//! The settings window, built from native Windows controls.
//! 设置窗口，由原生 Windows 控件构成。

use crate::config::{Action, TargetType, WindowSel, MAX_DELAY};
use crate::gsi::{self, CfgStatus};
use crate::win::{file_name, from_wide, wide};
use crate::{app, with_engine};
use std::sync::atomic::Ordering;
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY, OPENFILENAMEW};
use windows::Win32::UI::Controls::{TOOLTIP_FLAGS, TTTOOLINFOW};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem, SystemParametersInfoForDpi};
use windows::Win32::UI::Shell::{SHBrowseForFolderW, SHGetPathFromIDListW, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW};
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: PCWSTR = w!("CSToolSettings");
const CLIENT_W: i32 = 440;
const CLIENT_H: i32 = 576;
const STATUS_TIMER: usize = 1;

const ID_MODE: i32 = 101;
const ID_BANNER_TITLE: i32 = 102;
const ID_BANNER_DETAIL: i32 = 120;
const ID_FOOTER: i32 = 121;
const ID_DOWNLOAD: i32 = 122;
const ID_TYPE: i32 = 103;
const ID_WINDOW: i32 = 104;
const ID_REFRESH: i32 = 105;
const ID_TARGET: i32 = 106;
const ID_BROWSE: i32 = 107;
const ID_TARGET_HINT: i32 = 108;
const ID_DELAY: i32 = 109;
const ID_DELAY_VALUE: i32 = 110;
const ID_RETURN: i32 = 111;
const ID_RETURN_VALUE: i32 = 112;
const ID_ACTION: i32 = 113;
const ID_RELEASE: i32 = 114;
const ID_CS2_PATH: i32 = 115;
const ID_CS2_BROWSE: i32 = 116;
const ID_CFG_STATUS: i32 = 117;
const ID_CFG_WRITE: i32 = 118;
const ID_TARGET_LABEL: i32 = 119;
/// Static texts with this id are drawn in gray. 使用这个 id 的静态文本以灰色绘制。
const ID_HINT: i32 = 200;

// Control styles and messages, as plain numbers. 控件样式和消息（直接用数值）。
const WS_TAB: u32 = 0x0001_0000;
const WS_VSCROLL_: u32 = 0x0020_0000;
const BS_AUTOCHECKBOX_: u32 = 0x3;
const CBS_DROPDOWNLIST_: u32 = 0x3;
const ES_AUTOHSCROLL_: u32 = 0x80;
const TBS_AUTOTICKS_: u32 = 0x1;
const SS_NOPREFIX_: u32 = 0x80;
const SS_PATHELLIPSIS_: u32 = 0x8000;
const TTS_ALWAYSTIP_NOPREFIX: u32 = 0x3;
const TTM_ADDTOOLW_: u32 = 0x432;
const TTF_IDISHWND_SUBCLASS: u32 = 0x11;
const TTF_SUBCLASS_: u32 = 0x10;
const CB_ADDSTRING_: u32 = 0x143;
const CB_GETCURSEL_: u32 = 0x147;
const CB_RESETCONTENT_: u32 = 0x14B;
const CB_SETCURSEL_: u32 = 0x14E;
const BM_GETCHECK_: u32 = 0xF0;
const BM_SETCHECK_: u32 = 0xF1;
const TBM_GETPOS_: u32 = 0x400;
const TBM_SETPOS_: u32 = 0x405;
const TBM_SETRANGE_: u32 = 0x406;
const TBM_SETPAGESIZE_: u32 = 0x415;
const CBN_SELCHANGE_: u32 = 1;
const EN_CHANGE_: u32 = 0x300;

#[derive(Default)]
pub struct UiState {
    pub hwnd: HWND,
    dpi: u32,
    font: HFONT,
    bold: HFONT,
    /// Windows listed in the combo box, in order (after the placeholder row).
    /// 下拉框中列出的窗口，按顺序排列（位于占位行之后）。
    windows: Vec<WindowSel>,
    /// Index into BANNER_COLORS for the readiness banner. 就绪横幅在 BANNER_COLORS 中的下标。
    level: usize,
    brushes: [HBRUSH; 4],
    /// True while the code itself fills controls, so change events are ignored.
    /// 代码自己填充控件时为 true，此时忽略变更事件。
    loading: bool,
}

fn px(value: i32) -> i32 {
    value * app().ui.dpi as i32 / 96
}

fn item(id: i32) -> HWND {
    unsafe { GetDlgItem(app().ui.hwnd, id).unwrap_or_default() }
}

fn send(id: i32, message: u32, wparam: usize, lparam: isize) -> isize {
    unsafe { SendMessageW(item(id), message, WPARAM(wparam), LPARAM(lparam)).0 }
}

fn set_text(id: i32, text: &str) {
    let control = item(id);
    unsafe {
        // Skip unchanged text to avoid flicker. 文本未变化时跳过，避免闪烁。
        let mut current = vec![0u16; GetWindowTextLengthW(control) as usize + 1];
        GetWindowTextW(control, &mut current);
        if from_wide(&current) != text {
            let _ = SetWindowTextW(control, PCWSTR(wide(text).as_ptr()));
        }
    }
}

fn show_item(id: i32, visible: bool) {
    unsafe {
        let _ = ShowWindow(item(id), if visible { SW_SHOW } else { SW_HIDE });
    }
}

fn add_string(id: i32, text: &str) {
    send(id, CB_ADDSTRING_, 0, wide(text).as_ptr() as isize);
}

#[allow(clippy::too_many_arguments)]
fn add(class: PCWSTR, text: &str, style: u32, ex: u32, x: i32, y: i32, width: i32, height: i32, id: i32, bold: bool) {
    let a = app();
    unsafe {
        let control = CreateWindowExW(
            WINDOW_EX_STYLE(ex),
            class,
            PCWSTR(wide(text).as_ptr()),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(style),
            px(x), px(y), px(width), px(height),
            a.ui.hwnd,
            HMENU(id as isize as *mut _),
            a.hinstance,
            None,
        )
        .unwrap_or_default();
        let font = if bold { a.ui.bold } else { a.ui.font };
        SendMessageW(control, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
    }
}

fn label(text: &str, x: i32, y: i32, width: i32, id: i32) {
    add(w!("STATIC"), text, SS_NOPREFIX_, 0, x, y, width, 20, id, false);
}

fn header(text: &str, y: i32) {
    add(w!("STATIC"), text, SS_NOPREFIX_, 0, 20, y, 400, 20, -1, true);
}

fn build() {
    add(w!("STATIC"), "", SS_NOPREFIX_, 0, 26, 17, 388, 20, ID_BANNER_TITLE, true);
    add(w!("STATIC"), "", SS_NOPREFIX_, 0, 26, 39, 388, 20, ID_BANNER_DETAIL, false);
    label("切屏模式", 20, 82, 84, -1);
    add(w!("COMBOBOX"), "", CBS_DROPDOWNLIST_ | WS_VSCROLL_ | WS_TAB, 0, 108, 78, 312, 120, ID_MODE, false);
    for mode in ["死亡自动切出，自动切回", "死亡不会自动切出，仅自动切回", "停用"] {
        add_string(ID_MODE, mode);
    }

    header("死亡后切到", 112);
    label("目标类型", 20, 140, 84, -1);
    add(w!("COMBOBOX"), "", CBS_DROPDOWNLIST_ | WS_VSCROLL_ | WS_TAB, 0, 108, 136, 312, 200, ID_TYPE, false);
    for name in ["当前打开的应用", "网页", "本地程序"] {
        add_string(ID_TYPE, name);
    }
    label("目标", 20, 172, 84, ID_TARGET_LABEL);
    add(w!("COMBOBOX"), "", CBS_DROPDOWNLIST_ | WS_VSCROLL_ | WS_TAB, 0, 108, 168, 232, 300, ID_WINDOW, false);
    add(w!("BUTTON"), "刷新", WS_TAB, 0, 348, 167, 72, 26, ID_REFRESH, false);
    add(w!("EDIT"), "", ES_AUTOHSCROLL_ | WS_TAB, WS_EX_CLIENTEDGE.0, 108, 168, 232, 24, ID_TARGET, false);
    add(w!("BUTTON"), "浏览…", WS_TAB, 0, 348, 167, 72, 26, ID_BROWSE, false);
    label("", 108, 198, 312, ID_TARGET_HINT);

    header("延迟", 228);
    label("延迟切出", 20, 258, 84, -1);
    add(w!("msctls_trackbar32"), "", TBS_AUTOTICKS_ | WS_TAB, 0, 102, 252, 264, 30, ID_DELAY, false);
    label("", 374, 258, 46, ID_DELAY_VALUE);
    label("延迟切回", 20, 292, 84, -1);
    add(w!("msctls_trackbar32"), "", TBS_AUTOTICKS_ | WS_TAB, 0, 102, 286, 264, 30, ID_RETURN, false);
    label("", 374, 292, 46, ID_RETURN_VALUE);
    label("满足买枪阶段或复活的切回条件后，才开始计算“延迟切回”。", 20, 320, 400, ID_HINT);
    for id in [ID_DELAY, ID_RETURN] {
        send(id, TBM_SETRANGE_, 1, (MAX_DELAY as isize) << 16);
        send(id, TBM_SETPAGESIZE_, 0, 1);
    }

    header("切屏时的附加动作", 352);
    label("附加动作", 20, 380, 84, -1);
    add(w!("COMBOBOX"), "", CBS_DROPDOWNLIST_ | WS_VSCROLL_ | WS_TAB, 0, 108, 376, 312, 200, ID_ACTION, false);
    add(w!("BUTTON"), "切屏前松开按住的键盘键和鼠标键", BS_AUTOCHECKBOX_ | WS_TAB, 0, 20, 408, 400, 22, ID_RELEASE, false);
    label("松开后需要实际松手再按下才能继续输入（最长 5 秒）。", 38, 430, 382, ID_HINT);

    header("CS2 连接", 462);
    add(w!("STATIC"), "", SS_NOPREFIX_ | SS_PATHELLIPSIS_, 0, 20, 490, 320, 20, ID_CS2_PATH, false);
    add(w!("BUTTON"), "选择…", WS_TAB, 0, 348, 486, 72, 26, ID_CS2_BROWSE, false);
    label("", 20, 522, 320, ID_CFG_STATUS);
    add(w!("BUTTON"), "写入配置", WS_TAB, 0, 348, 518, 72, 26, ID_CFG_WRITE, false);
    label("关闭此窗口后，程序继续在系统托盘中运行。", 20, 550, 300, ID_FOOTER);
    add(w!("BUTTON"), "下载新版本", WS_TAB, 0, 328, 545, 92, 26, ID_DOWNLOAD, false);
    show_item(ID_DOWNLOAD, false);
    add_tooltip(&[ID_DELAY], (20, 252, 400, 30), "推荐 4 秒，方便观看完整的死亡视角");
}

/// Shows `text` when the pointer rests on any of the given controls.
/// 鼠标指针停在任一指定控件上时显示 `text`。
fn add_tooltip(ids: &[i32], row: (i32, i32, i32, i32), text: &str) {
    let a = app();
    let mut text = wide(text);
    unsafe {
        let Ok(tip) = CreateWindowExW(
            WS_EX_TOPMOST,
            w!("tooltips_class32"),
            PCWSTR::null(),
            WS_POPUP | WINDOW_STYLE(TTS_ALWAYSTIP_NOPREFIX),
            CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT,
            a.ui.hwnd, None, a.hinstance, None,
        ) else {
            return;
        };
        for &id in ids {
            let tool = TTTOOLINFOW {
                cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
                uFlags: TOOLTIP_FLAGS(TTF_IDISHWND_SUBCLASS),
                hwnd: a.ui.hwnd,
                uId: item(id).0 as usize,
                lpszText: PWSTR(text.as_mut_ptr()),
                ..Default::default()
            };
            SendMessageW(tip, TTM_ADDTOOLW_, WPARAM(0), LPARAM(&tool as *const _ as isize));
        }
        // Plain text labels pass mouse messages through to the window, so the rest of the row
        // is covered by a rectangle on the window itself.
        // 普通文本标签会把鼠标消息传给窗口，所以这一行的其余部分用窗口上的一个矩形来覆盖。
        let (x, y, width, height) = row;
        let tool = TTTOOLINFOW {
            cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
            uFlags: TOOLTIP_FLAGS(TTF_SUBCLASS_),
            hwnd: a.ui.hwnd,
            uId: 1,
            rect: RECT { left: px(x), top: px(y), right: px(x + width), bottom: px(y + height) },
            lpszText: PWSTR(text.as_mut_ptr()),
            ..Default::default()
        };
        SendMessageW(tip, TTM_ADDTOOLW_, WPARAM(0), LPARAM(&tool as *const _ as isize));
    }
}

/// Fills every control from the current settings.
/// 用当前设置填充所有控件。
fn load() {
    let a = app();
    a.ui.loading = true;
    sync_mode_selection();
    send(ID_RELEASE, BM_SETCHECK_, a.cfg.release_input as usize, 0);
    send(ID_TYPE, CB_SETCURSEL_, a.cfg.target_type as usize, 0);
    send(ID_DELAY, TBM_SETPOS_, 1, a.cfg.delay as isize);
    send(ID_RETURN, TBM_SETPOS_, 1, a.cfg.return_delay as isize);
    set_text(ID_DELAY_VALUE, &format!("{} 秒", a.cfg.delay));
    set_text(ID_RETURN_VALUE, &format!("{} 秒", a.cfg.return_delay));
    a.ui.loading = false;
    fill_windows();
    update_target();
    update_status();
    update_cs2();
    sync_update();
}

fn fill_windows() {
    let a = app();
    a.ui.windows = crate::win::list_windows();
    send(ID_WINDOW, CB_RESETCONTENT_, 0, 0);
    add_string(ID_WINDOW, "请选择已打开的应用");
    // Select the window the switch itself would use, so the list matches what will happen.
    // 选中切屏时实际会使用的那个窗口，让列表与实际行为一致。
    let current = a.cfg.window.as_ref().and_then(|sel| crate::win::pick_window(&a.ui.windows, sel, true));
    let mut selected = 0;
    for (index, window) in a.ui.windows.iter().enumerate() {
        let text: String = format!("{} — {}", file_name(&window.path), window.title).chars().take(60).collect();
        add_string(ID_WINDOW, &text);
        if current == Some(window) {
            selected = index + 1;
        }
    }
    send(ID_WINDOW, CB_SETCURSEL_, selected, 0);
    let hint = if a.cfg.window.is_some() && selected == 0 {
        "所选应用已关闭或窗口已变化，请重新选择。"
    } else if a.ui.windows.is_empty() {
        "没有可选择的应用，请打开应用后点击“刷新”。"
    } else {
        ""
    };
    set_text(ID_TARGET_HINT, hint);
}

/// Shows the controls that belong to the chosen target type.
/// 显示与所选目标类型对应的控件。
fn update_target() {
    let a = app();
    let kind = a.cfg.target_type;
    show_item(ID_WINDOW, kind == TargetType::Window);
    show_item(ID_REFRESH, kind == TargetType::Window);
    show_item(ID_TARGET_HINT, kind == TargetType::Window);
    show_item(ID_TARGET, kind != TargetType::Window);
    show_item(ID_BROWSE, kind == TargetType::App);
    unsafe {
        let width = if kind == TargetType::Url { 312 } else { 232 };
        let _ = SetWindowPos(item(ID_TARGET), None, 0, 0, px(width), px(24), SWP_NOMOVE | SWP_NOZORDER);
    }
    set_text(ID_TARGET_LABEL, match kind {
        TargetType::Window => "应用窗口",
        TargetType::Url => "网址",
        TargetType::App => "程序路径",
    });
    a.ui.loading = true;
    set_text(ID_TARGET, if kind == TargetType::App { &a.cfg.app } else { &a.cfg.url });
    a.ui.loading = false;

    let actions = match kind {
        TargetType::Window => ["无", "切回时关闭所选窗口", "切出时播放媒体，切回时暂停"],
        TargetType::Url => ["无", "切回时强制关闭所有浏览器进程", "切出时播放媒体，切回时暂停"],
        TargetType::App => ["无", "切回时强制关闭该程序的进程", "切出时播放媒体，切回时暂停"],
    };
    send(ID_ACTION, CB_RESETCONTENT_, 0, 0);
    for name in actions {
        add_string(ID_ACTION, name);
    }
    send(ID_ACTION, CB_SETCURSEL_, a.cfg.action as usize, 0);
}

/// Banner colors per level: (background, text), as 0x00BBGGRR.
/// 每个级别的横幅颜色：（背景，文字），格式为 0x00BBGGRR。
const BANNER_COLORS: [(u32, u32); 4] = [
    (0x00EA_F4E6, 0x0033_7313), // ready 已就绪：绿色
    (0x00E0_F7FE, 0x0000_60B0), // waiting 等待中：黄色
    (0x00E6_E8FC, 0x001F_22C5), // problem 有问题：红色
    (0x00F4_F3F1, 0x0068_635F), // off 已停用：灰色
];
const READY: usize = 0;
const WAITING: usize = 1;
const PROBLEM: usize = 2;
const OFF: usize = 3;
/// Seconds without a message before a running game counts as not connected.
/// 游戏运行时，超过这么多秒没有消息就视为未连接。
const SILENCE_LIMIT: u64 = 30;

fn banner_rect() -> RECT {
    RECT { left: px(12), top: px(10), right: px(CLIENT_W - 12), bottom: px(66) }
}

/// Works out whether everything needed for switching is in place, and what to do if not.
/// 判断切屏所需的条件是否都已满足；如果没有，给出该怎么做。
fn readiness() -> (usize, String, String) {
    let a = app();
    let text = |level, title: &str, detail: &str| (level, title.to_string(), detail.to_string());
    if !a.cfg.enabled {
        return text(OFF, "已停用", "请选择下方的切屏模式以启用助手。");
    }
    let Some(port) = a.port else {
        let detail = format!("端口 {}–{} 都被占用，请关闭占用的程序后重新启动本软件。", a.cfg.port, a.cfg.port.saturating_add(9));
        return (PROBLEM, "无法工作：端口被占用".to_string(), detail);
    };
    match gsi::cfg_status(&a.cfg.cs2_path, port) {
        CfgStatus::NoGame => return text(PROBLEM, "未就绪：没有找到 CS2", "请在最下方点击“选择…”，指定 CS2 的安装文件夹。"),
        CfgStatus::Missing | CfgStatus::Outdated => {
            return text(PROBLEM, "未就绪：游戏配置尚未写入", "请在最下方点击“写入配置”，然后重启 CS2。")
        }
        CfgStatus::Ready => {}
    }
    let target_problem = match a.cfg.target_type {
        TargetType::Window => match &a.cfg.window {
            None => Some("请在“应用窗口”中选择死亡后要切到的应用。"),
            Some(sel) if crate::win::resolve_window(sel, true).is_none() => Some("所选的应用已关闭，请重新打开它，或点击“刷新”后另选。"),
            Some(_) => None,
        },
        TargetType::Url if a.cfg.url.is_empty() => Some("请填写死亡后要打开的网址。"),
        TargetType::App if !std::path::Path::new(&a.cfg.app).is_file() => Some("请点击“浏览…”选择死亡后要启动的程序。"),
        _ => None,
    };
    if let Some(detail) = target_problem.filter(|_| a.cfg.auto_switch_out) {
        return text(PROBLEM, "未就绪：没有可用的切换目标", detail);
    }
    if crate::win::find_cs2_window().is_none() {
        return text(WAITING, "等待 CS2 启动", "设置已完成。启动游戏后会自动连接，无需其他操作。");
    }
    let last = gsi::LAST_MESSAGE.load(Ordering::Relaxed);
    if last == 0 || gsi::now().saturating_sub(last) > SILENCE_LIMIT {
        return text(PROBLEM, "CS2 正在运行，但没有收到游戏数据", "请重启一次 CS2，让它加载刚写入的配置。");
    }
    if !a.cfg.auto_switch_out {
        return text(READY, "已就绪", "手动切出后，下一回合买枪时自动切回。");
    }
    (READY, "已就绪".to_string(), format!("已连接 CS2（端口 {port}），死亡后会自动切屏。"))
}

fn update_status() {
    let (level, title, detail) = readiness();
    set_text(ID_BANNER_TITLE, &title);
    set_text(ID_BANNER_DETAIL, &detail);
    let a = app();
    if level != a.ui.level {
        a.ui.level = level;
        unsafe {
            let _ = InvalidateRect(a.ui.hwnd, Some(&banner_rect()), true);
        }
    }
}

fn update_cs2() {
    let a = app();
    let status = gsi::cfg_status(&a.cfg.cs2_path, a.port.unwrap_or(a.cfg.port));
    let path = if status == CfgStatus::NoGame { "未找到 CS2，请点击“选择…”指定安装文件夹" } else { &a.cfg.cs2_path };
    set_text(ID_CS2_PATH, path);
    set_text(ID_CFG_STATUS, match status {
        CfgStatus::NoGame => "GSI 配置：未写入",
        CfgStatus::Missing => "GSI 配置：尚未写入",
        CfgStatus::Outdated => "GSI 配置：需要更新",
        CfgStatus::Ready => "GSI 配置：已就绪（首次写入后需重启 CS2）",
    });
    unsafe {
        let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(item(ID_CFG_WRITE), status != CfgStatus::NoGame);
    }
}

fn message_box(text: &str) {
    unsafe {
        MessageBoxW(app().ui.hwnd, PCWSTR(wide(text).as_ptr()), PCWSTR(wide(crate::APP_NAME).as_ptr()), MB_OK | MB_ICONWARNING);
    }
}

fn write_cfg() {
    let a = app();
    if !gsi::write_cfg(&a.cfg.cs2_path, a.port.unwrap_or(a.cfg.port)) {
        message_box("无法写入 GSI 配置文件，请检查 CS2 文件夹是否可写。");
    }
    update_cs2();
}

fn browse_program() {
    let mut file = [0u16; 1024];
    let filter = wide("程序或快捷方式\0*.exe;*.lnk;*.bat;*.cmd\0所有文件\0*.*\0");
    let mut dialog = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: app().ui.hwnd,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(file.as_mut_ptr()),
        nMaxFile: file.len() as u32,
        Flags: OFN_FILEMUSTEXIST | OFN_HIDEREADONLY,
        ..Default::default()
    };
    if unsafe { GetOpenFileNameW(&mut dialog) }.as_bool() {
        // Setting the text triggers EN_CHANGE, which saves it. 设置文本会触发 EN_CHANGE，从而保存。
        unsafe {
            let _ = SetWindowTextW(item(ID_TARGET), PCWSTR(file.as_ptr()));
        }
    }
}

fn browse_cs2() {
    let title = wide("请选择 CS2 的安装文件夹（Counter-Strike Global Offensive）");
    let info = BROWSEINFOW {
        hwndOwner: app().ui.hwnd,
        lpszTitle: PCWSTR(title.as_ptr()),
        ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
        ..Default::default()
    };
    let mut buffer = [0u16; 260];
    let picked = unsafe {
        let list = SHBrowseForFolderW(&info);
        let ok = !list.is_null() && SHGetPathFromIDListW(list, &mut buffer).as_bool();
        CoTaskMemFree(Some(list as *const _));
        ok
    };
    if !picked {
        return;
    }
    let path = from_wide(&buffer);
    let a = app();
    if gsi::cfg_status(&path, a.cfg.port) == CfgStatus::NoGame {
        message_box("这个文件夹里没有 game\\csgo\\cfg，不是 CS2 的安装文件夹。");
        return;
    }
    a.cfg.cs2_path = path;
    a.cfg.save();
    write_cfg();
}

fn on_command(id: i32, code: u32) {
    let a = app();
    if a.ui.loading {
        return;
    }
    match (id, code) {
        (ID_MODE, CBN_SELCHANGE_) => {
            let index = send(ID_MODE, CB_GETCURSEL_, 0, 0);
            match index {
                0 | 1 => a.cfg.auto_switch_out = index == 0,
                2 => {}
                _ => return,
            }
            crate::set_enabled(index != 2);
        }
        (ID_RELEASE, _) => {
            a.cfg.release_input = send(ID_RELEASE, BM_GETCHECK_, 0, 0) == 1;
            if !a.cfg.release_input {
                a.releaser.close();
            }
            a.cfg.save();
        }
        (ID_TYPE, CBN_SELCHANGE_) => {
            a.cfg.target_type = match send(ID_TYPE, CB_GETCURSEL_, 0, 0) {
                1 => TargetType::Url,
                2 => TargetType::App,
                _ => TargetType::Window,
            };
            a.cfg.save();
            update_target();
        }
        (ID_WINDOW, CBN_SELCHANGE_) => {
            let index = send(ID_WINDOW, CB_GETCURSEL_, 0, 0);
            a.cfg.window = if index > 0 { a.ui.windows.get(index as usize - 1).cloned() } else { None };
            a.cfg.save();
            set_text(ID_TARGET_HINT, "");
        }
        (ID_REFRESH, _) => fill_windows(),
        (ID_TARGET, EN_CHANGE_) => {
            let mut buffer = [0u16; 2048];
            unsafe { GetWindowTextW(item(ID_TARGET), &mut buffer) };
            let text = from_wide(&buffer).trim().to_string();
            match a.cfg.target_type {
                TargetType::App => a.cfg.app = text,
                TargetType::Url => a.cfg.url = text,
                TargetType::Window => return,
            }
            a.cfg.save();
        }
        (ID_BROWSE, _) => browse_program(),
        (ID_ACTION, CBN_SELCHANGE_) => {
            a.cfg.action = match send(ID_ACTION, CB_GETCURSEL_, 0, 0) {
                1 => Action::Close,
                2 => Action::Pause,
                _ => Action::None,
            };
            a.cfg.save();
        }
        (ID_CS2_BROWSE, _) => browse_cs2(),
        (ID_CFG_WRITE, _) => write_cfg(),
        (ID_DOWNLOAD, _) => crate::open_download(),
        _ => {}
    }
    update_status();
}

fn on_slider() {
    let a = app();
    let delay = send(ID_DELAY, TBM_GETPOS_, 0, 0) as u32;
    let return_delay = send(ID_RETURN, TBM_GETPOS_, 0, 0) as u32;
    set_text(ID_DELAY_VALUE, &format!("{delay} 秒"));
    set_text(ID_RETURN_VALUE, &format!("{return_delay} 秒"));
    if return_delay != a.cfg.return_delay {
        with_engine(|engine, _, platform| engine.on_return_delay_changed(platform));
    }
    if (delay, return_delay) != (a.cfg.delay, a.cfg.return_delay) {
        a.cfg.delay = delay;
        a.cfg.return_delay = return_delay;
        a.cfg.save();
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_COMMAND => on_command((wparam.0 & 0xFFFF) as i32, (wparam.0 >> 16) as u32 & 0xFFFF),
        WM_HSCROLL => on_slider(),
        WM_TIMER => update_status(),
        // White background for all texts, check boxes and sliders; hints in gray.
        // 所有文本、复选框和滑块使用白色背景；提示文字用灰色。
        WM_ERASEBKGND => {
            let dc = HDC(wparam.0 as *mut _);
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            FillRect(dc, &client, GetSysColorBrush(COLOR_WINDOW));
            FillRect(dc, &banner_rect(), banner_brush());
            return LRESULT(1);
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            let dc = HDC(wparam.0 as *mut _);
            let id = GetDlgCtrlID(HWND(lparam.0 as *mut _));
            if matches!(id, ID_BANNER_TITLE | ID_BANNER_DETAIL) {
                let (background, text) = BANNER_COLORS[app().ui.level];
                SetBkColor(dc, COLORREF(background));
                SetTextColor(dc, COLORREF(text));
                return LRESULT(banner_brush().0 as isize);
            }
            SetBkColor(dc, COLORREF(GetSysColor(COLOR_WINDOW)));
            if matches!(id, ID_HINT | ID_FOOTER | ID_TARGET_HINT | ID_CFG_STATUS) {
                SetTextColor(dc, COLORREF(0x006E_6E6E));
            }
            return LRESULT(GetSysColorBrush(COLOR_WINDOW).0 as isize);
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
        }
        WM_DESTROY => {
            let _ = KillTimer(hwnd, STATUS_TIMER);
            app().ui.hwnd = HWND::default();
        }
        _ => return DefWindowProcW(hwnd, message, wparam, lparam),
    }
    LRESULT(0)
}

fn banner_brush() -> HBRUSH {
    let a = app();
    if a.ui.brushes[0].0.is_null() {
        a.ui.brushes = BANNER_COLORS.map(|(background, _)| unsafe { CreateSolidBrush(COLORREF(background)) });
    }
    a.ui.brushes[a.ui.level]
}

fn create_fonts() {
    let a = app();
    if !a.ui.font.0.is_null() {
        return;
    }
    unsafe {
        let mut metrics = NONCLIENTMETRICSW { cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
        let _ = SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS.0,
            metrics.cbSize,
            Some(&mut metrics as *mut _ as *mut _),
            0,
            a.ui.dpi,
        );
        let mut font = metrics.lfMessageFont;
        a.ui.font = CreateFontIndirectW(&font);
        font.lfWeight = 700;
        a.ui.bold = CreateFontIndirectW(&font);
    }
}

/// Opens the settings window, or brings it forward if it is already open.
/// 打开设置窗口；如果已经打开则把它调到前台。
pub fn show() {
    let a = app();
    unsafe {
        if !a.ui.hwnd.0.is_null() {
            let _ = ShowWindow(a.ui.hwnd, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(a.ui.hwnd);
            return;
        }
        a.ui.dpi = GetDpiForSystem();
        create_fonts();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: a.hinstance,
            lpszClassName: CLASS_NAME,
            hbrBackground: GetSysColorBrush(COLOR_WINDOW),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: crate::load_icon(GetSystemMetrics(SM_CXICON)),
            hIconSm: crate::load_icon(GetSystemMetrics(SM_CXSMICON)),
            ..Default::default()
        };
        RegisterClassExW(&class);

        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let mut rect = RECT { left: 0, top: 0, right: px(CLIENT_W), bottom: px(CLIENT_H) };
        let _ = AdjustWindowRectExForDpi(&mut rect, style, false, WINDOW_EX_STYLE(0), a.ui.dpi);
        let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
        let x = (GetSystemMetrics(SM_CXSCREEN) - width) / 2;
        let y = (GetSystemMetrics(SM_CYSCREEN) - height) / 2;
        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            PCWSTR(wide(crate::APP_NAME).as_ptr()),
            style,
            x, y, width, height,
            None, None, a.hinstance, None,
        ) else {
            return;
        };
        a.ui.hwnd = hwnd;
        build();
        load();
        SetTimer(hwnd, STATUS_TIMER, 1000, None);
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = SetForegroundWindow(hwnd);
    }
}

/// Shows the download button once a newer release is known.
/// 得知有新版本后显示下载按钮。
pub fn sync_update() {
    let a = app();
    if let (false, Some(tag)) = (a.ui.hwnd.0.is_null(), &a.update) {
        set_text(ID_FOOTER, &format!("发现新版本 {tag}（当前 v{}）", env!("CARGO_PKG_VERSION")));
        show_item(ID_DOWNLOAD, true);
    }
}

/// Keeps the check box in step when the tray menu toggles the feature.
/// 托盘菜单切换功能开关时，让复选框保持同步。
fn sync_mode_selection() {
    let cfg = &app().cfg;
    let index = if !cfg.enabled { 2 } else if cfg.auto_switch_out { 0 } else { 1 };
    send(ID_MODE, CB_SETCURSEL_, index, 0);
}

pub fn sync_enabled() {
    let a = app();
    if !a.ui.hwnd.0.is_null() {
        sync_mode_selection();
        update_status();
    }
}
