use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

/// PTY 핸들
struct PtyHandle {
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send>,
    master: Box<dyn portable_pty::MasterPty + Send>,
}

/// SSH 메시지 (입력 + 리사이즈)
enum SshMsg {
    Input(Vec<u8>),
    Resize(u16, u16),
}
type SshInputSender = std::sync::mpsc::Sender<SshMsg>;

/// SSH 셸 루프 종료 사유. 재연결 판단에 사용.
enum SshOutcome {
    /// 사용자가 정상 종료 (재연결하지 않음)
    Closed,
    /// 네트워크 끊김 등 비정상 종료 (재연결 시도)
    Disconnected(String),
}

/// SFTP 핸들 (SSH 연결과 연동)
struct SftpHandle {
    session: ssh2::Session,
    cwd: String,
}

/// PTY + SSH 입력 관리 (터미널 경로)
struct PtyManager {
    ptys: HashMap<u32, PtyHandle>,
    ssh_inputs: HashMap<u32, SshInputSender>,
}

type PtyState = Arc<Mutex<PtyManager>>;

/// SFTP 세션은 별도 뮤텍스로 분리한다.
/// 파일 전송이 락을 길게 잡아도 터미널 입력(PtyManager)은 막히지 않게 하기 위함.
type SftpState = Arc<Mutex<HashMap<u32, SftpHandle>>>;

/// SSH 연결 설정 (프론트에서 받음)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SshConfig {
    host: String,
    port: u16,
    username: String,
    #[serde(rename = "authType", default)]
    auth_type: String,
    #[serde(rename = "keyPath", default)]
    key_path: String,
    #[serde(default)]
    name: String,
}

/// 로컬 PTY 생성
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri 커맨드: 인자는 모두 프레임워크가 주입
fn create_pty(
    id: u32,
    cols: u16,
    rows: u16,
    tab_type: String,
    ssh_config: Option<SshConfig>,
    state: State<'_, PtyState>,
    sftp_state: State<'_, SftpState>,
    app: AppHandle,
) -> Result<u32, String> {
    if tab_type == "ssh" {
        if let Some(config) = ssh_config {
            return create_ssh_pty(id, cols, rows, config, state, sftp_state, app);
        }
        return Err("SSH config required".to_string());
    }

    let pty_system = native_pty_system();
    let pty_pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;

    let mut cmd = CommandBuilder::new_default_prog();
    cmd.env("TERM", "xterm-256color");

    let child = pty_pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    let writer = pty_pair.master.take_writer().map_err(|e| e.to_string())?;
    let mut reader = pty_pair.master.try_clone_reader().map_err(|e| e.to_string())?;

    // 읽기 스레드: PTY → 프론트 (+ OSC 타이틀 감지)
    let app_clone = app.clone();
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        let mut osc_state: u8 = 0; // 0=none, 1=got ESC, 2=in OSC
        let mut osc_buf: Vec<u8> = Vec::new();

        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let data = &buf[..n];

                    // OSC 시퀀스 감지
                    for &byte in data {
                        match osc_state {
                            0 => {
                                if byte == 0x1b { osc_state = 1; }
                            }
                            1 => {
                                if byte == 0x5d { // ]
                                    osc_state = 2;
                                    osc_buf.clear();
                                } else {
                                    osc_state = 0;
                                }
                            }
                            2 => {
                                if byte == 0x07 || (byte == 0x5c && osc_buf.last() == Some(&0x1b)) {
                                    // OSC 끝 (BEL 또는 ST)
                                    if byte == 0x5c { osc_buf.pop(); } // ESC 제거
                                    if let Ok(s) = std::str::from_utf8(&osc_buf) {
                                        if s.starts_with("0;") || s.starts_with("2;") {
                                            let _ = app_clone.emit("pty-title", serde_json::json!({
                                                "id": id,
                                                "title": &s[2..],
                                            }));
                                        }
                                    }
                                    osc_state = 0;
                                } else if osc_buf.len() < 512 {
                                    osc_buf.push(byte);
                                } else {
                                    osc_state = 0; // 너무 길면 포기
                                }
                            }
                            _ => { osc_state = 0; }
                        }
                    }

                    let _ = app_clone.emit("pty-output", serde_json::json!({
                        "id": id,
                        "data": data.to_vec(),
                    }));
                }
                Err(_) => break,
            }
        }
        let _ = app_clone.emit("pty-exit", serde_json::json!({ "id": id }));
    });

    let mut mgr = state.lock().unwrap();
    mgr.ptys.insert(id, PtyHandle {
        writer,
        child,
        master: pty_pair.master,
    });

    Ok(id)
}

