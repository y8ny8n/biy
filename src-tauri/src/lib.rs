use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

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

/// SFTP 핸들 (SSH 연결과 연동)
struct SftpHandle {
    session: ssh2::Session,
    cwd: String,
}

/// 전체 PTY + SFTP 관리
struct PtyManager {
    ptys: HashMap<u32, PtyHandle>,
    ssh_inputs: HashMap<u32, SshInputSender>,
    sftps: HashMap<u32, SftpHandle>,
}

type PtyState = Arc<Mutex<PtyManager>>;

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
fn create_pty(
    id: u32,
    cols: u16,
    rows: u16,
    tab_type: String,
    ssh_config: Option<SshConfig>,
    state: State<'_, PtyState>,
    app: AppHandle,
) -> Result<u32, String> {
    if tab_type == "ssh" {
        if let Some(config) = ssh_config {
            return create_ssh_pty(id, cols, rows, config, state, app);
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
    app: AppHandle,
) -> Result<u32, String> {
    // 1. SFTP용 SSH 세션을 먼저 동기적으로 생성 (인증 포함)
    //    이 과정에서 비밀번호가 키체인에 저장됨
    match create_sftp_session(&config) {
        Ok(sftp_session) => {
            // 홈 디렉토리 가져오기
            sftp_session.set_blocking(true);
            let cwd = (|| -> Option<String> {
                let mut ch = sftp_session.channel_session().ok()?;
                ch.exec("pwd").ok()?;
                let mut out = String::new();
                ch.read_to_string(&mut out).ok()?;
                ch.wait_close().ok()?;
                let p = out.trim().to_string();
                if p.is_empty() { None } else { Some(p) }
            })().unwrap_or_else(|| "/root".to_string());

            sftp_session.set_blocking(false);

            if let Ok(mut mgr) = state.lock() {
                mgr.sftps.insert(id, SftpHandle {
                    session: sftp_session,
                    cwd,
                });
            }
            log::info!("SFTP session created for tab {}", id);
        }
        Err(e) => {
            log::warn!("SFTP session failed: {} (file tree won't work)", e);
        }
    }

    // 2. SSH 입력 채널 생성
    let (input_tx, input_rx) = std::sync::mpsc::channel::<SshMsg>();
    {
        if let Ok(mut mgr) = state.lock() {
            mgr.ssh_inputs.insert(id, input_tx);
        }
    }

    // 3. SSH 터미널 연결 (별도 스레드)
    let app_clone = app.clone();
    let state_clone = Arc::clone(state.inner());
    thread::spawn(move || {
        match ssh_session(&config, cols, rows, id, &app_clone, input_rx) {
            Ok(_) => {}
            Err(e) => {
                log::error!("SSH error: {}", e);
                let _ = app_clone.emit("pty-exit", serde_json::json!({ "id": id }));
            }
        }
        if let Ok(mut mgr) = state_clone.lock() {
            mgr.ssh_inputs.remove(&id);
            mgr.sftps.remove(&id);
        }
    });

    Ok(id)
}

/// SFTP용 SSH 세션 생성
fn create_sftp_session(config: &SshConfig) -> Result<ssh2::Session, Box<dyn std::error::Error>> {
    use std::net::TcpStream;

    let addr = format!("{}:{}", config.host, config.port);
    let tcp = TcpStream::connect(&addr)?;

    let mut session = ssh2::Session::new()?;
    session.set_tcp_stream(tcp);
    session.handshake()?;

    // 인증 (ssh_session과 동일 로직)
    let mut authenticated = false;

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

    if !authenticated {
        let home = std::env::var("HOME").unwrap_or_default();
        for key in &["id_ed25519", "id_rsa", "id_ecdsa"] {
            let path = std::path::PathBuf::from(&home).join(".ssh").join(key);
            if path.exists() {
                if session.userauth_pubkey_file(&config.username, None, &path, None).is_ok()
                    && session.authenticated()
                {
                    authenticated = true;
                    break;
                }
            }
        }
    }

    if !authenticated {
        if let Some(password) = keychain_get_password(&config.host, &config.username) {
            if session.userauth_password(&config.username, &password).is_ok() {
                authenticated = true;
            }
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
    input_rx: std::sync::mpsc::Receiver<SshMsg>,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::net::TcpStream;

    let addr = format!("{}:{}", config.host, config.port);
    let tcp = TcpStream::connect(&addr)?;

    let mut session = ssh2::Session::new()?;
    session.set_tcp_stream(tcp);
    session.handshake()?;

    // 인증
    let mut authenticated = false;

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

    // 2. 키 파일
    if !authenticated {
        let home = std::env::var("HOME").unwrap_or_default();
        for key in &["id_ed25519", "id_rsa", "id_ecdsa"] {
            let path = std::path::PathBuf::from(&home).join(".ssh").join(key);
            if path.exists() {
                if session.userauth_pubkey_file(&config.username, None, &path, None).is_ok()
                    && session.authenticated()
                {
                    authenticated = true;
                    break;
                }
            }
        }
    }

    // 3. 키체인 비밀번호
    if !authenticated {
        if let Some(password) = keychain_get_password(&config.host, &config.username) {
            if session.userauth_password(&config.username, &password).is_ok() && session.authenticated() {
                authenticated = true;
            }
        }
    }

    // 4. 비밀번호 프롬프트
    if !authenticated {
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

    loop {
        match channel.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
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
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => break,
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
        thread::sleep(std::time::Duration::from_millis(10));
    }

    let _ = app.emit("pty-exit", serde_json::json!({ "id": id }));
    Ok(())
}

fn keychain_get_password(host: &str, username: &str) -> Option<String> {
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-a", username, "-s", &format!("biy-ssh-{}", host), "-w"])
        .output().ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else { None }
}

fn keychain_save_password(host: &str, username: &str, password: &str) {
    let _ = std::process::Command::new("security")
        .args(["delete-generic-password", "-a", username, "-s", &format!("biy-ssh-{}", host)])
        .output();
    let _ = std::process::Command::new("security")
        .args(["add-generic-password", "-a", username, "-s", &format!("biy-ssh-{}", host), "-w", password])
        .output();
}

fn prompt_password(host: &str, username: &str) -> Option<String> {
    let script = format!(
        r#"display dialog "{}@{} 비밀번호:" default answer "" with hidden answer with title "biy SSH" buttons {{"취소", "연결"}} default button "연결""#,
        username, host
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
fn sftp_list_dir(id: u32, path: Option<String>, state: State<'_, PtyState>) -> Result<Vec<FileEntry>, String> {
    let mut mgr = state.lock().map_err(|e| e.to_string())?;
    let sftp_handle = mgr.sftps.get_mut(&id).ok_or("SFTP 연결 없음")?;

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
fn sftp_get_cwd(id: u32, state: State<'_, PtyState>) -> Result<String, String> {
    let mgr = state.lock().map_err(|e| e.to_string())?;
    let sftp_handle = mgr.sftps.get(&id).ok_or("SFTP 연결 없음")?;
    Ok(sftp_handle.cwd.clone())
}

/// SSH 셸의 실제 pwd 실행 (새로고침 버튼 전용)
#[tauri::command]
fn sftp_sync_cwd(id: u32, state: State<'_, PtyState>) -> Result<String, String> {
    let mut mgr = state.lock().map_err(|e| e.to_string())?;
    let sftp_handle = mgr.sftps.get_mut(&id).ok_or("SFTP 연결 없음")?;

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
fn sftp_download(id: u32, remote_path: String, local_path: String, state: State<'_, PtyState>, app: AppHandle) -> Result<(), String> {
    // 연결 존재 여부만 빠르게 확인
    {
        let mgr = state.lock().map_err(|e| e.to_string())?;
        if !mgr.sftps.contains_key(&id) {
            return Err("SFTP 연결 없음".to_string());
        }
    }

    let file_name = std::path::Path::new(&remote_path)
        .file_name().map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| remote_path.clone());

    let state_clone = state.inner().clone();
    let app_clone = app.clone();

    thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            let mut mgr = state_clone.lock().map_err(|e| e.to_string())?;
            let sftp_handle = mgr.sftps.get_mut(&id).ok_or("SFTP 연결 없음")?;

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
                        "name": file_name,
                        "progress": progress,
                        "downloaded": downloaded,
                        "total": total_size,
                    }));
                }

                let _ = app_clone.emit("transfer-complete", serde_json::json!({
                    "type": "download",
                    "name": file_name,
                }));

                Ok(())
            })();
            sftp_handle.session.set_blocking(false);
            transfer_result
        })();

        if let Err(e) = result {
            let _ = app_clone.emit("transfer-error", serde_json::json!({
                "type": "download",
                "name": file_name,
                "error": e,
            }));
        }
    });

    Ok(())
}

