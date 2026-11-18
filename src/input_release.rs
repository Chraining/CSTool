//! Releases held keys and mouse buttons before a focus switch, then ignores them
//! until they are physically released (at most 5 seconds). No input is recorded.
//! Hooks exist only during that short quarantine, never during normal play.
//! 切换焦点前松开按住的键盘键和鼠标键，并在它们被实际松开之前忽略它们（最长 5 秒）。
//! 不记录任何输入。钩子只在这段短暂的隔离期内存在，正常游戏时不会安装。

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Tags our own synthetic key-ups so the hooks let them through.
/// 标记我们自己合成的松键事件，让钩子放行。
const MARKER: usize = 0x4353_3254;
const QUARANTINE: Duration = Duration::from_secs(5);
const MOUSE_KEYS: [u32; 5] = [1, 2, 4, 5, 6];

/// Virtual keys that stay ignored until physically released.
/// 在实际松开之前保持被忽略的虚拟键。
static BLOCKED: Mutex<Vec<u32>> = Mutex::new(Vec::new());

fn blocked() -> std::sync::MutexGuard<'static, Vec<u32>> {
    BLOCKED.lock().unwrap_or_else(|e| e.into_inner())
}

/// Returns true if the event must be swallowed. A physical key-up ends the quarantine for that key.
/// 返回 true 表示该事件必须被吞掉。实际的松键会结束该键的隔离。
fn suppress(key: u32, is_up: bool, injected: bool) -> bool {
    let mut keys = blocked();
    let Some(index) = keys.iter().position(|&k| k == key) else { return false };
    if is_up && !injected {
        keys.swap_remove(index);
    }
    true
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let data = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let is_up = matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP);
        if data.dwExtraInfo != MARKER && suppress(data.vkCode, is_up, data.flags.0 & LLKHF_INJECTED.0 != 0) {
            return LRESULT(1);
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let data = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        let button = match wparam.0 as u32 {
            WM_LBUTTONDOWN => Some((1, false)),
            WM_LBUTTONUP => Some((1, true)),
            WM_RBUTTONDOWN => Some((2, false)),
            WM_RBUTTONUP => Some((2, true)),
            WM_MBUTTONDOWN => Some((4, false)),
            WM_MBUTTONUP => Some((4, true)),
            message @ (WM_XBUTTONDOWN | WM_XBUTTONUP) => {
                Some((if data.mouseData >> 16 == 1 { 5 } else { 6 }, message == WM_XBUTTONUP))
            }
            _ => None,
        };
        if let Some((key, is_up)) = button {
            if data.dwExtraInfo != MARKER && suppress(key, is_up, data.flags & 1 != 0) {
                return LRESULT(1);
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

fn release_event(vk: u32) -> INPUT {
    let mouse = |flags: MOUSE_EVENT_FLAGS, data: u32| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { mouseData: data, dwFlags: flags, dwExtraInfo: MARKER, ..Default::default() } },
    };
    match vk {
        1 => mouse(MOUSEEVENTF_LEFTUP, 0),
        2 => mouse(MOUSEEVENTF_RIGHTUP, 0),
        4 => mouse(MOUSEEVENTF_MIDDLEUP, 0),
        5 => mouse(MOUSEEVENTF_XUP, 1),
        6 => mouse(MOUSEEVENTF_XUP, 2),
        _ => {
            let extended = matches!(vk, 0xA3 | 0xA5 | 0x21..=0x28 | 0x2D | 0x2E | 0x5B | 0x5C | 0x6F);
            let flags = if extended { KEYEVENTF_KEYUP | KEYEVENTF_EXTENDEDKEY } else { KEYEVENTF_KEYUP };
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk as u16), dwFlags: flags, dwExtraInfo: MARKER, ..Default::default() },
                },
            }
        }
    }
}

#[derive(Default)]
pub struct Releaser {
    thread: Option<JoinHandle<()>>,
    thread_id: Arc<AtomicU32>,
    stopping: Arc<AtomicBool>,
}

impl Releaser {
    pub fn held() -> impl Iterator<Item = u32> {
        (1u32..255)
            .filter(|vk| !matches!(vk, 0x10..=0x12) && unsafe { GetAsyncKeyState(*vk as i32) } as u16 & 0x8000 != 0)
    }