/// SSH PTY 생성 + SFTP 세션
fn create_ssh_pty(
    id: u32,
    cols: u16,
    rows: u16,
    config: SshConfig,
    state: State<'_, PtyState>,
    sftp_state: State<'_, SftpState>,
    app: AppHandle,
) -> Result<u32, String> {
    // 0. 비밀번호 모드 첫 접속 처리.
    //    create_sftp_session은 비밀번호를 묻지 않고 키체인만 확인하므로,
    //    키체인이 비어 있으면 SFTP가 먼저 인증에 실패해 첫 접속에 파일트리가 뜨지 않는다.
    //    여기서 비밀번호를 한 번만 받아 키체인에 저장하면 SFTP·SSH 셸 둘 다 그것을 재사용한다.
    if config.auth_type.eq_ignore_ascii_case("password")
        && keychain_get_password(&config.host, &config.username).is_none()
    {
        if let Some(pw) = prompt_password(&config.host, &config.username) {
            keychain_save_password(&config.host, &config.username, &pw);
        }
    }

    // 1. SFTP용 SSH 세션을 먼저 동기적으로 생성 (인증은 키체인 비밀번호/키로 수행)
    setup_sftp(id, &config, sftp_state.inner(), &app);

    // 2. SSH 입력 채널 생성
    let (input_tx, input_rx) = std::sync::mpsc::channel::<SshMsg>();
    {
        if let Ok(mut mgr) = state.lock() {
            mgr.ssh_inputs.insert(id, input_tx);
        }
    }

    // 3. SSH 터미널 연결 (별도 스레드). 끊기면 최대 3회 자동 재연결.
    let app_clone = app.clone();
    let state_clone = Arc::clone(state.inner());
    let sftp_state_clone = Arc::clone(sftp_state.inner());
    thread::spawn(move || {
        let mut attempt = 0u32;
        loop {
            let started = std::time::Instant::now();
            let outcome = ssh_session(&config, cols, rows, id, &app_clone, &input_rx);

            // 30초 이상 유지됐다면 안정적 연결로 보고 재시도 카운터 리셋
            if started.elapsed() >= std::time::Duration::from_secs(30) {
                attempt = 0;
            }

            let reason = match outcome {
                Ok(SshOutcome::Closed) => {
                    let _ = app_clone.emit("pty-exit", serde_json::json!({ "id": id }));
                    break;
                }
                Ok(SshOutcome::Disconnected(r)) => r,
                Err(e) => {
                    log::error!("SSH error: {}", e);
                    format!("{}", e)
                }
            };

            attempt += 1;
            if attempt > 3 {
                let _ = app_clone.emit("pty-error", serde_json::json!({
                    "id": id,
                    "error": format!("{} — 재연결 실패", reason),
                }));
                let _ = app_clone.emit("pty-exit", serde_json::json!({ "id": id }));
                break;
            }

            let _ = app_clone.emit("pty-error", serde_json::json!({
                "id": id,
                "error": format!("{} — 재연결 중 ({}/3)...", reason, attempt),
            }));
            thread::sleep(std::time::Duration::from_secs(2 * attempt as u64));

            // SFTP 재생성 후 프론트에 재연결 알림 (TMOUT/OSC7 재주입 트리거)
            setup_sftp(id, &config, &sftp_state_clone, &app_clone);
            let _ = app_clone.emit("pty-reconnected", serde_json::json!({ "id": id }));
        }

        if let Ok(mut mgr) = state_clone.lock() {
            mgr.ssh_inputs.remove(&id);
        }
        if let Ok(mut sftps) = sftp_state_clone.lock() {
            sftps.remove(&id);
        }
    });

    Ok(id)
}

/// SFTP 세션을 만들어 상태에 등록한다. 실패해도 치명적이지 않음(파일트리만 비활성화).
fn setup_sftp(id: u32, config: &SshConfig, sftp_state: &SftpState, app: &AppHandle) {
    match create_sftp_session(config) {
        Ok(sftp_session) => {
            // 홈 디렉토리 가져오기
            sftp_session.set_timeout(30_000);
            sftp_session.set_blocking(true);
            let cwd = (|| -> Option<String> {
                let mut ch = sftp_session.channel_session().ok()?;
                ch.exec("pwd").ok()?;
                let mut out = String::new();
                ch.read_to_string(&mut out).ok()?;
                ch.wait_close().ok()?;
                let p = out.trim().to_string();
                if p.is_empty() { None } else { Some(p) }
            })()
            .unwrap_or_else(|| "/root".to_string());

            sftp_session.set_blocking(false);

            if let Ok(mut sftps) = sftp_state.lock() {
                sftps.insert(id, SftpHandle {
                    session: sftp_session,
                    cwd,
                });
            }
            log::info!("SFTP session created for tab {}", id);
        }
        Err(e) => {
            log::warn!("SFTP session failed: {} (file tree won't work)", e);
            let _ = app.emit("pty-error", serde_json::json!({
                "id": id,
                "error": format!("파일 트리 연결 실패: {}", e),
            }));
        }
    }
}

/// 서버 호스트 키 검증 (TOFU). 처음 보는 서버는 저장하고, 지문이 바뀌면 차단한다.
/// 저장 위치: ~/.config/termy/known_hosts (시스템 ~/.ssh/known_hosts는 건드리지 않음).
fn verify_host_key(
    session: &ssh2::Session,
    host: &str,
    port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = dirs::home_dir()
        .ok_or("홈 디렉토리를 찾을 수 없음")?
        .join(".config/termy/known_hosts");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let mut known = session.known_hosts()?;
    // 기존 파일 로드 (없으면 무시)
    let _ = known.read_file(&path, ssh2::KnownHostFileKind::OpenSSH);

    let (key, key_type) = session
        .host_key()
        .ok_or("서버 호스트 키를 가져올 수 없음")?;

    match known.check_port(host, port, key) {
        ssh2::CheckResult::Match => Ok(()),
        ssh2::CheckResult::NotFound => {
            // TOFU: 처음 보는 서버 → 저장
            known.add(host, key, "termy", key_type.into())?;
            known.write_file(&path, ssh2::KnownHostFileKind::OpenSSH)?;
            log::info!("새 호스트 키 저장: {}:{}", host, port);
            Ok(())
        }
        ssh2::CheckResult::Mismatch => Err(format!(
            "서버 지문이 바뀌었습니다 ({}:{}). 중간자 공격(MITM) 가능성이 있어 접속을 차단합니다. \
             서버를 정말 교체한 게 맞다면 ~/.config/termy/known_hosts 의 해당 항목을 삭제하세요.",
            host, port
        )
        .into()),
        ssh2::CheckResult::Failure => Err("호스트 키 검증에 실패했습니다".into()),
    }
}

/// SFTP용 SSH 세션 생성
fn create_sftp_session(config: &SshConfig) -> Result<ssh2::Session, Box<dyn std::error::Error>> {
    use std::net::TcpStream;

    let addr = format!("{}:{}", config.host, config.port);
    let socket_addr: std::net::SocketAddr = addr.parse()
        .or_else(|_| {
            use std::net::ToSocketAddrs;
            addr.to_socket_addrs()?.next().ok_or_else(|| std::io::Error::other("DNS 실패"))
        })?;
    let tcp = TcpStream::connect_timeout(&socket_addr, std::time::Duration::from_secs(10))?;
    tcp.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
    tcp.set_write_timeout(Some(std::time::Duration::from_secs(30)))?;

    let mut session = ssh2::Session::new()?;
    session.set_tcp_stream(tcp);
    session.handshake()?;
    session.set_keepalive(true, 30);
    session.set_timeout(30_000);

    verify_host_key(&session, &config.host, config.port)?;

    // 인증 (ssh_session과 동일 로직)
    let mut authenticated = false;
    let is_password_mode = config.auth_type.eq_ignore_ascii_case("password");

    if !is_password_mode {
        if let Ok(mut agent) = session.agent() {
            if agent.connect().is_ok() {
                let _ = agent.list_identities();
                if let Ok(identities) = agent.identities() {
                    for identity in &identities {
                        if agent.userauth(&config.username, identity).is_ok() && session.authenticated() {
                            authenticated = true;
                            break;
                        }
                    }
                }
            }
        }

        // 사용자 지정 키 경로 우선 시도 (프롬프트 허용)
        if !authenticated && !config.key_path.trim().is_empty() {
            let path = expand_path(&config.key_path);
            if path.exists() && try_key_auth(&session, &config.username, &path, true) {
                authenticated = true;
            }
        }

        // 기본 키는 passphrase 없이만 조용히 시도
        if !authenticated {
            let home = std::env::var("HOME").unwrap_or_default();
            for key in &["id_ed25519", "id_rsa", "id_ecdsa"] {
                let path = std::path::PathBuf::from(&home).join(".ssh").join(key);
                if path.exists() && try_key_auth(&session, &config.username, &path, false) {
                    authenticated = true;
                    break;
                }
            }
        }
    }

    if !authenticated && !config.auth_type.eq_ignore_ascii_case("key") {
        if let Some(password) = keychain_get_password(&config.host, &config.username) {
            let _ = session.userauth_password(&config.username, &password);
        }
    }

    if !session.authenticated() {
        return Err("SFTP 인증 실패".into());
    }

    Ok(session)
}

