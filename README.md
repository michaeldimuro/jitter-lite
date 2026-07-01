# Jitter

**Keep your computer showing as "active" in chat and presence apps** (Slack, Microsoft Teams, Discord, …) instead of flipping to "away"/yellow after a period of inactivity.

Jitter is a tiny, single-binary tray app written in Rust. It sits in your menu bar / system tray and, only when you've actually gone idle, performs an imperceptible 1-pixel mouse nudge (right, then back) to reset the OS input-idle timer — the timer presence apps key off. When you're actually using your machine, Jitter stays completely silent.

---

## How it works

Presence indicators don't look at whether your machine is awake — they look at the **operating-system input-idle timer** (time since your last real mouse/keyboard event). "Prevent sleep" tools like `caffeinate` keep the display on but do **not** reset this timer, so they won't keep you green.

Jitter resets the idle timer with a nudge that moves the cursor **1px right, then 1px back**, so it ends exactly where it started and the movement is invisible. It nudges only when **all** of these are true:

1. A randomized interval has elapsed (base ± jitter, so the cadence looks organic).
2. Jitter is enabled (not paused from the tray).
3. The current time is inside your active window (work hours + weekday rules).
4. **You've actually been idle** past a threshold — if you moved the mouse or typed recently, Jitter does nothing, because your real input already keeps you active.

Because each nudge resets idle to ~0, your idle time never climbs past the threshold while you're away — comfortably under any chat app's away cutoff (Slack ≈ 10 min).

---

## Download & install

Prebuilt binaries are published for macOS, Windows, and Linux on every tagged release.

1. Go to the **[Releases page](https://github.com/michaeldimuro/jitter-lite/releases)** and open the latest release.
2. Download the archive for your platform from **Assets**:
   - `jitter-macos-arm64.zip` — contains `Jitter.app`, the `jitter` CLI binary, and this README
   - `jitter-windows-x64.zip` — contains `jitter.exe` and this README
   - `jitter-linux-x64.tar.gz` — contains the `jitter` binary and this README

> Building from an untagged commit? The same binaries are also available as run artifacts under the [Actions tab](https://github.com/michaeldimuro/jitter-lite/actions) — open the latest **build** run and grab the artifact for your platform.

### macOS

1. Unzip the artifact and move **`Jitter.app`** to `/Applications`.
2. The binaries from CI are unsigned, so Gatekeeper will complain on first launch. Right-click the app → **Open** → **Open**, or run once:
   ```bash
   xattr -dr com.apple.quarantine /Applications/Jitter.app
   ```
3. Launch it. Jitter lives in the **menu bar only** (no Dock icon).
4. **Grant Accessibility permission** when prompted (System Settings → Privacy & Security → Accessibility → enable Jitter). Input injection silently fails without it, so this step is required.

### Windows

1. Unzip and put `jitter.exe` wherever you like (e.g. `C:\Tools\Jitter\`).
2. Double-click to run. SmartScreen may warn about an unsigned app — choose **More info → Run anyway**.
3. Jitter appears in the system tray.

### Linux

Jitter needs the GTK/appindicator runtime libraries and an **X11** session (Wayland is unsupported — see Limitations).

```bash
# Debian/Ubuntu
sudo apt install libgtk-3-dev libxdo-dev libayatana-appindicator3-dev

chmod +x jitter
./jitter
```

Jitter appears in the system tray. On GNOME you may need an AppIndicator extension for the tray icon to show.

---

## Using Jitter

### Tray menu

Click the tray/menu-bar icon:

- **Status** *(live)* — one of `Active — you're here`, `Keeping you active`, `Outside work hours`, or `Paused`.
- **Runtime** *(live)* — uptime and total nudge count.
- **Enabled** *(checkbox)* — pause/resume. The icon switches **green ↔ grey** to show state at a glance.
- **Start at login** *(checkbox)* — register/unregister Jitter to launch automatically at login.
- **Quit** — stop and exit.

### Start at login from the command line

For scripted installs, without opening the UI:

```bash
jitter --install      # register autostart at login
jitter --uninstall    # remove it
```

### Configuration

On first run Jitter writes **`jitter.toml`** next to the executable and reads it on every launch. Edit it to change behavior — no recompiling needed. Defaults:

```toml
interval_secs = 45          # base gap between nudges
jitter_secs = 20            # random ± applied to each interval (organic cadence)
idle_threshold_secs = 25    # only nudge after this much real idle time
active_start_hour = 8       # 24h clock; start of active window
active_end_hour = 18        # end of active window
weekdays_only = true        # skip Sat/Sun
```

Notes:
- Set `active_start_hour == active_end_hour` to **disable the schedule** and run 24/7.
- The active window **wraps past midnight** correctly (e.g. `22`→`6`).

---

## Build from source

Requires the [Rust toolchain](https://rustup.rs).

```bash
cargo build --release        # binary at target/release/jitter
```

**macOS** — build the menu-bar app bundle:
```bash
./packaging/macos/bundle.sh  # produces dist/Jitter.app
```

**Linux build prerequisites:**
```bash
sudo apt install libgtk-3-dev libxdo-dev libayatana-appindicator3-dev libdbus-1-dev pkg-config
```

The release profile is size-optimized (`opt-level = "z"`, LTO, stripped). CI builds all three platforms via `.github/workflows/release.yml` on every `v*` tag or manual dispatch.

---

## Permissions & limitations

- **macOS** — requires **Accessibility** permission (input injection fails silently without it).
- **Linux** — works on **X11**. **Wayland is unsupported**: both idle detection and synthetic input are unreliable or blocked by design.
- **Windows** — no special permission, but some corporate MDM/endpoint-security tools may flag input-injection binaries.

---

## A note on responsible use

Jitter spoofs presence indicators. There are legitimate uses (presentations, long renders/downloads, remote sessions timing out). If you're running it on a work machine, check your employer's acceptable-use policy first — some treat presence-spoofing as a violation regardless of the tool. That's your call to make.
