# CLAUDE.md — Jitter

Guidance for any agent (or human) resuming work on **Jitter**. Read this fully before changing code; it records not just *what* the project is but *why* it's built the way it is, including one feature that was built and deliberately removed.

---

## 1. Purpose

Jitter keeps a computer showing as **"active"/green** in chat/presence apps (Slack, Microsoft Teams, Discord, etc.) instead of flipping to **"away"/yellow** after a period of inactivity.

**The one insight the whole project rests on:** presence indicators key off the operating system's **input-idle timer** — the time since the last real mouse/keyboard event — *not* the machine's power/sleep state. Consequences:

- "Prevent sleep" APIs (`caffeinate` on macOS, `SetThreadExecutionState` on Windows) keep the display awake but do **not** reset the input-idle timer, so they will **not** keep you green. Don't go down that road.
- To reset the idle timer you must either (a) inject a real input event, or (b) call a per-OS "declare user activity" API. Jitter uses (a): a tiny mouse nudge.

The nudge is **1px right, then 1px back** (`move_mouse(1,0,Rel)` → `move_mouse(-1,0,Rel)`), so the cursor ends exactly where it started and the movement is imperceptible.

---

## 2. Design principles (do not regress these)

1. **Lightweight, single compiled binary, cross-platform.** This was the founding requirement. That's why the stack is Rust (one static-ish binary per OS) and why Electron / Python-bundles / anything requiring a heavy runtime were rejected.
2. **Never fight the user.** Jitter must never nudge while the person is actively using the machine. This is enforced by the **idle gate** (see §4). Their real input already keeps them green; Jitter only fills the gaps *after* they've gone idle past a threshold. This principle is why a manual "don't interfere" kill-switch is unnecessary.
3. **Look organic.** Randomized interval (base ± jitter) and a work-hours/weekday schedule, so presence isn't robotically uniform or "active" at 3 a.m.
4. **Visible state.** The tray icon and menu always reflect what Jitter is doing.

---

## 3. Current state

**Version:** v0.4. **Repo layout:** `Cargo.toml` + `src/main.rs` (~410 lines). (Historically `main.rs` embedded `Cargo.toml` as a comment; that has been split into a real Cargo project.) Packaging lives under `packaging/` and CI under `.github/workflows/`.

The v0.4 feature set: idle-aware nudging, organic randomized interval, work-hours + weekday schedule, TOML config file, login autostart, and a **system-tray icon with a menu** (Status, Runtime + nudge count, Enabled toggle, Start-at-login toggle, Quit).

---

## 4. How it works (behavioral logic)

The mouse engine runs on a **background worker thread**. Every 5s tick it decides whether to nudge. It nudges **only if all** of these hold:

1. The randomized interval since the last nudge has elapsed (`interval_secs ± jitter_secs`).
2. `enabled` is true (not paused via the tray).
3. The current local time is inside the active window (work hours + weekday rules).
4. **The idle gate:** the machine has been idle ≥ `idle_threshold_secs`. If the user moved the mouse or typed within that window, Jitter stays silent — their real input already resets the idle timer.

Because each nudge resets idle to ~0, when the user is away idle never climbs past `idle_threshold_secs + interval`, which stays well under any chat app's away threshold (Slack ≈ 10 min).

---

## 5. Configuration

On first run Jitter writes **`jitter.toml`** next to the executable and reads it thereafter. Editing the file changes behavior without recompiling. Schema and defaults:

```toml
interval_secs = 45          # base gap between nudges
jitter_secs = 20            # random ± applied to each interval (organic cadence)
idle_threshold_secs = 25    # only nudge after this much real idle time
active_start_hour = 8       # 24h clock; start of active window
active_end_hour = 18        # end of active window
weekdays_only = true        # skip Sat/Sun
```

Notes:
- Set `active_start_hour == active_end_hour` to **disable the schedule** (run 24/7).
- The window **wraps past midnight** correctly (e.g. `22`→`6`).

---

## 6. Tray menu

- **Status** (disabled label, live) — one of: `Active — you're here`, `Keeping you active`, `Outside work hours`, `Paused`.
- **Runtime** (disabled label, live) — uptime + total nudge count, refreshed ~1×/sec.
- **Enabled** (checkbox) — pauses/resumes the engine and swaps the tray icon **green↔grey** for at-a-glance state.
- **Start at login** (checkbox) — initialized from the real autostart state; toggling calls `auto-launch` and reverts the checkmark on failure.
- **Quit** — stops the worker and exits cleanly.

There is also a **headless CLI** for scripted installs: `jitter --install` / `jitter --uninstall` register/deregister login autostart without opening the UI.