fn ssh_session(
    config: &SshConfig,
    cols: u16,
    rows: u16,
    id: u32,
    app: &AppHandle,
    input_rx: &std::sync::mpsc::Receiver<SshMsg>,
) -> Result<SshOutcome, Box<dyn std::error::Error>> {
    use std::net::TcpStream;

    let addr = format!("{}:{}", config.host, config.port);
    let socket_addr: std::net::SocketAddr = addr.parse()
        .or_else(|_| {
            use std::net::ToSocketAddrs;
            addr.to_socket_addrs()?.next().ok_or_else(|| std::io::Error::other("DNS 실패"))
        })
        .map_err(|e| -> Box<dyn std::error::Error> {
            format!("호스트를 찾을 수 없음: {} ({})", addr, e).into()
        })?;
    let tcp = TcpStream::connect_timeout(&socket_addr, std::time::Duration::from_secs(10))
        .map_err(|e| -> Box<dyn std::error::Error> {
            format!("서버에 연결할 수 없음: {} ({})", addr, e).into()
        })?;

    let mut session = ssh2::Session::new()?;
    session.set_tcp_stream(tcp);
    session.handshake().map_err(|e| -> Box<dyn std::error::Error> {
        format!("SSH 핸드셰이크 실패: {}", e).into()
    })?;
    session.set_keepalive(true, 30);
    session.set_timeout(30_000);

    verify_host_key(&session, &config.host, config.port)?;

    // 인증
    let mut authenticated = false;
    let is_password_mode = config.auth_type.eq_ignore_ascii_case("password");

    if !is_password_mode {
        // 1. ssh-agent
        if let Ok(mut agent) = session.agent() {
            if agent.connect().is_ok() {
                let _ = agent.list_identities();
                if let Ok(identities) = agent.identities() {
                    for identity in &identities {
                        if agent.userauth(&config.username, identity).is_ok() && session.authenticated() {
                            authenticated = true;
                            break;
                        }
                    }
                }
            }
        }

        // 2. 사용자 지정 키 경로 우선 (프롬프트 허용)
        if !authenticated && !config.key_path.trim().is_empty() {
            let path = expand_path(&config.key_path);
            if path.exists() && try_key_auth(&session, &config.username, &path, true) {
                authenticated = true;
            }
        }

        // 3. 기본 키 파일 (passphrase 없이만 조용히)
        if !authenticated {
            let home = std::env::var("HOME").unwrap_or_default();
            for key in &["id_ed25519", "id_rsa", "id_ecdsa"] {
                let path = std::path::PathBuf::from(&home).join(".ssh").join(key);
                if path.exists() && try_key_auth(&session, &config.username, &path, false) {
                    authenticated = true;
                    break;
                }
            }
        }
    }

    // 3. 키체인 비밀번호 (key 모드면 건너뜀)
    let is_key_mode = config.auth_type.eq_ignore_ascii_case("key");
    if !authenticated && !is_key_mode {
        if let Some(password) = keychain_get_password(&config.host, &config.username) {
            if session.userauth_password(&config.username, &password).is_ok() && session.authenticated() {
                authenticated = true;
            }
        }
    }

    // 4. 비밀번호 프롬프트 (key 모드면 건너뜀)
    if !authenticated && !is_key_mode {
        if let Some(password) = prompt_password(&config.host, &config.username) {
            session.userauth_password(&config.username, &password)?;
            if session.authenticated() {
                keychain_save_password(&config.host, &config.username, &password);
                authenticated = true;
            }
        }
    }

    if !authenticated {
        return Err("인증 실패".into());
    }

    let mut channel = session.channel_session()?;
    channel.request_pty("xterm-256color", None, Some((cols as u32, rows as u32, 0, 0)))?;
    channel.shell()?;

    session.set_blocking(false);

    let mut buf = [0u8; 8192];
    let mut osc_state: u8 = 0;
    let mut osc_buf: Vec<u8> = Vec::new();
    let mut last_keepalive = std::time::Instant::now();
    let mut consecutive_errors: u32 = 0;
    let mut disconnect_reason: Option<String> = None;

    loop {
        match channel.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                consecutive_errors = 0;
                let data = &buf[..n];

                // OSC 타이틀 파싱
                for &byte in data {
                    match osc_state {
                        0 => { if byte == 0x1b { osc_state = 1; } }
                        1 => {
                            if byte == 0x5d { osc_state = 2; osc_buf.clear(); }
                            else { osc_state = 0; }
                        }
                        2 => {
                            if byte == 0x07 {
                                if let Ok(s) = std::str::from_utf8(&osc_buf) {
                                    if s.starts_with("0;") || s.starts_with("2;") {
                                        let _ = app.emit("pty-title", serde_json::json!({
                                            "id": id, "title": &s[2..],
                                        }));
                                    } else if let Some(rest) = s.strip_prefix("7;") {
                                        // OSC 7: file://<host>/<path> → 경로만 추출해 트리에 전달
                                        let body = rest.strip_prefix("file://").unwrap_or(rest);
                                        let path = match body.find('/') {
                                            Some(i) => &body[i..],
                                            None => body,
                                        };
                                        if !path.is_empty() {
                                            let _ = app.emit("pty-cwd", serde_json::json!({
                                                "id": id, "cwd": path,
                                            }));
                                        }
                                    }
                                }
                                osc_state = 0;
                            } else if osc_buf.len() < 512 {
                                osc_buf.push(byte);
                            } else { osc_state = 0; }
                        }
                        _ => { osc_state = 0; }
                    }
                }

                let _ = app.emit("pty-output", serde_json::json!({
                    "id": id,
                    "data": data.to_vec(),
                }));
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                consecutive_errors = 0;
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {
                consecutive_errors += 1;
                if consecutive_errors >= 3 {
                    log::warn!("SSH tab {}: 연속 타임아웃 {}회, 연결 종료", id, consecutive_errors);
                    disconnect_reason = Some("서버 응답 없음 (타임아웃)".to_string());
                    break;
                }
            }
            Err(e) => {
                log::warn!("SSH tab {}: read 에러: {}", id, e);
                disconnect_reason = Some(format!("연결 끊김: {}", e));
                break;
            }
        }

        // 입력/리사이즈 수신
        while let Ok(msg) = input_rx.try_recv() {
            match msg {
                SshMsg::Input(data) => {
                    let _ = channel.write_all(&data);
                    let _ = channel.flush();
                }
                SshMsg::Resize(cols, rows) => {
                    let _ = channel.request_pty_size(cols as u32, rows as u32, None, None);
                }
            }
        }

        if channel.eof() { break; }

        // keepalive 전송 (30초마다)
        if last_keepalive.elapsed() >= std::time::Duration::from_secs(30) {
            if let Err(e) = session.keepalive_send() {
                log::warn!("SSH tab {}: keepalive 실패: {}", id, e);
                disconnect_reason = Some(format!("keepalive 실패: {}", e));
                break;
            }
            last_keepalive = std::time::Instant::now();
        }

        thread::sleep(std::time::Duration::from_millis(10));
    }

    // 종료 사유를 호출자(재연결 루프)에게 반환. 이벤트 emit은 루프가 담당.
    match disconnect_reason {
        Some(reason) => Ok(SshOutcome::Disconnected(reason)),
        None => Ok(SshOutcome::Closed),
    }
}

