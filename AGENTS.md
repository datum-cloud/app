# Agent notes

How to build, run, and exercise the Datum desktop app locally. The workspace has three crates: `lib` (shared logic), `ui` (the Dioxus desktop app, package `datum-connect-gui`, binary `Datum`), and `cli`.

## Protect the installed app first

A dev build shares the installed app's data folder and updates itself by default. Set both of these before the first launch, in the shell you launch from:

```bash
export DATUM_CONNECT_REPO="$HOME/datum-dev-repo"
mkdir -p "$DATUM_CONNECT_REPO"
printf 'auto_update_enabled: false\n' > "$DATUM_CONNECT_REPO/update_settings.yml"
```

- Without `DATUM_CONNECT_REPO`, the app uses `~/Library/Application Support/Datum` on macOS, which holds the installed app's login, tunnels, and keys.
- A dev build reports version 0.1.0, so every release looks newer. With auto-update on, it downloads the latest release and installs it on the next launch: on macOS into `/Applications/Datum.app`, on Linux over the running binary.
- Do not use `DATUM_UPDATE_FAKE=1` to avoid this. Its "Install now" opens the installed app.
- A `.env` file in the launch directory is loaded. The API defaults to production.

## macOS

Prerequisites:

```bash
xcode-select --install
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup update stable   # connect-share needs Rust 1.98 or newer
```

`ui/assets/tailwind.css` is committed, so no npm step is needed to run the app.

Run a dev build from the repository root, with the environment from the section above:

```bash
cargo run -p datum-connect-gui --bin Datum
```

Or with hot reload, using the Dioxus CLI:

```bash
cd ui && dx serve --platform desktop
```

CI bundles with dx 0.7.2, because `dx bundle` path handling regressed in 0.7.3 and later (see `.github/workflows/bundle.yml`). For `dx serve` and `dx build`, 0.7.3 has worked.

To build a `.app` bundle: `cd ui && dx bundle --desktop --release --package-types macos`. It is ad-hoc signed and not notarized. Keep it out of `/Applications`: it has the installed app's bundle ID, and auto-update targets that path. Launch its binary directly, `"$APP/Contents/MacOS/Datum"`, so `DATUM_CONNECT_REPO` carries over; `open` does not pass shell variables.

Closing the window only hides it. Reopen it from the tray menu's **Show Window**, and quit with Cmd+Q. Quit the installed app first to avoid two tray icons.

## Try local tunnels without an account

Local tunnels pair devices with a ticket through `connect-share`. They need no Datum account.

1. Start something to share: `python3 -m http.server 8000`.
2. On the login screen, click **Continue without an account**.
3. Click **Add New**, choose **Share with devices**, and enter `127.0.0.1:8000`. The app shows a QR code and a `DCSHARE2…` ticket.
4. To connect back, choose **Add New**, then **Open a ticket**, paste the ticket, and pick a local port. The service opens at `http://127.0.0.1:PORT`.

The other side can also be the Datum Connect Android app (scan the QR code), `datumctl connect dial TICKET`, or a second app instance launched with a different `DATUM_CONNECT_REPO`.

A share keeps its ticket across restarts; **New ticket** in its menu replaces it and cuts off devices holding the old one. A connection turns failed about 13 seconds after its share goes away.

## Linux, including headless

The flake's dev shell provides Rust, `dx`, WebKitGTK, and GTK:

```bash
nix develop -c dx build --platform desktop   # run from ui/
```

The dev shell lacks two runtime libraries. Supply them from the flake's nixpkgs:

- `libxdo`, needed to link: `LIBRARY_PATH=$(nix build --no-link --print-out-paths --inputs-from . nixpkgs#xdotool)/lib`
- `libayatana-appindicator`, needed for the tray icon at startup: add its `lib` directory to `LD_LIBRARY_PATH`.

To run under Xvfb, WebKit needs software EGL from the same nixpkgs as the build:

```bash
MESA=$(nix build --no-link --print-out-paths --inputs-from . nixpkgs#mesa)
DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1 \
  __EGL_VENDOR_LIBRARY_FILENAMES=$MESA/share/glvnd/egl_vendor.d/50_mesa.json \
  WEBKIT_DISABLE_DMABUF_RENDERER=1 WEBKIT_DISABLE_COMPOSITING_MODE=1 \
  target/dx/Datum/debug/linux/app/Datum
```

Drive it with `xdotool` and capture it with `import -window root`. Give `xdotool type` a `--delay` of 50 ms or more, or WebKit drops repeated keystrokes.

## Checks before committing

```bash
cargo fmt --check --all
cargo check --locked
cargo test -p lib --locked     # local tunnel tests use the public iroh relays
```

After adding Tailwind classes, regenerate the stylesheet from `ui/`: `npx @tailwindcss/cli -i ./theme.css -o ./assets/tailwind.css`. Commit only the rules your change needs; Tailwind also picks up ordinary words in Rust source, such as `running`.