---

## 7. Architecture

- **Main thread** owns the `tao` event loop (required: on macOS the loop and tray must live on the main thread; on Linux the loop init brings up GTK). It only touches UI. It wakes ~once a second via `ControlFlow::WaitUntil` to refresh the Status/Runtime labels, and handles menu clicks.
- **Worker thread** owns the `enigo` instance and runs the nudge logic in §4.
- **Shared state** (all `Arc`):
  - `enabled: AtomicBool` — UI writes, worker reads.
  - `nudges: AtomicU64` — worker increments, UI reads for the label.
  - `running: AtomicBool` — set false on Quit/Ctrl-C to stop the worker.
- **Menu events** are delivered by forwarding `tray_icon::menu::MenuEvent::set_event_handler` into the loop via the tao `EventLoopProxy`, then matching on cloned `MenuId`s (`enabled_id`, `autostart_id`, `quit_id`).
- **Ordering constraint:** build the `EventLoop` **before** the menu/tray so GTK is initialized on Linux before any menu widget is created. Do not reorder this.

---

## 8. Dependencies and why each was chosen

| Crate | Ver (verify) | Role | Notes / rationale |
|---|---|---|---|
| `enigo` | 0.6 | mouse input simulation | Small, cross-platform. API: `Enigo::new(&Settings::default())`, `move_mouse(x, y, Coordinate::Rel)`. |
| `user-idle` | 0.1 | idle-time detection (**non-macOS only**) | `UserIdle::get_time().as_seconds()`. ⚠️ Correction to earlier docs: v0.1.1 has **no working macOS path** — on any `cfg(unix)` target (incl. macOS) it queries the ScreenSaver over **D-Bus**, which needs `libdbus` to build and, on macOS, always errors → idle reads as 0 → Jitter never nudges. So it's now scoped to `[target.'cfg(not(target_os = "macos"))'.dependencies]` and macOS uses a native call instead (next row). On Linux it reports screensaver-lock time. |
| CoreGraphics FFI | (system) | idle-time detection (**macOS**) | `mac_idle` module in `main.rs`: direct `#[link]` to `CGEventSourceSecondsSinceLastEventType(kCGEventSourceStateHIDSystemState, kCGAnyInputEventType)` → real HID input-idle seconds. No crate dependency; the framework ships with macOS. |
| `chrono` | 0.4 | local time | Work-hours/weekday schedule (`Local::now()`, `.hour()`, `.weekday()`). |
| `auto-launch` | 0.5 | login autostart | `AutoLaunchBuilder` → `enable/disable/is_enabled`. `set_use_launch_agent(true)` for macOS LaunchAgent. |
| `ctrlc` | 3 | clean terminal shutdown | Sets `running=false` and exits. |
| `serde` + `toml` | 1 / 0.8 | config file | `#[serde(default)]` so partial configs fill from defaults. |
| `tao` | 0.31 | event loop | **Chosen over `winit`** deliberately: tao (Tauri's windowing lib) initializes **GTK on Linux automatically** and uses the simpler closure-based `run` API, avoiding winit 0.30's verbose `ApplicationHandler` trait *and* manual GTK juggling. |
| `tray-icon` | 0.21 | tray icon + menu | From tauri-apps; **re-exports `muda`** as `tray_icon::menu`, so no separate menu dependency. `CheckMenuItem` auto-toggles on click (read `is_checked()` after the event; don't flip manually). Icons via `Icon::from_rgba(Vec<u8>, w, h)`. |

**Version-drift warning:** `enigo`, `tao`, and `tray-icon` change APIs fairly often. If a build fails, run `cargo add enigo user-idle chrono auto-launch ctrlc tao tray-icon` and re-check the signatures above against current docs before assuming a logic bug.

---

## 9. Removed feature — do NOT reintroduce without cause

**v0.3 had a global hotkey** (`Ctrl+Alt+J`) to pause/resume, implemented with the **`device_query`** crate (polls global key state, no event loop). It was **deliberately removed in v0.4** and replaced by the tray toggle. Context so it isn't re-added by reflex:

- The user first suggested a bare **ESC** toggle. That was rejected: ESC is far too busy a key (dialogs, fullscreen, vim, cancel), and a global hook on it is collision-prone. A modifier combo was used instead.
- Then the user decided the tray menu should own on/off entirely, and asked to **scrap the hotkey**. So `device_query` and all key-watching code are gone.
- If a hotkey is ever wanted again: use `device_query` (`DeviceState::get_keys()` → `Vec<Keycode>`, no event loop, fits the polling design), use a **modifier combo, never bare ESC**, and note it needs **Input Monitoring** permission on macOS (separate from Accessibility) and X11 on Linux.

---

## 10. Build

```bash
cargo build --release
# binary at target/release/jitter
```

Release profile (in the embedded Cargo.toml) is size-tuned: `opt-level = "z"`, `lto = true`, `strip = true`.

**Per-OS build prerequisites:**
- **Windows** — none beyond the Rust toolchain.
- **macOS** — Xcode command-line tools.
- **Linux** — GTK + appindicator + xdo dev libraries:
  ```bash
  sudo apt install libgtk-3-dev libxdo-dev libappindicator3-dev
  # or the libayatana-appindicator3-dev equivalent on newer distros
  ```

---

## 11. Runtime permissions

- **macOS** — `enigo` needs **Accessibility** permission (System Settings → Privacy & Security → Accessibility). Input injection silently fails without it. Running as a proper `.app` bundle makes the permission prompt behave; a bare binary works but the flow is clunky.
- **Linux** — works on **X11**. **Wayland is the known weak spot**: both idle detection and synthetic input are unreliable/blocked by design. Test on the target session type.
- **Windows** — no special permission, but note some MDM/endpoint-security tools flag input-injection binaries.

---

## 12. Distribution

- **Windows:** ship `jitter.exe`. Optionally code-sign to avoid SmartScreen warnings. Autostart via the tray toggle or `--install` (registry Run key, handled by `auto-launch`).
- **macOS:** wrap in a `.app` bundle. Set `LSUIElement = true` (a.k.a. "Application is agent") in `Info.plist` so it lives only in the menu bar with **no Dock icon**. For distribution outside your own machine, **code-sign + notarize**, or users hit Gatekeeper. Autostart uses a LaunchAgent (`set_use_launch_agent(true)`).
- **Linux:** ship the binary; the target must have the GTK/appindicator runtime libraries present. Package as `.deb` or **AppImage** for convenience. Desktop-environment support for the tray varies (GNOME may need an appindicator extension).

---

## 13. Known limitations

- **Wayland** — idle detection and input injection are unreliable; X11 is the supported Linux path.
- **Endpoint security** — corporate MDM/EDR tooling may flag or block input-injection binaries.
- **Cross-building** — only the host OS's binary is produced locally. macOS is built on this machine; Windows/Linux binaries come from CI (`.github/workflows/release.yml`), since GUI binaries with C deps don't cleanly cross-compile from macOS.

---

## 14. Natural next steps / extension ideas

- **Settings submenu in the tray** (the user was offered this and it's the obvious next feature): interval/idle presets and a "Run 24/7" toggle, so behavior is adjustable without editing `jitter.toml`. Write changes back to the config file.
- **macOS no-movement mode:** instead of a cursor nudge, call `IOPMAssertionDeclareUserActivity` (IOKit) to reset the HID idle timer with zero visible movement. A per-OS "zero-movement input injection" path (Windows `SendInput` with a 0,0 relative move; X11 `XTestFakeRelativeMotionEvent(0,0)`) is the "pro" version of the nudge.
- **Notifications** on pause/resume, or a "pause for 30 min" timed option.
- **Split into modules** (`config.rs`, `engine.rs`, `tray.rs`) once it grows.

---

## 15. Usage & ethics note

Jitter spoofs presence indicators. There are legitimate uses (presentations, long renders/downloads, remote sessions timing out). If it's aimed at a work machine, the user should check their employer's acceptable-use policy — some treat presence-spoofing as a violation regardless of tool. This is the user's call to make; it's noted here for context, not as a blocker.

---

## 16. Version history

- **v0.1** — minimal loop: fixed-interval 1px nudge. Established the "presence = input-idle timer" insight and the Rust + `enigo` stack.
- **v0.2** — idle-aware gating, organic randomized interval, work-hours/weekday schedule, `jitter.toml` config, `--install/--uninstall` autostart, Ctrl-C shutdown.
- **v0.3** — global `Ctrl+Alt+J` pause/resume hotkey via `device_query`. **(Removed in v0.4.)**
- **v0.4** — system-tray icon + menu (`tao` + `tray-icon`): live Status, Runtime + nudge count, Enabled toggle with green/grey icon, Start-at-login toggle, Quit. Hotkey and `device_query` removed. Current.
- **v0.4.1** (build/packaging) — split into a real Cargo project (`Cargo.toml` + `src/main.rs`); **fixed macOS idle detection** (native CoreGraphics call, replacing the non-functional `user-idle` D-Bus path); added `packaging/macos/` (Info.plist + bundle.sh → `Jitter.app`, `LSUIElement`) and a GitHub Actions matrix build for macOS/Windows/Linux.