/// SFTP 파일 업로드 (별도 스레드 + 이벤트 기반 진행률)
#[tauri::command]
fn sftp_upload(id: u32, local_path: String, remote_path: String, state: State<'_, PtyState>, app: AppHandle) -> Result<(), String> {
    // 연결 존재 여부만 빠르게 확인
    {
        let mgr = state.lock().map_err(|e| e.to_string())?;
        if !mgr.sftps.contains_key(&id) {
            return Err("SFTP 연결 없음".to_string());
        }
    }

    let file_name = std::path::Path::new(&local_path)
        .file_name().map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| local_path.clone());

    let state_clone = state.inner().clone();
    let app_clone = app.clone();

    thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            // 로컬 파일은 lock 밖에서 미리 읽기
            let contents = std::fs::read(&local_path).map_err(|e| e.to_string())?;
            let total_size = contents.len() as u64;

            let mut mgr = state_clone.lock().map_err(|e| e.to_string())?;
            let sftp_handle = mgr.sftps.get_mut(&id).ok_or("SFTP 연결 없음")?;

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
                        "name": file_name,
                        "progress": progress,
                        "uploaded": uploaded,
                        "total": total_size,
                    }));
                }

                let _ = app_clone.emit("transfer-complete", serde_json::json!({
                    "type": "upload",
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
        .join(".config/biy/ssh_connections.json");
    if let Ok(data) = std::fs::read_to_string(&path) {
        serde_json::from_str(&data).unwrap_or_default()
    } else {
        vec![]
    }
}

/// SFTP 파일 삭제
#[tauri::command]
fn sftp_delete(id: u32, remote_path: String, is_dir: bool, state: State<'_, PtyState>) -> Result<(), String> {
    let mut mgr = state.lock().map_err(|e| e.to_string())?;
    let sftp_handle = mgr.sftps.get_mut(&id).ok_or("SFTP 연결 없음")?;
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
fn sftp_rename(id: u32, old_path: String, new_path: String, state: State<'_, PtyState>) -> Result<(), String> {
    let mut mgr = state.lock().map_err(|e| e.to_string())?;
    let sftp_handle = mgr.sftps.get_mut(&id).ok_or("SFTP 연결 없음")?;
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
        .join(".config/biy/settings.json");
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
        .join(".config/biy/settings.json");
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
        .join(".config/biy/ssh_connections.json");
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

    let settings_path = base.join("biy-settings/.build/debug/biy-settings");

    if settings_path.exists() {
        let _ = std::process::Command::new(&settings_path).spawn();
    } else {
        // 빌드 안 되어 있으면 빌드 후 실행
        let settings_dir = base.join("biy-settings");
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
pub fn run() {
    tauri::Builder::default()
        .manage(Arc::new(Mutex::new(PtyManager {
            ptys: HashMap::new(),
            ssh_inputs: HashMap::new(),
            sftps: HashMap::new(),
        })))
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
            local_list_dir,
            get_pty_cwd,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
