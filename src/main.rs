// Jitter v0.4 — idle-aware presence keeper with a system-tray menu.
//
// Presence indicators key off the OS input-idle timer, so a 1px out-and-back
// nudge resets it. Jitter only nudges when you're actually idle, during work
// hours, on an organic cadence — never while you're using the machine.
//
// Tray menu: Status • Runtime/nudges • Enabled (toggles + swaps icon) •
//            Start at login • Quit.
//
// Linux build deps: sudo apt install libgtk-3-dev libxdo-dev libappindicator3-dev
// (or the libayatana-appindicator3-dev equivalent).

// On Windows, build as a GUI-subsystem app in release so no console window opens
// (a console owns the process, so closing it would kill the tray app). Debug
// builds keep the console so stdout/stderr stay visible during development.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use chrono::{Datelike, Local, Timelike, Weekday};
use enigo::{Coordinate, Enigo, Mouse, Settings};
use serde::{Deserialize, Serialize};
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};
#[cfg(not(target_os = "macos"))]
use user_idle::UserIdle;

// macOS idle detection.
//
// `user-idle` 0.1 has no working macOS path: on any `cfg(unix)` target it
// queries the freedesktop/GNOME/KDE ScreenSaver over D-Bus, which doesn't exist
// on macOS, so it always errors and idle reads as 0 — Jitter would never nudge.
// Instead we call CoreGraphics directly for the real HID input-idle time.
#[cfg(target_os = "macos")]
mod mac_idle {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceSecondsSinceLastEventType(state_id: i32, event_type: u32) -> f64;
    }

    pub fn idle_seconds() -> u64 {
        // kCGEventSourceStateHIDSystemState = 1, kCGAnyInputEventType = ~0.
        const HID_SYSTEM_STATE: i32 = 1;
        const ANY_INPUT_EVENT: u32 = 0xFFFF_FFFF;
        let secs = unsafe {
            CGEventSourceSecondsSinceLastEventType(HID_SYSTEM_STATE, ANY_INPUT_EVENT)
        };
        if secs.is_finite() && secs > 0.0 {
            secs as u64
        } else {
            0
        }
    }
}

// ---------- config ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Config {
    interval_secs: u64,
    jitter_secs: u64,
    idle_threshold_secs: u64,
    active_start_hour: u32,
    active_end_hour: u32,
    weekdays_only: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            interval_secs: 45,
            jitter_secs: 20,
            idle_threshold_secs: 25,
            active_start_hour: 8,
            active_end_hour: 18,
            weekdays_only: true,
        }
    }
}

fn config_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("jitter.toml")))
        .unwrap_or_else(|| PathBuf::from("jitter.toml"))
}

fn load_config() -> Config {
    let path = config_path();
    match fs::read_to_string(&path) {
        Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
            eprintln!("config parse error ({e}); using defaults");
            Config::default()
        }),
        Err(_) => {
            let cfg = Config::default();
            if let Ok(s) = toml::to_string_pretty(&cfg) {
                let _ = fs::write(&path, s);
            }
            cfg
        }
    }
}

// ---------- scheduling / timing ----------

fn within_active_window(cfg: &Config) -> bool {
    let now = Local::now();
    if cfg.weekdays_only && matches!(now.weekday(), Weekday::Sat | Weekday::Sun) {
        return false;
    }
    if cfg.active_start_hour == cfg.active_end_hour {
        return true;
    }
    let h = now.hour();
    if cfg.active_start_hour < cfg.active_end_hour {
        h >= cfg.active_start_hour && h < cfg.active_end_hour
    } else {
        h >= cfg.active_start_hour || h < cfg.active_end_hour
    }
}

fn next_interval(cfg: &Config) -> Duration {
    if cfg.jitter_secs == 0 {
        return Duration::from_secs(cfg.interval_secs.max(5));
    }
    let span = cfg.jitter_secs * 2 + 1;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let offset = (nanos % span) as i64 - cfg.jitter_secs as i64;
    let secs = (cfg.interval_secs as i64 + offset).max(5) as u64;
    Duration::from_secs(secs)
}

fn idle_secs() -> u64 {
    #[cfg(target_os = "macos")]
    {
        mac_idle::idle_seconds()
    }
    #[cfg(not(target_os = "macos"))]
    {
        UserIdle::get_time().map(|i| i.as_seconds() as u64).unwrap_or(0)
    }
}

fn nudge(enigo: &mut Enigo) {
    let _ = enigo.move_mouse(1, 0, Coordinate::Rel);
    thread::sleep(Duration::from_millis(40));
    let _ = enigo.move_mouse(-1, 0, Coordinate::Rel);
}

fn fmt_dur(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{s}s")
    }
}

// ---------- autostart ----------

fn auto_launch() -> Option<auto_launch::AutoLaunch> {
    use auto_launch::AutoLaunchBuilder;
    let exe = std::env::current_exe().ok()?;
    AutoLaunchBuilder::new()
        .set_app_name("Jitter")
        .set_app_path(exe.to_string_lossy().as_ref())
        .set_use_launch_agent(true)
        .build()
        .ok()
}

fn autostart_enabled() -> bool {
    auto_launch().and_then(|a| a.is_enabled().ok()).unwrap_or(false)
}

fn set_autostart(enable: bool) -> Result<(), Box<dyn std::error::Error>> {
    let a = auto_launch().ok_or("could not resolve executable path")?;
    if enable {
        a.enable()?;
    } else {
        a.disable()?;
    }
    Ok(())
}

// ---------- icon ----------

