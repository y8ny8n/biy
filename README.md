# termy

AI-integrated terminal emulator for macOS.

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

## 소스에서 직접 빌드 (Build from source)

내려받은 앱이 Gatekeeper("손상됨"/"확인되지 않은 개발자")로 안 열릴 때는, **직접 빌드하면 그런 경고 없이 바로 실행**됩니다 (로컬 빌드 앱은 격리가 안 붙음).

### 사전 준비 (Prerequisites)
- **Rust** (stable) — https://rustup.rs 에서 한 줄 설치
  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```
- **Node.js 18+** — https://nodejs.org (또는 `brew install node`)
- macOS (Apple Silicon 권장)

### 빌드 (Build)
```bash
git clone https://github.com/y8ny8n/biy.git
cd biy
npm install
npm run build
```
빌드가 끝나면 앱 실행:
```bash
open src-tauri/target/release/bundle/macos/termy.app
```
→ 마음에 들면 `termy.app`을 응용 프로그램(Applications) 폴더로 드래그하면 됩니다.

### 개발 모드 (Dev, 실시간 리로드)
```bash
npm run dev
```

> 처음 빌드는 Rust 의존성 컴파일로 몇 분 걸립니다. 이후 빌드는 훨씬 빠릅니다.

## License

MIT