/// ~ 또는 $HOME 확장
fn expand_path(p: &str) -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    expand_path_with_home(p, &home)
}

/// `~`, `~/`, `$HOME` 확장. 환경변수 의존을 분리해 단위 테스트가 가능하도록 home을 인자로 받음.
fn expand_path_with_home(p: &str, home: &str) -> std::path::PathBuf {
    let trimmed = p.trim();
    if trimmed.is_empty() {
        return std::path::PathBuf::new();
    }
    let expanded = if trimmed == "~" {
        home.to_string()
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        format!("{}/{}", home, rest)
    } else if trimmed.contains("$HOME") {
        trimmed.replace("$HOME", home)
    } else {
        trimmed.to_string()
    };
    std::path::PathBuf::from(expanded)
}

/// 키 파일 인증. `interactive=true`일 때만 passphrase 프롬프트를 띄움.
fn try_key_auth(
    session: &ssh2::Session,
    username: &str,
    path: &std::path::Path,
    interactive: bool,
) -> bool {
    if let Err(e) = std::fs::metadata(path) {
        log::warn!("키 파일 접근 실패 {}: {}", path.display(), e);
        return false;
    }
    match session.userauth_pubkey_file(username, None, path, None) {
        Ok(_) if session.authenticated() => return true,
        Ok(_) => {}
        Err(e) => log::info!("키 인증 None passphrase 실패 {}: {}", path.display(), e),
    }
    let key_id = path.to_string_lossy().to_string();
    if let Some(pp) = keychain_get_key_passphrase(&key_id) {
        if session.userauth_pubkey_file(username, None, path, Some(&pp)).is_ok()
            && session.authenticated()
        {
            return true;
        }
    }
    if !interactive {
        return false;
    }
    if let Some(pp) = prompt_key_passphrase(&key_id) {
        if session.userauth_pubkey_file(username, None, path, Some(&pp)).is_ok()
            && session.authenticated()
        {
            keychain_save_key_passphrase(&key_id, &pp);
            return true;
        }
    }
    false
}

fn keychain_get_key_passphrase(key_id: &str) -> Option<String> {
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-a", key_id, "-s", "termy-sshkey", "-w"])
        .output().ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else { None }
}

fn keychain_save_key_passphrase(key_id: &str, passphrase: &str) {
    let _ = std::process::Command::new("security")
        .args(["delete-generic-password", "-a", key_id, "-s", "termy-sshkey"])
        .output();
    let _ = std::process::Command::new("security")
        .args(["add-generic-password", "-a", key_id, "-s", "termy-sshkey", "-w", passphrase])
        .output();
}

fn prompt_key_passphrase(key_id: &str) -> Option<String> {
    let safe_key = key_id.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        r#"display dialog "키 passphrase ({}):" default answer "" with hidden answer with title "termy SSH" buttons {{"취소", "확인"}} default button "확인""#,
        safe_key
    );
    let output = std::process::Command::new("osascript")
        .args(["-e", &script]).output().ok()?;
    if !output.status.success() { return None; }
    String::from_utf8_lossy(&output.stdout)
        .split("text returned:").nth(1)
        .map(|s| s.trim().to_string())
}

fn keychain_get_password(host: &str, username: &str) -> Option<String> {
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-a", username, "-s", &format!("termy-ssh-{}", host), "-w"])
        .output().ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else { None }
}

fn keychain_save_password(host: &str, username: &str, password: &str) {
    let _ = std::process::Command::new("security")
        .args(["delete-generic-password", "-a", username, "-s", &format!("termy-ssh-{}", host)])
        .output();
    let _ = std::process::Command::new("security")
        .args(["add-generic-password", "-a", username, "-s", &format!("termy-ssh-{}", host), "-w", password])
        .output();
}