fn make_icon(r: u8, g: u8, b: u8) -> Icon {
    let size: u32 = 32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let c = (size as f32 - 1.0) / 2.0;
    let rad = size as f32 * 0.42;
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            if dx * dx + dy * dy <= rad * rad {
                let i = ((y * size + x) * 4) as usize;
                rgba[i] = r;
                rgba[i + 1] = g;
                rgba[i + 2] = b;
                rgba[i + 3] = 255;
            }
        }
    }
    Icon::from_rgba(rgba, size, size).expect("failed to build icon")
}

fn icon_active() -> Icon {
    make_icon(46, 160, 67) // green
}
fn icon_paused() -> Icon {
    make_icon(140, 140, 140) // grey
}

// ---------- worker (mouse engine) ----------

fn spawn_worker(
    cfg: Config,
    enabled: Arc<AtomicBool>,
    nudges: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let mut enigo = match Enigo::new(&Settings::default()) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("input init failed (grant Accessibility on macOS): {e}");
                return;
            }
        };
        let tick = Duration::from_secs(5);
        let mut next_due = next_interval(&cfg);
        let mut waited = Duration::ZERO;

        while running.load(Ordering::SeqCst) {
            thread::sleep(tick);
            waited += tick;
            if waited < next_due {
                continue;
            }
            waited = Duration::ZERO;
            next_due = next_interval(&cfg);

            if !enabled.load(Ordering::SeqCst) {
                continue;
            }
            if !within_active_window(&cfg) {
                continue;
            }
            if idle_secs() < cfg.idle_threshold_secs {
                continue;
            }
            nudge(&mut enigo);
            nudges.fetch_add(1, Ordering::SeqCst);
        }
    });
}

// ---------- events ----------

enum UserEvent {
    MenuEvent(tray_icon::menu::MenuEvent),
}

fn main() {
    // Headless CLI still available for scripted installs.
    match std::env::args().nth(1).as_deref() {
        Some("--install") => {
            let _ = set_autostart(true).map_err(|e| eprintln!("{e}"));
            return;
        }
        Some("--uninstall") => {
            let _ = set_autostart(false).map_err(|e| eprintln!("{e}"));
            return;
        }
        _ => {}
    }

    let cfg = load_config();
    let start = Instant::now();

    // Shared state between the UI thread and the worker.
    let enabled = Arc::new(AtomicBool::new(true));
    let nudges = Arc::new(AtomicU64::new(0));
    let running = Arc::new(AtomicBool::new(true));

    spawn_worker(cfg.clone(), enabled.clone(), nudges.clone(), running.clone());

    {
        let r = running.clone();
        let _ = ctrlc::set_handler(move || {
            r.store(false, Ordering::SeqCst);
            std::process::exit(0);
        });
    }

    // Event loop MUST be built before the menu/tray so GTK is initialized on Linux.
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();

    // Forward menu clicks into the loop so it wakes to handle them.
    let proxy = event_loop.create_proxy();
    tray_icon::menu::MenuEvent::set_event_handler(Some(move |ev| {
        let _ = proxy.send_event(UserEvent::MenuEvent(ev));
    }));

    // Build the menu.
    let status_item = MenuItem::new("Status: starting…", false, None);
    let runtime_item = MenuItem::new("Runtime: 0s", false, None);
    let enabled_item = CheckMenuItem::new("Enabled", true, true, None);
    let autostart_item =
        CheckMenuItem::new("Start at login", true, autostart_enabled(), None);
    let quit_item = MenuItem::new("Quit Jitter", true, None);

    let menu = Menu::new();
    menu.append_items(&[
        &status_item,
        &runtime_item,
        &PredefinedMenuItem::separator(),
        &enabled_item,
        &autostart_item,
        &PredefinedMenuItem::separator(),
        &quit_item,
    ])
    .expect("failed to build menu");

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Jitter")
        .with_icon(icon_active())
        .build()
        .expect("failed to build tray icon");

    // IDs to match menu events against.
    let enabled_id = enabled_item.id().clone();
    let autostart_id = autostart_item.id().clone();
    let quit_id = quit_item.id().clone();

    let cfg_ui = cfg.clone();

    event_loop.run(move |event, _, control_flow| {
        // Wake ~once a second to refresh the labels.
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_secs(1));

        match event {
            Event::NewEvents(StartCause::Init) => { /* first tick refreshes below */ }
            Event::UserEvent(UserEvent::MenuEvent(ev)) => {
                if ev.id == quit_id {
                    running.store(false, Ordering::SeqCst);
                    *control_flow = ControlFlow::Exit;
                } else if ev.id == enabled_id {
                    // CheckMenuItem toggles itself; read the new state.
                    let on = enabled_item.is_checked();
                    enabled.store(on, Ordering::SeqCst);
                    let _ = tray.set_icon(Some(if on { icon_active() } else { icon_paused() }));
                } else if ev.id == autostart_id {
                    let want = autostart_item.is_checked();
                    if let Err(e) = set_autostart(want) {
                        eprintln!("autostart change failed: {e}");
                        autostart_item.set_checked(!want); // revert on failure
                    }
                }
            }
            _ => {}
        }

        // Refresh labels every wake.
        let secs = start.elapsed().as_secs();
        let n = nudges.load(Ordering::SeqCst);
        runtime_item.set_text(format!("Runtime: {} • {} nudges", fmt_dur(secs), n));

        let status = if !enabled.load(Ordering::SeqCst) {
            "Paused"
        } else if !within_active_window(&cfg_ui) {
            "Outside work hours"
        } else if idle_secs() < cfg_ui.idle_threshold_secs {
            "Active — you're here"
        } else {
            "Keeping you active"
        };
        status_item.set_text(format!("Status: {status}"));
    });
}
