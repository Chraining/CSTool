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
    fn prepare_input_switch(&mut self) -> bool;
    fn input_held(&self) -> bool;
    fn poll_release(&mut self);
    fn resolve_window(&mut self, sel: &WindowSel, rebind: bool) -> Result<Option<WindowSel>, ()>;
    fn activate_window(&mut self, window: &WindowSel) -> bool;
    fn close_window(&mut self, window: &WindowSel);
    fn send_media_key(&mut self);
    /// Asks the target program itself to play or pause. Returns false if it offers no media
    /// control, in which case the play/pause key is used instead.
    /// 直接要求目标程序播放或暂停。如果它不提供媒体控制则返回 false，此时改用播放/暂停键。
    fn media_command(&mut self, play: bool, exe: Option<&str>) -> bool;
    fn schedule_media_check(&mut self, ms: u32);
    fn game_in_front(&self) -> bool;
    fn is_audio_playing(&mut self, app_exe: Option<&str>) -> bool;
    fn open_url(&mut self, url: &str) -> bool;
    fn browser_exe(&self, url: &str) -> Option<String>;
    fn launch_app(&mut self, path: &str) -> bool;
    fn app_exe_name(&mut self, path: &str) -> String;
    fn activate_browser(&mut self, exe: &str) -> bool;
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
    waiting_for_release: bool,
    played_in_foreground: bool,
    last_manual_focus: Option<bool>,
    previous_buy_phase: Option<bool>,
    restore_pending: bool,
    media_toggled: bool,
    switched_config: Option<Config>,
    browser_target: Option<String>,
    target_opened: bool,
    browser_opened: bool,
    return_timer_active: bool,
    /// How many more times to look for sound before pressing play.
    /// 按下播放键之前，还要再检查几次是否有声音。
    media_checks_left: u8,
    pause_recheck: Option<String>,
    selected_window: Option<WindowSel>,
}