fn prompt_password(host: &str, username: &str) -> Option<String> {
    // AppleScript injection 방지: 큰따옴표와 백슬래시 escape
    let safe_user = username.replace('\\', "\\\\").replace('"', "\\\"");
    let safe_host = host.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        r#"display dialog "{}@{} 비밀번호:" default answer "" with hidden answer with title "termy SSH" buttons {{"취소", "연결"}} default button "연결""#,
        safe_user, safe_host
    );
    let output = std::process::Command::new("osascript")
        .args(["-e", &script]).output().ok()?;
    if !output.status.success() { return None; }
    String::from_utf8_lossy(&output.stdout)
        .split("text returned:").nth(1)
        .map(|s| s.trim().to_string())
}

/// PTY/SSH에 데이터 쓰기
#[tauri::command]
fn pty_write(id: u32, data: String, state: State<'_, PtyState>) {
    if let Ok(mut mgr) = state.lock() {
        // 로컬 PTY
        if let Some(pty) = mgr.ptys.get_mut(&id) {
            let _ = pty.writer.write_all(data.as_bytes());
            let _ = pty.writer.flush();
            return;
        }
        // SSH
        if let Some(tx) = mgr.ssh_inputs.get(&id) {
            let _ = tx.send(SshMsg::Input(data.as_bytes().to_vec()));
        }
    }
}

/// PTY/SSH 리사이즈
#[tauri::command]
fn pty_resize(id: u32, cols: u16, rows: u16, state: State<'_, PtyState>) {
    if let Ok(mgr) = state.lock() {
        // 로컬 PTY
        if let Some(pty) = mgr.ptys.get(&id) {
            let _ = pty.master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
            return;
        }
        // SSH
        if let Some(tx) = mgr.ssh_inputs.get(&id) {
            let _ = tx.send(SshMsg::Resize(cols, rows));
        }
    }
}

/// PTY 닫기
#[tauri::command]
fn pty_close(id: u32, state: State<'_, PtyState>) {
    if let Ok(mut mgr) = state.lock() {
        mgr.ptys.remove(&id);
    }
}

/// 파일 정보
#[derive(Serialize)]
struct FileEntry {
    name: String,
    is_dir: bool,
    size: u64,
    path: String,
}

/// SFTP 디렉토리 목록
#[tauri::command]
fn sftp_list_dir(id: u32, path: Option<String>, sftp_state: State<'_, SftpState>) -> Result<Vec<FileEntry>, String> {
    let mut sftps = sftp_state.lock().map_err(|e| e.to_string())?;
    let sftp_handle = sftps.get_mut(&id).ok_or("SFTP 연결 없음")?;

    sftp_handle.session.set_timeout(30_000);
    sftp_handle.session.set_blocking(true);
    let dir_path = path.unwrap_or_else(|| sftp_handle.cwd.clone());
    let sftp = sftp_handle.session.sftp().map_err(|e| e.to_string())?;

    let entries = sftp.readdir(std::path::Path::new(&dir_path))
        .map_err(|e| format!("디렉토리 읽기 실패: {}", e))?;

    let mut files: Vec<FileEntry> = entries
        .into_iter()
        .filter_map(|(path_buf, stat)| {
            let name = path_buf.file_name()?.to_string_lossy().to_string();
            if name == "." || name == ".." { return None; }
            Some(FileEntry {
                is_dir: stat.is_dir(),
                size: stat.size.unwrap_or(0),
                path: path_buf.to_string_lossy().to_string(),
                name,
            })
        })
        .collect();

    // 폴더 먼저, 이름순
    files.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    sftp_handle.cwd = dir_path;
    sftp_handle.session.set_blocking(false);

    Ok(files)
}

/// SFTP 현재 경로 (저장된 cwd 반환)
#[tauri::command]
fn sftp_get_cwd(id: u32, sftp_state: State<'_, SftpState>) -> Result<String, String> {
    let sftps = sftp_state.lock().map_err(|e| e.to_string())?;
    let sftp_handle = sftps.get(&id).ok_or("SFTP 연결 없음")?;
    Ok(sftp_handle.cwd.clone())
}

/// SSH 셸의 실제 pwd 실행 (새로고침 버튼 전용)
#[tauri::command]
fn sftp_sync_cwd(id: u32, sftp_state: State<'_, SftpState>) -> Result<String, String> {
    let mut sftps = sftp_state.lock().map_err(|e| e.to_string())?;
    let sftp_handle = sftps.get_mut(&id).ok_or("SFTP 연결 없음")?;

    sftp_handle.session.set_timeout(30_000);
    sftp_handle.session.set_blocking(true);
    let result = (|| -> Result<String, Box<dyn std::error::Error>> {
        let mut channel = sftp_handle.session.channel_session()?;
        channel.exec("pwd")?;
        let mut output = String::new();
        channel.read_to_string(&mut output)?;
        channel.wait_close()?;
        let pwd = output.trim().to_string();
        if pwd.is_empty() { Err("pwd 빈 결과".into()) } else { Ok(pwd) }
    })();
    sftp_handle.session.set_blocking(false);

    match result {
        Ok(pwd) => {
            sftp_handle.cwd = pwd.clone();
            Ok(pwd)
        }
        Err(_) => Ok(sftp_handle.cwd.clone()),
    }
}

/// SFTP 파일 다운로드 (별도 스레드 + 이벤트 기반 진행률)
#[tauri::command]
fn sftp_download(id: u32, remote_path: String, local_path: String, sftp_state: State<'_, SftpState>, app: AppHandle) -> Result<(), String> {
    // 연결 존재 여부만 빠르게 확인
    {
        let sftps = sftp_state.lock().map_err(|e| e.to_string())?;
        if !sftps.contains_key(&id) {
            return Err("SFTP 연결 없음".to_string());
        }
    }

    let file_name = std::path::Path::new(&remote_path)
        .file_name().map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| remote_path.clone());

    let sftp_state_clone = sftp_state.inner().clone();
    let app_clone = app.clone();

    thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            let mut sftps = sftp_state_clone.lock().map_err(|e| e.to_string())?;
            let sftp_handle = sftps.get_mut(&id).ok_or("SFTP 연결 없음")?;

            sftp_handle.session.set_timeout(60_000);
            sftp_handle.session.set_blocking(true);
            let transfer_result = (|| -> Result<(), String> {
                let sftp = sftp_handle.session.sftp().map_err(|e| e.to_string())?;

                let stat = sftp.stat(std::path::Path::new(&remote_path)).map_err(|e| e.to_string())?;
                let total_size = stat.size.unwrap_or(0);

                let mut remote_file = sftp.open(std::path::Path::new(&remote_path))
                    .map_err(|e| format!("파일 열기 실패: {}", e))?;

                let mut local_file = std::fs::File::create(&local_path).map_err(|e| e.to_string())?;
                let mut downloaded: u64 = 0;
                let mut buf = [0u8; 32768];

                loop {
                    let n = remote_file.read(&mut buf).map_err(|e| e.to_string())?;
                    if n == 0 { break; }
                    local_file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
                    downloaded += n as u64;

                    let progress = if total_size > 0 { (downloaded as f64 / total_size as f64 * 100.0) as u32 } else { 0 };
                    let _ = app_clone.emit("transfer-progress", serde_json::json!({
                        "type": "download",
                        "tabId": id,
                        "name": file_name,
                        "progress": progress,
                        "downloaded": downloaded,
                        "total": total_size,
                    }));
                }

                let _ = app_clone.emit("transfer-complete", serde_json::json!({
                    "type": "download",
                    "tabId": id,
                    "name": file_name,
                }));

                Ok(())
            })();
            sftp_handle.session.set_blocking(false);
            transfer_result
        })();

        if let Err(e) = result {
            // 불완전한 파일 정리
            let _ = std::fs::remove_file(&local_path);
            let _ = app_clone.emit("transfer-error", serde_json::json!({
                "type": "download",
                "tabId": id,
                "name": file_name,
                "error": e,
            }));
        }
    });

    Ok(())
}

