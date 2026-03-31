# biy

AI-integrated terminal emulator for macOS.

Big Bang + Yoon = **biy**

## Features

### Terminal
- Local shell + SSH connection
- Tabs (create / close / drag reorder)
- Split view (horizontal / vertical / drag split)
- Search (⌘F)
- Font size shortcuts (⌘+ / ⌘- / ⌘0)

### SSH & SFTP
- SSH connection manager (add / edit / delete)
- Authentication: SSH Agent / Key file / Password (macOS Keychain)
- Auto TMOUT=0 on SSH connect
- SFTP file tree (local & remote)
- File download / upload with progress bar
- Right-click context menu (download / rename / delete / copy path)
- Hidden files toggle

### UI
- macOS native look & feel
- 12 color themes (Dracula, Nord, Tokyo Night, Catppuccin, etc.)
- Sidebar with session / SSH server tabs
- Resizable sidebar
- Settings modal (theme, font, cursor, scrollback)
- Settings persistence

## Tech Stack

- **Tauri 2** — Rust backend + Web frontend
- **xterm.js** — Terminal emulation (WebGL rendering)
- **ssh2** — Rust SSH/SFTP library
- **portable-pty** — Local PTY management
- **SwiftUI** — Native macOS settings app (legacy)

## Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| ⌘T | New tab |
| ⌘W | Close tab |
| ⌘D | Split horizontal |
| ⌘⇧D | Split vertical |
| ⌘⇧N | SSH connect |
| ⌘B | Toggle sidebar |
| ⌘F | Search |
| ⌘, | Settings |
| ⌘+ / ⌘- | Font size |
| ⌘0 | Reset font size |
| ⌘[ / ⌘] | Switch tabs |
| ⌘⌥← / → | Switch split panes |

## Build

### Prerequisites
- Rust (latest stable)
- Node.js 18+
- Tauri CLI (`cargo install tauri-cli`)

### Development
```bash
# Install dependencies
npm install

# Build frontend
npx esbuild src/main.js --bundle --outfile=dist/main.js --format=esm --platform=browser
cp src/styles.css dist/
cp src/index.html dist/
cp node_modules/@xterm/xterm/css/xterm.css dist/

# Build app
cargo tauri build --debug
```

### Run
```bash
open src-tauri/target/debug/bundle/macos/biy.app
```

## License

MIT
