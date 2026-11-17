//! Receives CS2 Game State Integration messages and manages the GSI config file.
//! 接收 CS2 的 Game State Integration 消息，并管理 GSI 配置文件。

use crate::engine::Snapshot;
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CFG_NAME: &str = "gamestate_integration_cstool.cfg";
const MAX_BODY: usize = 1 << 20;

/// Unix time in seconds of the last message from the game; 0 means none yet.
/// 最近一次收到游戏消息的 Unix 时间（秒）；0 表示尚未收到。
pub static LAST_MESSAGE: AtomicU64 = AtomicU64::new(0);

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Binds the configured port, or one of the next nine if it is taken.
/// 绑定配置的端口；被占用时尝试后面的九个端口。
pub fn bind(port: u16) -> Option<(TcpListener, u16)> {
    (port..=port.saturating_add(9)).find_map(|p| TcpListener::bind(("127.0.0.1", p)).ok().map(|l| (l, p)))
}

/// Serves forever; `deliver` is called with each parsed message.
/// 持续服务；每解析出一条消息就调用一次 `deliver`。
pub fn serve(listener: TcpListener, deliver: impl Fn(Snapshot)) {
    for stream in listener.incoming().flatten() {
        if let Some(body) = read_request(stream) {
            if let Ok(value) = serde_json::from_slice::<Value>(&body) {
                LAST_MESSAGE.store(now(), Ordering::Relaxed);
                deliver(parse(&value));
            }
        }
    }
}

fn read_request(mut stream: TcpStream) -> Option<Vec<u8>> {
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut data = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        if data.len() > 16384 {
            return None;
        }
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        data.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&data[..header_end]).to_ascii_lowercase();
    if !head.starts_with("post ") {
        let _ = stream.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return None;
    }
    let length: usize = head.lines().find_map(|l| l.strip_prefix("content-length:"))?.trim().parse().ok()?;
    if length > MAX_BODY {
        return None;
    }
    let mut body = data.split_off(header_end);
    while body.len() < length {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK");
    Some(body)
}

pub fn parse(v: &Value) -> Snapshot {
    let player = &v["player"];
    if !player.as_object().is_some_and(|o| !o.is_empty()) {
        return Snapshot::default();
    }
    let own_id = v["provider"]["steamid"].as_str().unwrap_or("");
    let player_id = player["steamid"].as_str().unwrap_or("");
    let team = player["team"].as_str().unwrap_or("");
    // The message describes someone else while we spectate them.
    // 观战时，消息描述的是被观战的玩家而不是自己。
    let observing = (!own_id.is_empty() && !player_id.is_empty() && own_id != player_id)
        || !player["spectarget"].is_null()
        || player["activity"].as_str() == Some("spectating")
        || !matches!(team, "T" | "CT");
    Snapshot {
        has_player: true,
        health: player["state"]["health"].as_i64().unwrap_or(100),
        observing,
        round_phase: v["round"]["phase"].as_str().unwrap_or("").to_string(),
        map_phase: v["map"]["phase"].as_str().unwrap_or("").to_string(),
    }
}

fn cfg_content(port: u16) -> String {
    format!(
        "\"CSTool\"\r\n{{\r\n    \"uri\" \"http://127.0.0.1:{port}\"\r\n    \"timeout\" \"1.0\"\r\n    \"buffer\"  \"0.0\"\r\n    \"throttle\" \"0.0\"\r\n    \"heartbeat\" \"1.0\"\r\n    \"data\"\r\n    {{\r\n        \"provider\" \"1\"\r\n        \"map\" \"1\"\r\n        \"round\" \"1\"\r\n        \"player_id\" \"1\"\r\n        \"player_state\" \"1\"\r\n    }}\r\n}}\r\n"
    )
}

fn cfg_dir(cs2_path: &str) -> PathBuf {
    Path::new(cs2_path).join("game").join("csgo").join("cfg")
}

#[derive(PartialEq, Clone, Copy)]
pub enum CfgStatus {
    NoGame,
    Missing,
    Outdated,
    Ready,
}

pub fn cfg_status(cs2_path: &str, port: u16) -> CfgStatus {
    let dir = cfg_dir(cs2_path);
    if cs2_path.is_empty() || !dir.is_dir() {
        return CfgStatus::NoGame;
    }
    match std::fs::read_to_string(dir.join(CFG_NAME)) {
        Ok(text) if text == cfg_content(port) => CfgStatus::Ready,
        Ok(_) => CfgStatus::Outdated,
        Err(_) => CfgStatus::Missing,
    }
}

pub fn write_cfg(cs2_path: &str, port: u16) -> bool {
    let dir = cfg_dir(cs2_path);
    !cs2_path.is_empty() && dir.is_dir() && std::fs::write(dir.join(CFG_NAME), cfg_content(port)).is_ok()
}

/// Looks for the CS2 folder in every Steam library listed in libraryfolders.vdf.
/// 在 libraryfolders.vdf 列出的每个 Steam 库中查找 CS2 文件夹。
pub fn find_cs2(steam_path: &str) -> Option<String> {
    let mut libraries = vec![steam_path.replace('/', "\\")];
    let vdf = Path::new(steam_path).join("steamapps").join("libraryfolders.vdf");
    for line in std::fs::read_to_string(vdf).unwrap_or_default().lines() {
        let parts: Vec<&str> = line.split('"').collect();
        if parts.len() >= 4 && parts[1] == "path" {
            libraries.push(parts[3].replace("\\\\", "\\"));
        }
    }
    libraries.into_iter().find_map(|lib| {
        let game = Path::new(&lib).join("steamapps").join("common").join("Counter-Strike Global Offensive");
        cfg_dir(&game.to_string_lossy()).is_dir().then(|| game.to_string_lossy().into_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn own_player_is_not_observing() {
        let s = parse(&json!({
            "provider": {"steamid": "1"},
            "player": {"steamid": "1", "team": "CT", "activity": "playing", "state": {"health": 0}},
            "round": {"phase": "live"}, "map": {"phase": "live"}
        }));
        assert_eq!(s, Snapshot { has_player: true, health: 0, observing: false, round_phase: "live".into(), map_phase: "live".into() });
    }

    #[test]
    fn spectated_player_and_menu_are_observing() {
        let other = parse(&json!({"provider": {"steamid": "1"}, "player": {"steamid": "2", "team": "CT", "state": {"health": 55}}}));
        assert!(other.observing && other.health == 55);
        let menu = parse(&json!({"provider": {"steamid": "1"}, "player": {"steamid": "1", "activity": "menu"}}));
        assert!(menu.observing);
        assert!(!parse(&json!({"provider": {"steamid": "1"}})).has_player);
    }
}