/// Only for programs without direct media control: sound is checked this many times, this far
/// apart, before the play/pause key is pressed, because the key is a toggle and must not be
/// pressed while something is already playing.
/// 仅用于不支持直接媒体控制的程序：按这个间隔检查这么多次是否有声音，之后才按播放/暂停键。
/// 因为这个键是开关，已经在播放时不能按。
const MEDIA_CHECKS: u8 = 4;
const MEDIA_CHECK_INTERVAL_MS: u32 = 250;

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
        let active = cfg.enabled && s.has_player && !s.observing && s.health > 0 && s.map_phase == "live";
        let keep_manual_round = !cfg.auto_switch_out && cfg.enabled && s.has_player && s.map_phase == "live";
        if (!active && !keep_manual_round) || self.pending() {
            self.played_in_foreground = false;
            self.previous_buy_phase = None;
            self.last_manual_focus = None;
        } else if !active {
            if matches!(s.round_phase.as_str(), "live" | "over") {
                self.previous_buy_phase = Some(false);
                self.played_in_foreground |= p.game_in_front();
            }
        } else if matches!(s.round_phase.as_str(), "freezetime" | "live" | "over") {
            let buy_phase = s.round_phase == "freezetime";
            let entered_buy = self.previous_buy_phase.replace(buy_phase) == Some(false) && buy_phase;
            let foreground = p.game_in_front();
            let buy_return = entered_buy && (self.played_in_foreground || !cfg.auto_switch_out);
            let respawn_return = !cfg.auto_switch_out && self.is_dead && matches!(s.round_phase.as_str(), "live" | "freezetime");
            let auto_return = !foreground && (buy_return || respawn_return);
            let user_returned = foreground && self.last_manual_focus == Some(false);
            if auto_return {
                crate::log::write(if entered_buy { "return on buy phase" } else { "return on respawn" });
                p.restore_game_now();
            }
            if cfg.action == Action::Pause && (auto_return || user_returned) {
                let target = match cfg.target_type {
                    TargetType::Window => cfg.window.as_ref().map(|w| exe_name(&w.path)),
                    TargetType::Url => p.browser_exe(&cfg.url),
                    TargetType::App => Self::app_exe(cfg, p),
                };
                if let Some(exe) = target {
                    p.media_command(false, Some(&exe));
                    if crate::win::is_browser_exe(&exe) {
                        self.pause_recheck = Some(exe);
                        p.schedule_media_check(500);
                    }
                }
            }
            self.last_manual_focus = Some(foreground || auto_return);
            if entered_buy {
                self.played_in_foreground = false;
            }
            self.played_in_foreground |= foreground;
        }
        if !cfg.enabled {
            if self.pending() {
                self.restore_context(cfg, p);
            }
            return;
        }
        if self.waiting_for_release && (!s.has_player || (!s.observing && s.health > 0)) {
            self.waiting_for_release = false;
            self.triggered = false;
        }
        if !s.has_player {
            self.is_dead = false;
            return;
        }

        if !s.observing && s.health == 0 && !self.is_dead {
            self.pause_recheck = None;
            self.waiting_for_release = false;
            self.is_dead = true;
            if cfg.auto_switch_out {
                self.triggered = true;
                self.restore_pending = true;
                p.schedule_switch_out(if cfg.delay > 0 { cfg.delay * 1000 } else { 100 });
            }
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

    fn restore_context(&mut self, _cfg: &Config, p: &mut dyn Platform) {
        self.return_timer_active = false;
        p.stop_return_timer();
        self.triggered = false;
        self.restore_pending = false;
        self.media_checks_left = 0;

        let Some(cfg) = self.switched_config.take() else { return };
        let cfg = &cfg;
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
                let exe = self.media_exe(cfg, p);
                if !p.media_command(false, exe.as_deref()) && !exe.as_deref().is_some_and(crate::win::is_browser_exe) && p.is_audio_playing(exe.as_deref()) {
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
                let exe = self.media_exe(cfg, p);
                if self.media_toggled && exe.is_some() && !p.media_command(false, exe.as_deref()) && cfg.target_type != TargetType::Url && !exe.as_deref().is_some_and(crate::win::is_browser_exe) && p.is_audio_playing(exe.as_deref()) {
                    p.send_media_key();
                }
                p.minimize_browser(exe.as_deref());
            }
            (_, Action::None) => {}
        }

        p.schedule_restore_game();
        if self.media_toggled && cfg.action == Action::Pause {
            self.pause_recheck = self.media_exe(cfg, p).filter(|exe| crate::win::is_browser_exe(exe));
        }
        self.media_toggled = false;
        self.target_opened = false;
    }

    pub fn on_restore_timer(&mut self, p: &mut dyn Platform) {
        p.restore_game_now();
        if self.pause_recheck.is_some() {
            p.schedule_media_check(500);
        }
    }

    fn ready_to_leave(&mut self, p: &mut dyn Platform) -> bool {
        self.waiting_for_release = !p.prepare_input_switch();
        if self.waiting_for_release {
            p.poll_release();
        }
        !self.waiting_for_release
    }

    /// Runs when the switch-out delay has elapsed.
    /// 切出延迟结束时执行。
    pub fn on_switch_out_timer(&mut self, cfg: &Config, p: &mut dyn Platform) {
        // Cancelled if the player already respawned during the delay.
        // 如果延迟期间玩家已经复活，则取消切屏。
        if !self.triggered || !cfg.enabled || !cfg.auto_switch_out {
            return;
        }
        if self.waiting_for_release && p.input_held() {
            p.poll_release();
            return;
        }

        if cfg.target_type == TargetType::Window {
            let resolved = match &cfg.window {
                Some(sel) => p.resolve_window(sel, true),
                None => Ok(None),
            };
            match resolved {
                Ok(Some(window)) => {
                    if !self.ready_to_leave(p) {
                        return;
                    }
                    p.minimize_game();
                    if p.activate_window(&window) {
                        self.target_opened = true;
                        self.selected_window = Some(window);
                        self.switched_config = Some(cfg.clone());
                        if cfg.action == Action::Pause {
                            self.start_media(cfg, p);
                        }
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

        if !self.ready_to_leave(p) {
            return;
        }
        p.minimize_game();

        if cfg.target_type == TargetType::App {
            if cfg.app.is_empty() || !p.launch_app(&cfg.app) {
                p.restore_game_now();
                return;
            }
        } else {
            let url = if cfg.url.starts_with("http://") || cfg.url.starts_with("https://") {
                cfg.url.clone()
            } else {
                format!("https://{}", cfg.url)
            };
            // Open the page once; afterwards only bring the browser forward, so tabs do not pile up.
            // 网页只打开一次；之后只把浏览器调到前台，避免标签页越开越多。
            let browser = p.browser_exe(&url);
            if !self.browser_opened || browser != self.browser_target || !browser.as_deref().is_some_and(|exe| p.activate_browser(exe)) {
                if !p.open_url(&url) {
                    p.restore_game_now();
                    return;
                }
                self.browser_opened = true;
            }
            self.browser_target = browser;
        }

        self.switched_config = Some(cfg.clone());
        if cfg.action == Action::Pause {
            self.start_media(cfg, p);
        }
    }

    /// The program whose media is controlled; None means any browser.
    /// 被控制媒体的程序；None 表示任意浏览器。
    fn media_exe(&self, cfg: &Config, p: &mut dyn Platform) -> Option<String> {
        match cfg.target_type {
            TargetType::Window => self.selected_window.as_ref().map(|w| exe_name(&w.path)),
            TargetType::Url => self.browser_target.clone(),
            _ => Self::app_exe(cfg, p),
        }
    }

    fn start_media(&mut self, cfg: &Config, p: &mut dyn Platform) {
        self.media_toggled = true;
        let exe = self.media_exe(cfg, p);
        // Preferred: tell the program to play, at once and without guessing.
        // 首选：直接让该程序播放，立即生效，无需猜测。
        if exe.is_none() || p.media_command(true, exe.as_deref()) || cfg.target_type == TargetType::Url || exe.as_deref().is_some_and(crate::win::is_browser_exe) {
            return;
        }
        // Otherwise fall back to the play/pause key, after checking nothing is playing.
        // 否则退回到播放/暂停键，按之前先确认没有在播放。
        self.media_checks_left = MEDIA_CHECKS;
        p.schedule_media_check(MEDIA_CHECK_INTERVAL_MS);
    }

    /// Runs a little after switching out: presses play only if nothing started by itself.
    /// The media key is a toggle, so it must never be pressed while something is playing.
    /// 切出后稍等片刻执行：只有媒体没有自己开始播放时才按播放键。
    /// 媒体键是开关，所以正在播放时绝不能按。
    pub fn on_media_check_timer(&mut self, cfg: &Config, p: &mut dyn Platform) {
        if let Some(exe) = self.pause_recheck.take() {
            if cfg.enabled && p.game_in_front() {
                p.media_command(false, Some(&exe));
            }
            return;
        }
        if self.media_checks_left == 0 {
            return;
        }
        let exe = self.media_exe(cfg, p);
        if p.is_audio_playing(exe.as_deref()) {
            self.media_checks_left = 0;
            return;
        }
        self.media_checks_left -= 1;
        if self.media_checks_left == 0 {
            p.send_media_key();
        } else {
            p.schedule_media_check(MEDIA_CHECK_INTERVAL_MS);
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
        direct_media: bool,
        away: bool,
        held: bool,
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
        fn input_held(&self) -> bool { self.held }
        fn poll_release(&mut self) { self.log.push("poll".into()); }
        fn prepare_input_switch(&mut self) -> bool {
            !self.held
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
        fn schedule_media_check(&mut self, ms: u32) { self.log.push(format!("media_check_in {ms}")); }
        fn game_in_front(&self) -> bool { !self.away }
        fn media_command(&mut self, play: bool, _exe: Option<&str>) -> bool {
            if self.direct_media {
                self.log.push(if play { "direct_play" } else { "direct_pause" }.into());
            }
            self.direct_media
        }
        fn is_audio_playing(&mut self, _app_exe: Option<&str>) -> bool {
            self.audio_playing
        }
        fn browser_exe(&self, _: &str) -> Option<String> { Some("msedge.exe".into()) }
        fn open_url(&mut self, url: &str) -> bool {
            self.log.push(format!("open_url {url}"));
            true
        }
        fn launch_app(&mut self, path: &str) -> bool {
            self.log.push(format!("launch {path}"));
            true
        }
        fn app_exe_name(&mut self, path: &str) -> String {
            path.to_lowercase()
        }
        fn activate_browser(&mut self, exe: &str) -> bool {
            assert_eq!(exe, "msedge.exe");
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

    /// Lets every pending media check run. 让所有待执行的媒体检查都运行完。
    fn media_checks(e: &mut Engine, cfg: &Config, m: &mut Mock) {
        for _ in 0..MEDIA_CHECKS {
            e.on_media_check_timer(cfg, m);
        }
    }

    fn keys(m: &Mock) -> usize {
        m.log.iter().filter(|l| *l == "media_key").count()
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
    fn rejected_release_waits_and_respawn_cancels() {
        for respawn in [true, false] {
            let (mut e, mut m, cfg) = (Engine::default(), mock(), window_cfg());
            m.held = true;
            e.on_snapshot(&snap(0, false, "live", "live"), &cfg, &mut m);
            e.on_switch_out_timer(&cfg, &mut m);
            e.on_switch_out_timer(&cfg, &mut m);
            assert!(has(&m, "poll") && !has(&m, "minimize_game"));
            if respawn {
                e.on_snapshot(&snap(100, false, "live", "live"), &cfg, &mut m);
            }
            m.held = false;
            e.on_switch_out_timer(&cfg, &mut m);
            assert_eq!(has(&m, "activate_window"), !respawn);
        }
    }

    #[test]
    fn manual_switch_returns_only_on_next_buy_phase() {
        let (mut e, mut m, cfg) = (Engine::default(), mock(), window_cfg());
        let buy = snap(100, false, "freezetime", "live");
        let live = snap(100, false, "live", "live");
        e.on_snapshot(&buy, &cfg, &mut m);
        m.away = true;
        e.on_snapshot(&buy, &cfg, &mut m);
        e.on_snapshot(&live, &cfg, &mut m);
        assert!(m.log.is_empty());
        e.on_snapshot(&buy, &cfg, &mut m);
        e.on_snapshot(&buy, &cfg, &mut m);
        assert_eq!(m.log, ["restore_game_now"]);
    }

    #[test]
    fn manual_returns_pause_selected_browser_only_once_per_return() {
        for at_buy in [true, false] {
            let (mut e, mut m, mut cfg) = (Engine::default(), mock(), window_cfg());
            cfg.window.as_mut().unwrap().path = "msedge.exe".into();
            cfg.action = Action::Pause;
            m.direct_media = true;
            let live = snap(100, false, "live", "live");
            e.on_snapshot(&live, &cfg, &mut m);
            m.away = true;
            e.on_snapshot(&live, &cfg, &mut m);
            m.away = at_buy;
            let next = snap(100, false, if at_buy { "freezetime" } else { "live" }, "live");
            e.on_snapshot(&next, &cfg, &mut m);
            m.away = false;
            e.on_snapshot(&next, &cfg, &mut m);
            assert_eq!(e.pause_recheck.as_deref(), Some("msedge.exe"));
            assert_eq!(m.log.iter().filter(|s| *s == "direct_pause").count(), 1);
            assert_eq!(has(&m, "restore_game_now"), at_buy);
        }
    }

    #[test]
    fn manual_only_survives_death_and_reviewing_a_teammate() {
        let (mut e, mut m, mut cfg) = (Engine::default(), mock(), window_cfg());
        cfg.auto_switch_out = false;
        e.on_snapshot(&snap(100, false, "live", "live"), &cfg, &mut m);
        e.on_snapshot(&snap(0, false, "live", "live"), &cfg, &mut m);
        m.away = true;
        e.on_snapshot(&snap(100, true, "over", "live"), &cfg, &mut m);
        assert!(m.log.is_empty());
        e.on_snapshot(&snap(100, false, "freezetime", "live"), &cfg, &mut m);
        assert_eq!(m.log, ["restore_game_now"]);
    }

    #[test]
    fn return_only_handles_unobserved_focus_and_live_respawns() {
        for respawn in [false, true] {
            let (mut e, mut m, mut cfg) = (Engine::default(), mock(), window_cfg());
            cfg.auto_switch_out = false;
            m.away = true;
            e.on_snapshot(&snap(if respawn { 0 } else { 100 }, false, "live", "live"), &cfg, &mut m);
            let alive = snap(100, false, if respawn { "live" } else { "freezetime" }, "live");
            e.on_snapshot(&alive, &cfg, &mut m);
            e.on_snapshot(&alive, &cfg, &mut m);
            assert_eq!(m.log, ["restore_game_now"]);
        }
    }

    #[test]
    fn background_only_game_does_not_take_focus() {
        let (mut e, mut m, cfg) = (Engine::default(), mock(), window_cfg());
        m.away = true;
        e.on_snapshot(&snap(100, false, "live", "live"), &cfg, &mut m);
        e.on_snapshot(&snap(100, false, "freezetime", "live"), &cfg, &mut m);
        assert!(m.log.is_empty());
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
    fn cancelled_switch_cannot_close_an_existing_target() {
        for target_type in [TargetType::Url, TargetType::App] {
            let cfg = Config { target_type, action: Action::Close, app: "player.exe".into(), ..Config::default() };
            let (mut e, mut m) = (Engine::default(), mock());
            e.on_snapshot(&snap(0, false, "live", "live"), &cfg, &mut m);
            e.on_snapshot(&snap(100, false, "live", "live"), &cfg, &mut m);
            assert!(!m.log.iter().any(|entry| entry.starts_with("kill")));
        }
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
        // Not at once: the media gets a moment to start by itself. 不是立刻：先给媒体一点时间自己开始。
        assert_eq!(keys(&m), 0);
        media_checks(&mut e, &cfg, &mut m);
        assert_eq!(keys(&m), 1);
        m.audio_playing = true;
        e.on_snapshot(&snap(100, false, "freezetime", "live"), &cfg, &mut m);
        assert_eq!(keys(&m), 2);
    }

    #[test]
    fn direct_media_control_is_used_at_once_and_without_the_key() {
        let (mut e, mut m) = (Engine::default(), mock());
        m.direct_media = true;
        let cfg = Config { action: Action::Pause, ..window_cfg() };
        e.on_snapshot(&snap(0, false, "live", "live"), &cfg, &mut m);
        e.on_switch_out_timer(&cfg, &mut m);
        assert!(has(&m, "direct_play"));
        media_checks(&mut e, &cfg, &mut m);
        e.on_snapshot(&snap(100, false, "freezetime", "live"), &cfg, &mut m);
        assert!(has(&m, "direct_pause") && keys(&m) == 0);
    }

    #[test]
    fn browser_pause_rechecks_once_and_new_death_cancels_it() {
        for die_again in [false, true] {
            let (mut e, mut m, mut cfg) = (Engine::default(), mock(), window_cfg());
            cfg.window.as_mut().unwrap().path = "msedge.exe".into();
            cfg.action = Action::Pause;
            m.direct_media = true;
            e.on_snapshot(&snap(0, false, "live", "live"), &cfg, &mut m);
            e.on_switch_out_timer(&cfg, &mut m);
            e.on_snapshot(&snap(100, false, "live", "live"), &cfg, &mut m);
            assert!(!has(&m, "media_check_in 500"));
            e.on_restore_timer(&mut m);
            assert!(has(&m, "media_check_in 500"));
            if die_again {
                e.on_snapshot(&snap(0, false, "live", "live"), &cfg, &mut m);
            }
            media_checks(&mut e, &cfg, &mut m);
            assert_eq!(m.log.iter().filter(|x| x.as_str() == "direct_pause").count(), if die_again { 1 } else { 2 });
            assert_eq!(keys(&m), 0);
        }
    }

    #[test]
    fn media_that_resumes_by_itself_is_left_alone() {
        let (mut e, mut m) = (Engine::default(), mock());
        let cfg = Config { action: Action::Pause, ..window_cfg() };
        e.on_snapshot(&snap(0, false, "live", "live"), &cfg, &mut m);
        e.on_switch_out_timer(&cfg, &mut m);
        e.on_media_check_timer(&cfg, &mut m);
        m.audio_playing = true;
        media_checks(&mut e, &cfg, &mut m);
        assert_eq!(keys(&m), 0);
    }

    #[test]
    fn respawn_before_the_media_check_presses_nothing() {
        let (mut e, mut m) = (Engine::default(), mock());
        let cfg = Config { action: Action::Pause, ..window_cfg() };
        e.on_snapshot(&snap(0, false, "live", "live"), &cfg, &mut m);
        e.on_switch_out_timer(&cfg, &mut m);
        e.on_snapshot(&snap(100, false, "freezetime", "live"), &cfg, &mut m);
        media_checks(&mut e, &cfg, &mut m);
        assert_eq!(keys(&m), 0);
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
        media_checks(&mut e, &cfg, &mut m);
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
            media_checks(&mut e, &cfg, &mut m);
            e.on_snapshot(&snap(100, false, "freezetime", "live"), &cfg, &mut m);
        }
        assert_eq!(m.log.iter().filter(|l| *l == "open_url https://example.com").count(), 1);
        assert!(has(&m, "activate_browser") && has(&m, "minimize_browser"));
        assert_eq!(keys(&m), 0);
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
