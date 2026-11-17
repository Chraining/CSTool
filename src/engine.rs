//! Death-switch state machine, ported from CS2Toolkit's visual_handler.py ("Boss Key").
//! It contains no Windows calls, so it can be unit-tested.
//! 死亡切屏状态机，移植自 CS2Toolkit 的 visual_handler.py（“Boss Key”）。
//! 这里不包含任何 Windows 调用，因此可以做单元测试。

use crate::config::{Action, Config, TargetType, WindowSel};

/// The fields of one GSI message that the switch logic needs.
/// 切屏逻辑需要的一条 GSI 消息中的字段。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub has_player: bool,
    pub health: i64,
    pub observing: bool,
    pub round_phase: String,
    pub map_phase: String,
}

/// Everything the state machine asks the operating system to do.
/// 状态机要求操作系统执行的所有操作。
pub trait Platform {
    fn schedule_switch_out(&mut self, ms: u32);
    fn start_return_timer(&mut self, ms: u32);
    fn stop_return_timer(&mut self);
    fn schedule_restore_game(&mut self);
    fn restore_game_now(&mut self);
    fn minimize_game(&mut self);
    /// Releases held keys if the option is on; false cancels the switch.
    /// 选项开启时释放按住的按键；返回 false 表示取消本次切换。
    fn prepare_input_switch(&mut self) -> bool;
    fn resolve_window(&mut self, sel: &WindowSel, rebind: bool) -> Result<Option<WindowSel>, ()>;
    fn activate_window(&mut self, window: &WindowSel) -> bool;
    fn close_window(&mut self, window: &WindowSel);
    fn send_media_key(&mut self);
    fn is_audio_playing(&mut self, app_exe: Option<&str>) -> bool;
    fn open_url(&mut self, url: &str);
    fn launch_app(&mut self, path: &str) -> bool;
    fn app_exe_name(&mut self, path: &str) -> String;
    fn activate_browser(&mut self) -> bool;
    fn minimize_browser(&mut self, app_exe: Option<&str>);
    fn kill_browsers(&mut self);
    fn kill_app(&mut self, exe: &str);
    fn notify_failure(&mut self, message: &str);
}

/// Lower-case file name of a path. 路径的小写文件名。
fn exe_name(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_lowercase()
}

#[derive(Default)]
pub struct Engine {
    is_dead: bool,
    triggered: bool,
    restore_pending: bool,
    media_toggled: bool,
    target_opened: bool,
    browser_opened: bool,
    return_timer_active: bool,
    selected_window: Option<WindowSel>,
}

impl Engine {
    fn pending(&self) -> bool {
        self.triggered || self.restore_pending || self.media_toggled
    }

    fn app_exe(cfg: &Config, p: &mut dyn Platform) -> Option<String> {
        if cfg.target_type == TargetType::App && !cfg.app.is_empty() {
            Some(p.app_exe_name(&cfg.app))
        } else {
            None
        }
    }

    pub fn on_snapshot(&mut self, s: &Snapshot, cfg: &Config, p: &mut dyn Platform) {
        if !cfg.enabled {
            if self.pending() {
                self.restore_context(cfg, p);
            }
            return;
        }
        if !s.has_player {
            self.is_dead = false;
            return;
        }

        if !s.observing && s.health == 0 && !self.is_dead {
            self.is_dead = true;
            self.triggered = true;
            self.restore_pending = true;
            p.schedule_switch_out(if cfg.delay > 0 { cfg.delay * 1000 } else { 100 });
        } else if !s.observing && s.health > 0 && self.is_dead {
            // Unlike the original, a spectated teammate's health does not clear the flag.
            // 与原版不同：观战队友的血量不会清除这个标志。
            self.is_dead = false;
        }

        if self.should_restore(s) {
            self.schedule_return(cfg, p);
        } else if self.return_timer_active {
            self.return_timer_active = false;
            p.stop_return_timer();
        }
    }

    fn should_restore(&self, s: &Snapshot) -> bool {
        if !self.pending() || s.observing || s.map_phase == "intermission" {
            return false;
        }
        s.health > 0 && matches!(s.round_phase.as_str(), "freezetime" | "" | "live")
    }

