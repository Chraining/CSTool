//! Settings stored as JSON in %LOCALAPPDATA%\CSTool\config.json.
//! 设置以 JSON 形式保存在 %LOCALAPPDATA%\CSTool\config.json。

use serde_json::{json, Value};
use std::path::PathBuf;

pub const MAX_DELAY: u32 = 10;
pub const DEFAULT_PORT: u16 = 3210;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TargetType {
    Window,
    Url,
    App,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Action {
    None,
    Close,
    Pause,
}

/// A window the user picked from the list of open applications.
/// 用户从已打开应用列表中选择的窗口。
#[derive(Clone, PartialEq, Debug, Default)]
pub struct WindowSel {
    pub hwnd: isize,
    pub pid: u32,
    pub path: String,
    pub title: String,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub enabled: bool,
    pub target_type: TargetType,
    pub window: Option<WindowSel>,
    pub url: String,
    pub app: String,
    pub delay: u32,
    pub return_delay: u32,
    pub action: Action,
    pub release_input: bool,
    pub port: u16,
    pub cs2_path: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            enabled: true,
            target_type: TargetType::Window,
            window: None,
            url: "https://www.bilibili.com".into(),
            app: String::new(),
            // Four seconds leaves time to watch the whole death camera. 4 秒可以看完完整的死亡视角。
            delay: 4,
            return_delay: 0,
            action: Action::None,
            release_input: false,
            port: DEFAULT_PORT,
            cs2_path: String::new(),
        }
    }
}

fn delay(value: &Value, fallback: u32) -> u32 {
    value.as_i64().unwrap_or(fallback as i64).clamp(0, MAX_DELAY as i64) as u32
}

fn text(value: &Value, fallback: &str) -> String {
    value.as_str().unwrap_or(fallback).to_string()
}

impl Config {
    pub fn path() -> PathBuf {
        let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default();
        base.join("CSTool").join("config.json")
    }

    pub fn load() -> Config {
        let default = Config::default();
        let Ok(raw) = std::fs::read(Self::path()) else { return default };
        // Notepad and PowerShell may add a UTF-8 byte-order mark. 记事本和 PowerShell 可能会加上 UTF-8 BOM。
        let raw = raw.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&raw);
        let Ok(v) = serde_json::from_slice::<Value>(raw) else { return default };
        let window = v.get("window").filter(|w| w.is_object()).map(|w| WindowSel {
            hwnd: w["hwnd"].as_i64().unwrap_or(0) as isize,
            pid: w["pid"].as_u64().unwrap_or(0) as u32,
            path: text(&w["path"], ""),
            title: text(&w["title"], ""),
        });
        Config {
            enabled: v["enabled"].as_bool().unwrap_or(default.enabled),
            target_type: match v["target_type"].as_str() {
                Some("url") => TargetType::Url,
                Some("app") => TargetType::App,
                _ => TargetType::Window,
            },
            window: window.filter(|w| !w.path.is_empty()),
            url: text(&v["url"], &default.url),
            app: text(&v["app"], ""),
            delay: delay(&v["delay"], default.delay),
            return_delay: delay(&v["return_delay"], default.return_delay),
            action: match v["action"].as_str() {
                Some("close") => Action::Close,
                Some("pause") => Action::Pause,
                _ => Action::None,
            },
            release_input: v["release_input"].as_bool().unwrap_or(false),
            port: v["port"].as_u64().filter(|p| (1..=65535).contains(p)).unwrap_or(DEFAULT_PORT as u64) as u16,
            cs2_path: text(&v["cs2_path"], ""),
        }
    }

    pub fn save(&self) {
        let window = match &self.window {
            Some(w) => json!({"hwnd": w.hwnd, "pid": w.pid, "path": w.path, "title": w.title}),
            None => Value::Null,
        };
        let v = json!({
            "enabled": self.enabled,
            "target_type": match self.target_type {
                TargetType::Window => "window",
                TargetType::Url => "url",
                TargetType::App => "app",
            },
            "window": window,
            "url": self.url,
            "app": self.app,
            "delay": self.delay,
            "return_delay": self.return_delay,
            "action": match self.action {
                Action::None => "none",
                Action::Close => "close",
                Action::Pause => "pause",
            },
            "release_input": self.release_input,
            "port": self.port,
            "cs2_path": self.cs2_path,
        });
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, serde_json::to_vec_pretty(&v).unwrap_or_default());
    }
}