/// SFTP 파일 업로드 (별도 스레드 + 이벤트 기반 진행률)
#[tauri::command]
fn sftp_upload(id: u32, local_path: String, remote_path: String, sftp_state: State<'_, SftpState>, app: AppHandle) -> Result<(), String> {
    // 연결 존재 여부만 빠르게 확인
    {
        let sftps = sftp_state.lock().map_err(|e| e.to_string())?;
        if !sftps.contains_key(&id) {
            return Err("SFTP 연결 없음".to_string());
        }
    }

    let file_name = std::path::Path::new(&local_path)
        .file_name().map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| local_path.clone());

    let sftp_state_clone = sftp_state.inner().clone();
    let app_clone = app.clone();

    thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            // 로컬 파일은 lock 밖에서 미리 읽기
            let contents = std::fs::read(&local_path).map_err(|e| e.to_string())?;
            let total_size = contents.len() as u64;

            let mut sftps = sftp_state_clone.lock().map_err(|e| e.to_string())?;
            let sftp_handle = sftps.get_mut(&id).ok_or("SFTP 연결 없음")?;

            sftp_handle.session.set_timeout(60_000);
            sftp_handle.session.set_blocking(true);
            let transfer_result = (|| -> Result<(), String> {
                let sftp = sftp_handle.session.sftp().map_err(|e| e.to_string())?;

                let mut remote_file = sftp.create(std::path::Path::new(&remote_path))
                    .map_err(|e| format!("파일 생성 실패: {}", e))?;

                let mut uploaded: u64 = 0;
                for chunk in contents.chunks(32768) {
                    remote_file.write_all(chunk).map_err(|e| e.to_string())?;
                    uploaded += chunk.len() as u64;

                    let progress = if total_size > 0 { (uploaded as f64 / total_size as f64 * 100.0) as u32 } else { 0 };
                    let _ = app_clone.emit("transfer-progress", serde_json::json!({
                        "type": "upload",
                        "tabId": id,
                        "name": file_name,
                        "progress": progress,
                        "uploaded": uploaded,
                        "total": total_size,
                    }));
                }

                let _ = app_clone.emit("transfer-complete", serde_json::json!({
                    "type": "upload",
                    "tabId": id,
                    "name": file_name,
                }));

                Ok(())
            })();
            sftp_handle.session.set_blocking(false);
            transfer_result
        })();

        if let Err(e) = result {
            let _ = app_clone.emit("transfer-error", serde_json::json!({
                "type": "upload",
                "tabId": id,
                "name": file_name,
                "error": e,
            }));
        }
    });

    Ok(())
}

/// PTY 셸의 현재 작업 디렉토리 가져오기 (macOS: lsof 사용)
#[tauri::command]
fn get_pty_cwd(id: u32, state: State<'_, PtyState>) -> Result<String, String> {
    let mgr = state.lock().map_err(|e| e.to_string())?;
    let pty = mgr.ptys.get(&id).ok_or("PTY 없음")?;

    let pid = pty.child.process_id().ok_or("PID 없음")?;

    // macOS: lsof로 프로세스의 cwd 가져오기
    let output = std::process::Command::new("lsof")
        .args(["-p", &pid.to_string(), "-Fn"])
        .output()
        .map_err(|e| e.to_string())?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if line.starts_with('n') && line.contains('/') {
            let path = &line[1..];
            // cwd는 보통 첫 번째 디렉토리 경로
            if std::path::Path::new(path).is_dir() {
                return Ok(path.to_string());
            }
        }
    }

    Err("CWD를 찾을 수 없음".to_string())
}

/// 사용자 홈 디렉토리
#[tauri::command]
fn home_dir() -> Result<String, String> {
    dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| "홈 디렉토리 없음".to_string())
}

/// 로컬 디렉토리 목록
#[tauri::command]
fn local_list_dir(path: String, show_hidden: Option<bool>) -> Result<Vec<FileEntry>, String> {
    let dir = std::path::Path::new(&path);
    if !dir.exists() {
        return Err("경로 없음".to_string());
    }
    let show_hidden = show_hidden.unwrap_or(false);

    let entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;

    let mut files: Vec<FileEntry> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().to_string();
            if !show_hidden && name.starts_with('.') { return None; }
            let metadata = entry.metadata().ok()?;
            Some(FileEntry {
                name,
                is_dir: metadata.is_dir(),
                size: metadata.len(),
                path: entry.path().to_string_lossy().to_string(),
            })
        })
        .collect();

    files.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(files)
}

/// SSH 연결 목록
#[tauri::command]
fn get_ssh_connections() -> Vec<SshConfig> {
    let path = dirs::home_dir()
        .unwrap_or_default()
        .join(".config/termy/ssh_connections.json");
    if let Ok(data) = std::fs::read_to_string(&path) {
        serde_json::from_str(&data).unwrap_or_default()
    } else {
        vec![]
    }
}