    fn schedule_return(&mut self, cfg: &Config, p: &mut dyn Platform) {
        if self.return_timer_active {
            return;
        }
        if cfg.return_delay > 0 {
            // A switch-out still waiting for its delay must not run after respawn.
            // 复活后不再执行尚未到时的延迟切出。
            self.triggered = false;
            self.restore_pending = true;
            self.return_timer_active = true;
            p.start_return_timer(cfg.return_delay * 1000);
        } else {
            self.restore_context(cfg, p);
        }
    }

    pub fn on_return_timer(&mut self, cfg: &Config, p: &mut dyn Platform) {
        if self.return_timer_active {
            self.restore_context(cfg, p);
        }
    }

    /// The return delay changed in settings: restart the countdown on the next message.
    /// 设置中的切回延迟改变：下一条消息到来时重新计时。
    pub fn on_return_delay_changed(&mut self, p: &mut dyn Platform) {
        if self.return_timer_active {
            self.return_timer_active = false;
            p.stop_return_timer();
        }
    }

    fn restore_context(&mut self, cfg: &Config, p: &mut dyn Platform) {
        self.return_timer_active = false;
        p.stop_return_timer();
        self.triggered = false;
        self.restore_pending = false;

        match (cfg.target_type, cfg.action) {
            (TargetType::Window, Action::Close) if self.target_opened => {
                if let Some(selected) = self.selected_window.clone() {
                    match p.resolve_window(&selected, false) {
                        Ok(Some(window)) => p.close_window(&window),
                        Ok(None) => {}
                        Err(()) => p.notify_failure("无法关闭所选窗口，请手动关闭。"),
                    }
                }
            }
            (TargetType::Window, Action::Pause) if self.media_toggled => {
                // Pause only if it is still playing, so a video the user paused by hand is not resumed.
                // 只有仍在播放时才暂停，避免把用户手动暂停的视频重新播放。
                let exe = self.selected_window.as_ref().map(|w| exe_name(&w.path));
                if p.is_audio_playing(exe.as_deref()) {
                    p.send_media_key();
                }
            }
            (TargetType::Window, _) => {}
            (TargetType::Url, Action::Close) => {
                p.kill_browsers();
                self.browser_opened = false;
            }
            (TargetType::App, Action::Close) => {
                if let Some(exe) = Self::app_exe(cfg, p) {
                    p.kill_app(&exe);
                }
                self.browser_opened = false;
            }
            (_, Action::Pause) => {
                let exe = Self::app_exe(cfg, p);
                if self.media_toggled && p.is_audio_playing(exe.as_deref()) {
                    p.send_media_key();
                }
                p.minimize_browser(exe.as_deref());
            }
            (_, Action::None) => {}
        }

        p.schedule_restore_game();
        self.media_toggled = false;
        self.target_opened = false;
    }