    /// Releases everything currently held. Err describes what failed, for the log.
    /// 松开当前按住的所有按键。Err 描述失败的原因，供日志使用。
    pub fn release(&mut self) -> Result<(), String> {
        // Generic Shift/Ctrl/Alt duplicate the left/right keys, so they are skipped.
        // 通用的 Shift/Ctrl/Alt 与左右键重复，所以跳过。
        let held: Vec<_> = Self::held().collect();
        if held.is_empty() {
            return Ok(());
        }
        crate::log::write(&format!("held input count: {}", held.len()));
        let mut all = blocked().clone();
        all.extend(held.iter().filter(|vk| !all.contains(vk)).copied().collect::<Vec<_>>());
        self.close();
        *blocked() = all;
        self.start_hooks()?;
        let events: Vec<INPUT> = held.iter().map(|&vk| release_event(vk)).collect();
        let sent = unsafe { SendInput(&events, std::mem::size_of::<INPUT>() as i32) } as usize;
        if sent != events.len() {
            let error = unsafe { windows::Win32::Foundation::GetLastError() };
            // Let physical releases through when a synthetic release failed.
            // 合成松键失败时，放行用户实际松开按键的事件。
            self.close();
            return Err(format!("SendInput sent {sent} of {}, last error {error:?}", events.len()));
        }
        Ok(())
    }

    pub fn close(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.stopping.store(true, Ordering::SeqCst);
            unsafe {
                let _ = PostThreadMessageW(self.thread_id.load(Ordering::SeqCst), WM_QUIT, WPARAM(0), LPARAM(0));
            }
            let _ = thread.join();
        }
        blocked().clear();
    }

    fn start_hooks(&mut self) -> Result<(), String> {
        let stopping = Arc::new(AtomicBool::new(false));
        let thread_id = Arc::new(AtomicU32::new(0));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let (stop, id) = (stopping.clone(), thread_id.clone());
        self.stopping = stopping;
        self.thread_id = thread_id;
        self.thread = Some(std::thread::spawn(move || unsafe { hook_thread(stop, id, ready_tx) }));
        let result = ready_rx.recv_timeout(Duration::from_secs(2)).unwrap_or_else(|_| Err("hook thread did not answer in 2 s".into()));
        if result.is_err() {
            self.close();
        }
        result
    }
}

unsafe fn hook_thread(stopping: Arc<AtomicBool>, thread_id: Arc<AtomicU32>, ready: mpsc::Sender<Result<(), String>>) {
    let mut message = MSG::default();
    // Creates this thread's message queue. 创建本线程的消息队列。
    let _ = PeekMessageW(&mut message, HWND::default(), 0, 0, PM_NOREMOVE);
    thread_id.store(GetCurrentThreadId(), Ordering::SeqCst);

    let module = GetModuleHandleW(None).unwrap_or_default();
    let (need_keyboard, need_mouse) = {
        let keys = blocked();
        (keys.iter().any(|k| !MOUSE_KEYS.contains(k)), keys.iter().any(|k| MOUSE_KEYS.contains(k)))
    };
    let mut hooks = Vec::new();
    let mut result = Ok(());
    if need_keyboard {
        match SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), module, 0) {
            Ok(hook) => hooks.push(hook),
            Err(error) => result = Err(format!("keyboard hook: {error}")),
        }
    }
    if need_mouse && result.is_ok() {
        match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), module, 0) {
            Ok(hook) => hooks.push(hook),
            Err(error) => result = Err(format!("mouse hook: {error}")),
        }
    }
    let ok = result.is_ok();
    let _ = ready.send(result);
    if ok {
        // The timer wakes the loop so it can notice the deadline and an empty list.
        // 定时器唤醒循环，让它能发现超时和列表已空。
        SetTimer(HWND::default(), 0, 100, None);
        let deadline = Instant::now() + QUARANTINE;
        while !stopping.load(Ordering::SeqCst)
            && !blocked().is_empty()
            && Instant::now() < deadline
            && GetMessageW(&mut message, HWND::default(), 0, 0).0 > 0
        {}
    }
    for hook in hooks {
        let _ = UnhookWindowsHookEx(hook);
    }
    if !stopping.load(Ordering::SeqCst) {
        blocked().clear();
    }
}