/// SFTP 파일 삭제
#[tauri::command]
fn sftp_delete(id: u32, remote_path: String, is_dir: bool, sftp_state: State<'_, SftpState>) -> Result<(), String> {
    let mut sftps = sftp_state.lock().map_err(|e| e.to_string())?;
    let sftp_handle = sftps.get_mut(&id).ok_or("SFTP 연결 없음")?;
    sftp_handle.session.set_timeout(30_000);
    sftp_handle.session.set_blocking(true);
    let result = (|| -> Result<(), String> {
        let sftp = sftp_handle.session.sftp().map_err(|e| e.to_string())?;
        let path = std::path::Path::new(&remote_path);
        if is_dir {
            sftp.rmdir(path).map_err(|e| e.to_string())?;
        } else {
            sftp.unlink(path).map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    sftp_handle.session.set_blocking(false);
    result
}

/// SFTP 이름 변경
#[tauri::command]
fn sftp_rename(id: u32, old_path: String, new_path: String, sftp_state: State<'_, SftpState>) -> Result<(), String> {
    let mut sftps = sftp_state.lock().map_err(|e| e.to_string())?;
    let sftp_handle = sftps.get_mut(&id).ok_or("SFTP 연결 없음")?;
    sftp_handle.session.set_timeout(30_000);
    sftp_handle.session.set_blocking(true);
    let result = (|| -> Result<(), String> {
        let sftp = sftp_handle.session.sftp().map_err(|e| e.to_string())?;
        sftp.rename(std::path::Path::new(&old_path), std::path::Path::new(&new_path), None)
            .map_err(|e| e.to_string())?;
        Ok(())
    })();
    sftp_handle.session.set_blocking(false);
    result
}

/// 원격 파일 → 로컬 경로로 읽기 (SftpState 락을 짧게 점유)
fn sftp_read_to_local(
    id: u32,
    remote_path: &str,
    local_path: &std::path::Path,
    sftp_state: &SftpState,
) -> Result<(), String> {
    let mut sftps = sftp_state.lock().map_err(|e| e.to_string())?;
    let h = sftps.get_mut(&id).ok_or("SFTP 연결 없음")?;
    h.session.set_timeout(60_000);
    h.session.set_blocking(true);
    let r = (|| -> Result<(), String> {
        let sftp = h.session.sftp().map_err(|e| e.to_string())?;
        let mut rf = sftp
            .open(std::path::Path::new(remote_path))
            .map_err(|e| format!("원격 파일 열기 실패: {}", e))?;
        let mut buf = Vec::new();
        rf.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        std::fs::write(local_path, &buf).map_err(|e| e.to_string())?;
        Ok(())
    })();
    h.session.set_blocking(false);
    r
}

/// 로컬 파일 → 원격 경로로 쓰기 (편집 자동 동기화용)
fn sftp_write_from_local(
    id: u32,
    remote_path: &str,
    local_path: &std::path::Path,
    sftp_state: &SftpState,
) -> Result<(), String> {
    let contents = std::fs::read(local_path).map_err(|e| e.to_string())?;
    let mut sftps = sftp_state.lock().map_err(|e| e.to_string())?;
    let h = sftps.get_mut(&id).ok_or("SFTP 연결 없음")?;
    h.session.set_timeout(60_000);
    h.session.set_blocking(true);
    let r = (|| -> Result<(), String> {
        let sftp = h.session.sftp().map_err(|e| e.to_string())?;
        let mut wf = sftp
            .create(std::path::Path::new(remote_path))
            .map_err(|e| format!("원격 파일 생성 실패: {}", e))?;
        wf.write_all(&contents).map_err(|e| e.to_string())?;
        Ok(())
    })();
    h.session.set_blocking(false);
    r
}

/// 원격 파일을 로컬 에디터(Sublime Text)로 열고, 저장될 때마다 서버에 자동 업로드.
/// 임시 파일을 감시하다 mtime이 바뀌면 업로드한다. SFTP 세션이 사라지면 감시 종료.
#[tauri::command]
fn sftp_edit(
    id: u32,
    remote_path: String,
    sftp_state: State<'_, SftpState>,
    app: AppHandle,
) -> Result<(), String> {
    let base = std::path::Path::new(&remote_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());

    // 임시 위치: <tmp>/termy-edit/<tabId>/<파일명> (확장자 보존 → 문법 강조 유지)
    let dir = std::env::temp_dir().join("termy-edit").join(id.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let local = dir.join(&base);

    // 1. 다운로드
    sftp_read_to_local(id, &remote_path, &local, sftp_state.inner())?;

    // 2. Sublime Text로 열기
    let opened = std::process::Command::new("open")
        .args(["-a", "Sublime Text", &local.to_string_lossy()])
        .status();
    match opened {
        Ok(s) if s.success() => {}
        _ => return Err("Sublime Text를 열 수 없습니다 (설치 여부 확인)".to_string()),
    }

    // 3. 저장(mtime 변경) 감시 → 자동 업로드
    let sftp_state_clone = Arc::clone(sftp_state.inner());
    let app_clone = app.clone();
    let local_w = local.clone();
    let remote_w = remote_path.clone();
    let base_w = base.clone();
    thread::spawn(move || {
        let mut last = std::fs::metadata(&local_w)
            .ok()
            .and_then(|m| m.modified().ok());
        loop {
            thread::sleep(std::time::Duration::from_secs(1));
            // 탭/접속이 사라지면 감시 종료
            let alive = sftp_state_clone
                .lock()
                .map(|m| m.contains_key(&id))
                .unwrap_or(false);
            if !alive {
                break;
            }
            let cur = std::fs::metadata(&local_w)
                .ok()
                .and_then(|m| m.modified().ok());
            if cur.is_some() && cur != last {
                last = cur;
                match sftp_write_from_local(id, &remote_w, &local_w, &sftp_state_clone) {
                    Ok(_) => {
                        let _ = app_clone
                            .emit("edit-synced", serde_json::json!({ "name": base_w }));
                    }
                    Err(e) => {
                        let _ = app_clone.emit(
                            "edit-error",
                            serde_json::json!({ "name": base_w, "error": e }),
                        );
                    }
                }
            }
        }
        log::info!("편집 감시 종료: {}", base_w);
    });

    Ok(())
}

/// 로컬 파일 삭제
#[tauri::command]
fn local_delete(path: String, is_dir: bool) -> Result<(), String> {
    if is_dir {
        std::fs::remove_dir_all(&path).map_err(|e| e.to_string())?;
    } else {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 로컬 이름 변경
#[tauri::command]
fn local_rename(old_path: String, new_path: String) -> Result<(), String> {
    std::fs::rename(&old_path, &new_path).map_err(|e| e.to_string())?;
    Ok(())
}

/// 앱 설정 저장
#[tauri::command]
fn save_app_settings(settings: serde_json::Value) -> Result<(), String> {
    let path = dirs::home_dir()
        .unwrap_or_default()
        .join(".config/termy/settings.json");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let data = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(&path, data).map_err(|e| e.to_string())?;
    Ok(())
}

/// 앱 설정 로드
#[tauri::command]
fn load_app_settings() -> Result<serde_json::Value, String> {
    let path = dirs::home_dir()
        .unwrap_or_default()
        .join(".config/termy/settings.json");
    if path.exists() {
        let data = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str(&data).map_err(|e| e.to_string())
    } else {
        Ok(serde_json::json!({}))
    }
}

/// 비밀번호를 키체인에 저장
#[tauri::command]
fn save_password_to_keychain(host: String, username: String, password: String) -> Result<(), String> {
    keychain_save_password(&host, &username, &password);
    Ok(())
}

/// SSH 연결 저장
#[tauri::command]
fn save_ssh_connections(connections: Vec<SshConfig>) -> Result<(), String> {
    let path = dirs::home_dir()
        .unwrap_or_default()
        .join(".config/termy/ssh_connections.json");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let data = serde_json::to_string_pretty(&connections).map_err(|e| e.to_string())?;
    std::fs::write(&path, data).map_err(|e| e.to_string())?;
    Ok(())
}

/// 설정 UI 열기
#[tauri::command]
fn open_settings() {
    let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();

    let settings_path = base.join("termy-settings/.build/debug/termy-settings");

    if settings_path.exists() {
        let _ = std::process::Command::new(&settings_path).spawn();
    } else {
        // 빌드 안 되어 있으면 빌드 후 실행
        let settings_dir = base.join("termy-settings");
        let _ = std::process::Command::new("swift")
            .args(["build"])
            .current_dir(&settings_dir)
            .status();
        if settings_path.exists() {
            let _ = std::process::Command::new(&settings_path).spawn();
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// biy → termy 마이그레이션 (1회성)
/// ~/.config/biy/* → ~/.config/termy/* 로 복사 (termy 디렉터리가 비어있을 때만)
fn migrate_from_biy() {
    let Some(home) = dirs::home_dir() else { return };
    let old_dir = home.join(".config/biy");
    let new_dir = home.join(".config/termy");

    if !old_dir.exists() {
        return;
    }
    if new_dir.exists() && std::fs::read_dir(&new_dir).map(|mut d| d.next().is_some()).unwrap_or(false) {
        // termy 디렉터리에 이미 파일이 있으면 덮어쓰지 않음
        return;
    }

    if let Err(e) = std::fs::create_dir_all(&new_dir) {
        log::warn!("migrate: create_dir_all 실패: {}", e);
        return;
    }

    for entry in std::fs::read_dir(&old_dir).into_iter().flatten().flatten() {
        let src = entry.path();
        let Some(file_name) = src.file_name() else { continue };
        let dst = new_dir.join(file_name);
        if src.is_file() {
            if let Err(e) = std::fs::copy(&src, &dst) {
                log::warn!("migrate: copy 실패 {:?}: {}", src, e);
            } else {
                log::info!("migrate: {:?} → {:?}", src, dst);
            }
        }
    }
}

pub fn run() {
    migrate_from_biy();
    tauri::Builder::default()
        .manage(Arc::new(Mutex::new(PtyManager {
            ptys: HashMap::new(),
            ssh_inputs: HashMap::new(),
        })))
        .manage(Arc::new(Mutex::new(HashMap::<u32, SftpHandle>::new())))
        .plugin(tauri_plugin_log::Builder::default().level(log::LevelFilter::Info).build())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            create_pty,
            pty_write,
            pty_resize,
            pty_close,
            get_ssh_connections,
            save_ssh_connections,
            save_password_to_keychain,
            save_app_settings,
            load_app_settings,
            sftp_delete,
            sftp_rename,
            local_delete,
            local_rename,
            open_settings,
            sftp_list_dir,
            sftp_get_cwd,
            sftp_sync_cwd,
            sftp_download,
            sftp_upload,
            sftp_edit,
            local_list_dir,
            home_dir,
            get_pty_cwd,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const HOME: &str = "/Users/tester";

    #[test]
    fn expand_empty_and_blank() {
        assert_eq!(expand_path_with_home("", HOME), PathBuf::new());
        assert_eq!(expand_path_with_home("   ", HOME), PathBuf::new());
    }

    #[test]
    fn expand_tilde_alone() {
        assert_eq!(expand_path_with_home("~", HOME), PathBuf::from(HOME));
    }

    #[test]
    fn expand_tilde_slash() {
        assert_eq!(
            expand_path_with_home("~/.ssh/id_rsa", HOME),
            PathBuf::from("/Users/tester/.ssh/id_rsa")
        );
    }

    #[test]
    fn expand_home_var() {
        assert_eq!(
            expand_path_with_home("$HOME/keys/server.pem", HOME),
            PathBuf::from("/Users/tester/keys/server.pem")
        );
        assert_eq!(expand_path_with_home("$HOME", HOME), PathBuf::from(HOME));
    }

    #[test]
    fn expand_absolute_unchanged() {
        assert_eq!(
            expand_path_with_home("/etc/ssh/key", HOME),
            PathBuf::from("/etc/ssh/key")
        );
    }

    #[test]
    fn expand_relative_unchanged() {
        assert_eq!(
            expand_path_with_home("keys/server.pem", HOME),
            PathBuf::from("keys/server.pem")
        );
    }

    #[test]
    fn expand_trims_surrounding_whitespace() {
        assert_eq!(
            expand_path_with_home("  ~/key.pem  ", HOME),
            PathBuf::from("/Users/tester/key.pem")
        );
    }

    #[test]
    fn expand_tilde_user_not_expanded() {
        // `~other` 형식은 홈 확장 대상이 아니라 그대로 둔다.
        assert_eq!(
            expand_path_with_home("~other/key", HOME),
            PathBuf::from("~other/key")
        );
    }
}