    /// Runs when the switch-out delay has elapsed.
    /// 切出延迟结束时执行。
    pub fn on_switch_out_timer(&mut self, cfg: &Config, p: &mut dyn Platform) {
        // Cancelled if the player already respawned during the delay.
        // 如果延迟期间玩家已经复活，则取消切屏。
        if !self.triggered {
            return;
        }

        if cfg.target_type == TargetType::Window {
            let resolved = match &cfg.window {
                Some(sel) => p.resolve_window(sel, true),
                None => Ok(None),
            };
            match resolved {
                Ok(Some(window)) => {
                    if !p.prepare_input_switch() {
                        return;
                    }
                    p.minimize_game();
                    if p.activate_window(&window) {
                        self.target_opened = true;
                        if cfg.action == Action::Pause {
                            // The media key is a toggle: press it only when nothing is playing,
                            // otherwise it would pause media that is already playing.
                            // 媒体键是开关：只有没在播放时才按，否则会把正在播放的媒体暂停。
                            if !p.is_audio_playing(Some(&exe_name(&window.path))) {
                                p.send_media_key();
                            }
                            self.media_toggled = true;
                        }
                        self.selected_window = Some(window);
                    } else {
                        p.restore_game_now();
                        p.notify_failure("Windows 未允许切换到所选应用，请重试。");
                    }
                }
                Ok(None) => {
                    self.selected_window = None;
                    p.notify_failure("所选应用已关闭或有多个同名窗口，请在设置中重新选择。");
                }
                Err(()) => {
                    p.restore_game_now();
                    p.notify_failure("无法读取应用窗口，请重新选择应用。");
                }
            }
            return;
        }

        if !p.prepare_input_switch() {
            return;
        }
        p.minimize_game();

        if cfg.target_type == TargetType::App {
            if !cfg.app.is_empty() && p.launch_app(&cfg.app) {
                self.target_opened = true;
            }
        } else {
            let url = if cfg.url.starts_with("http://") || cfg.url.starts_with("https://") {
                cfg.url.clone()
            } else {
                format!("https://{}", cfg.url)
            };
            // Open the page once; afterwards only bring the browser forward, so tabs do not pile up.
            // 网页只打开一次；之后只把浏览器调到前台，避免标签页越开越多。
            if !self.browser_opened || !p.activate_browser() {
                p.open_url(&url);
                self.browser_opened = true;
            }
        }

        if cfg.action == Action::Pause {
            // Send "play" only when nothing is playing yet.
            // 只有当前没有声音在播放时才发送“播放”。
            let exe = Self::app_exe(cfg, p);
            if !p.is_audio_playing(exe.as_deref()) {
                p.send_media_key();
            }
            self.media_toggled = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Mock {
        log: Vec<String>,
        activate_ok: bool,
        window_found: bool,
        audio_playing: bool,
    }

    impl Platform for Mock {
        fn schedule_switch_out(&mut self, ms: u32) {
            self.log.push(format!("switch_out_in {ms}"));
        }
        fn start_return_timer(&mut self, ms: u32) {
            self.log.push(format!("return_in {ms}"));
        }
        fn stop_return_timer(&mut self) {
            self.log.push("stop_return".into());
        }
        fn schedule_restore_game(&mut self) {
            self.log.push("restore_game".into());
        }
        fn restore_game_now(&mut self) {
            self.log.push("restore_game_now".into());
        }
        fn minimize_game(&mut self) {
            self.log.push("minimize_game".into());
        }
        fn prepare_input_switch(&mut self) -> bool {
            true
        }
        fn resolve_window(&mut self, sel: &WindowSel, _rebind: bool) -> Result<Option<WindowSel>, ()> {
            Ok(self.window_found.then(|| sel.clone()))
        }
        fn activate_window(&mut self, _window: &WindowSel) -> bool {
            self.log.push("activate_window".into());
            self.activate_ok
        }
        fn close_window(&mut self, _window: &WindowSel) {
            self.log.push("close_window".into());
        }
        fn send_media_key(&mut self) {
            self.log.push("media_key".into());
        }
        fn is_audio_playing(&mut self, _app_exe: Option<&str>) -> bool {
            self.audio_playing
        }
        fn open_url(&mut self, url: &str) {
            self.log.push(format!("open_url {url}"));
        }
        fn launch_app(&mut self, path: &str) -> bool {
            self.log.push(format!("launch {path}"));
            true
        }
        fn app_exe_name(&mut self, path: &str) -> String {
            path.to_lowercase()
        }
        fn activate_browser(&mut self) -> bool {
            self.log.push("activate_browser".into());
            true
        }
        fn minimize_browser(&mut self, _app_exe: Option<&str>) {
            self.log.push("minimize_browser".into());
        }
        fn kill_browsers(&mut self) {
            self.log.push("kill_browsers".into());
        }
        fn kill_app(&mut self, exe: &str) {
            self.log.push(format!("kill {exe}"));
        }
        fn notify_failure(&mut self, message: &str) {
            self.log.push(format!("fail {message}"));
        }
    }

    fn snap(health: i64, observing: bool, round: &str, map: &str) -> Snapshot {
        Snapshot { has_player: true, health, observing, round_phase: round.into(), map_phase: map.into() }
    }

    fn window_cfg() -> Config {
        Config {
            window: Some(WindowSel { hwnd: 1, pid: 2, path: "c:\\a.exe".into(), title: "A".into() }),
            delay: 0,
            ..Config::default()
        }
    }

    fn mock() -> Mock {
        Mock { activate_ok: true, window_found: true, ..Mock::default() }
    }

    fn has(m: &Mock, entry: &str) -> bool {
        m.log.iter().any(|l| l == entry)
    }

    #[test]
    fn death_switches_out_and_next_round_switches_back() {
        let (mut e, mut m, cfg) = (Engine::default(), mock(), window_cfg());
        e.on_snapshot(&snap(100, false, "live", "live"), &cfg, &mut m);
        assert!(m.log.is_empty());
        e.on_snapshot(&snap(0, false, "live", "live"), &cfg, &mut m);
        assert_eq!(m.log, ["switch_out_in 100"]);
        e.on_switch_out_timer(&cfg, &mut m);
        assert!(has(&m, "minimize_game") && has(&m, "activate_window"));
        // Spectating a living teammate must not switch back. 观战存活的队友不应切回。
        e.on_snapshot(&snap(80, true, "live", "live"), &cfg, &mut m);
        assert!(!has(&m, "restore_game"));
        // Round over, still dead: no switch back. 回合结束但仍死亡：不切回。
        e.on_snapshot(&snap(0, false, "over", "live"), &cfg, &mut m);
        assert!(!has(&m, "restore_game"));
        e.on_snapshot(&snap(100, false, "freezetime", "live"), &cfg, &mut m);
        assert!(has(&m, "restore_game"));
    }

    #[test]
    fn death_fires_only_once_and_not_while_observing() {
        let (mut e, mut m, cfg) = (Engine::default(), mock(), window_cfg());
        e.on_snapshot(&snap(0, true, "live", "live"), &cfg, &mut m);
        assert!(m.log.is_empty());
        e.on_snapshot(&snap(0, false, "live", "live"), &cfg, &mut m);
        e.on_snapshot(&snap(0, false, "live", "live"), &cfg, &mut m);
        e.on_snapshot(&snap(90, true, "live", "live"), &cfg, &mut m);
        e.on_snapshot(&snap(0, false, "live", "live"), &cfg, &mut m);
        assert_eq!(m.log.iter().filter(|l| l.starts_with("switch_out_in")).count(), 1);
    }

    #[test]
    fn halftime_does_not_switch_back() {
        let (mut e, mut m, cfg) = (Engine::default(), mock(), window_cfg());
        e.on_snapshot(&snap(0, false, "live", "live"), &cfg, &mut m);
        e.on_switch_out_timer(&cfg, &mut m);
        e.on_snapshot(&snap(100, false, "freezetime", "intermission"), &cfg, &mut m);
        assert!(!has(&m, "restore_game"));
        e.on_snapshot(&snap(100, false, "freezetime", "live"), &cfg, &mut m);
        assert!(has(&m, "restore_game"));
    }

    #[test]
    fn respawn_during_out_delay_cancels_the_switch() {
        let (mut e, mut m) = (Engine::default(), mock());
        let cfg = Config { delay: 5, return_delay: 2, ..window_cfg() };
        e.on_snapshot(&snap(0, false, "live", "live"), &cfg, &mut m);
        assert_eq!(m.log, ["switch_out_in 5000"]);
        e.on_snapshot(&snap(100, false, "live", "live"), &cfg, &mut m);
        assert!(has(&m, "return_in 2000"));
        e.on_switch_out_timer(&cfg, &mut m);
        assert!(!has(&m, "minimize_game"));
    }

    #[test]
    fn return_delay_restarts_when_conditions_break() {
        let (mut e, mut m) = (Engine::default(), mock());
        let cfg = Config { return_delay: 3, ..window_cfg() };
        e.on_snapshot(&snap(0, false, "live", "live"), &cfg, &mut m);
        e.on_switch_out_timer(&cfg, &mut m);
        e.on_snapshot(&snap(100, false, "freezetime", "live"), &cfg, &mut m);
        e.on_snapshot(&snap(100, false, "freezetime", "live"), &cfg, &mut m);
        assert_eq!(m.log.iter().filter(|l| *l == "return_in 3000").count(), 1);
        e.on_snapshot(&snap(100, true, "freezetime", "live"), &cfg, &mut m);
        assert!(has(&m, "stop_return"));
        e.on_snapshot(&snap(100, false, "freezetime", "live"), &cfg, &mut m);
        assert_eq!(m.log.iter().filter(|l| *l == "return_in 3000").count(), 2);
        e.on_return_timer(&cfg, &mut m);
        assert!(has(&m, "restore_game"));
    }

    #[test]
    fn missing_window_reports_failure_without_minimizing() {
        let (mut e, mut m, cfg) = (Engine::default(), Mock::default(), window_cfg());
        e.on_snapshot(&snap(0, false, "live", "live"), &cfg, &mut m);
        e.on_switch_out_timer(&cfg, &mut m);
        assert!(!has(&m, "minimize_game"));
        assert!(m.log.iter().any(|l| l.starts_with("fail ")));
    }

    #[test]
    fn close_action_closes_the_selected_window() {
        let (mut e, mut m) = (Engine::default(), mock());
        let cfg = Config { action: Action::Close, ..window_cfg() };
        e.on_snapshot(&snap(0, false, "live", "live"), &cfg, &mut m);
        e.on_switch_out_timer(&cfg, &mut m);
        e.on_snapshot(&snap(100, false, "live", "live"), &cfg, &mut m);
        assert!(has(&m, "close_window") && has(&m, "restore_game"));
    }

    #[test]
    fn media_plays_on_switch_out_and_pauses_on_return() {
        let (mut e, mut m) = (Engine::default(), mock());
        let cfg = Config { action: Action::Pause, ..window_cfg() };
        e.on_snapshot(&snap(0, false, "live", "live"), &cfg, &mut m);
        e.on_switch_out_timer(&cfg, &mut m);
        assert_eq!(m.log.iter().filter(|l| *l == "media_key").count(), 1);
        m.audio_playing = true;
        e.on_snapshot(&snap(100, false, "freezetime", "live"), &cfg, &mut m);
        assert_eq!(m.log.iter().filter(|l| *l == "media_key").count(), 2);
    }

    #[test]
    fn media_key_is_not_pressed_when_it_would_do_the_opposite() {
        // Already playing at death: no key. Paused by hand before return: no key.
        // 死亡时已在播放：不按键。切回前被手动暂停：不按键。
        let (mut e, mut m) = (Engine::default(), mock());
        let cfg = Config { action: Action::Pause, ..window_cfg() };
        m.audio_playing = true;
        e.on_snapshot(&snap(0, false, "live", "live"), &cfg, &mut m);
        e.on_switch_out_timer(&cfg, &mut m);
        m.audio_playing = false;
        e.on_snapshot(&snap(100, false, "freezetime", "live"), &cfg, &mut m);
        assert!(!has(&m, "media_key") && has(&m, "restore_game"));
    }

    #[test]
    fn url_opens_once_then_reuses_the_browser() {
        let (mut e, mut m) = (Engine::default(), mock());
        let cfg = Config { target_type: TargetType::Url, url: "example.com".into(), action: Action::Pause, ..Config::default() };
        for _ in 0..2 {
            e.on_snapshot(&snap(0, false, "live", "live"), &cfg, &mut m);
            e.on_switch_out_timer(&cfg, &mut m);
            e.on_snapshot(&snap(100, false, "freezetime", "live"), &cfg, &mut m);
        }
        assert_eq!(m.log.iter().filter(|l| *l == "open_url https://example.com").count(), 1);
        assert!(has(&m, "activate_browser") && has(&m, "minimize_browser"));
        assert_eq!(m.log.iter().filter(|l| *l == "media_key").count(), 2);
    }

    #[test]
    fn disabling_while_switched_out_restores_the_game() {
        let (mut e, mut m, mut cfg) = (Engine::default(), mock(), window_cfg());
        e.on_snapshot(&snap(0, false, "live", "live"), &cfg, &mut m);
        e.on_switch_out_timer(&cfg, &mut m);
        cfg.enabled = false;
        e.on_snapshot(&snap(0, false, "live", "live"), &cfg, &mut m);
        assert!(has(&m, "restore_game"));
    }
}
