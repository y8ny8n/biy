## 2026-04-13 10:36 ~ 10:38 (약 2분)

### 요청
- SSH 연결 끊김 / SFTP 연결 지연 시 앱이 멈추는(hang) 현상 수정

### 처리 결과
- 변경 파일: `src-tauri/src/lib.rs`
- TCP 연결에 `connect_timeout(10초)` 적용 (SSH, SFTP 세션 모두)
- SSH 세션에 `set_keepalive(true, 30)` + `set_timeout(30_000)` 추가
- SFTP 세션에도 keepalive/timeout 동일 적용
- SSH 터미널 루프에 수동 `keepalive_send()` (30초 간격) 추가, keepalive 실패 시 루프 종료
- SSH 터미널 루프에 연속 타임아웃 3회 감지 → 연결 종료 로직 추가
- 모든 SFTP 블로킹 커맨드에 `set_timeout` 적용 (일반 30초, 파일 전송 60초)
- `cargo check` 빌드 성공 확인

---

## 2026-04-13 10:38 ~ 10:41 (약 3분)

### 요청
- SSH/SFTP 연결 실패 시 사용자에게 에러 원인을 알려주는 처리 추가

### 처리 결과
- 변경 파일: `src-tauri/src/lib.rs`, `src/main.js`
- `pty-error` 이벤트 추가 (백엔드 → 프론트엔드 에러 전달)
- SSH 연결 실패 시: 에러 원인별 구체적 메시지 (DNS 실패, 연결 불가, 핸드셰이크 실패, 인증 실패)
- SSH 세션 중 끊김 시: 타임아웃/keepalive 실패/read 에러 원인을 터미널에 빨간색으로 출력
- SFTP 세션 실패 시: "파일 트리 연결 실패" 메시지를 터미널에 출력
- 프론트엔드에 `pty-error` 리스너 추가 → 터미널에 `[termy] 에러내용` 형태로 표시
- `cargo check` 빌드 성공 확인

---

## 2026-04-13 10:42 ~ 10:43 (약 1분)

### 요청
- 빌드 후 기존 앱 교체

### 처리 결과
- `cargo tauri build` release 빌드 성공
- `/Applications/termy.app` 교체 완료 후 실행

---

## 2026-04-14 11:00 ~ 11:03 (약 3분)

### 요청
- .pem 키 SSH 연결이 안 되는 증상 확인 요청

### 처리 결과
- 근본 원인: `SshConfig.key_path`를 백엔드가 완전히 무시, `~/.ssh/id_*`만 하드코딩 시도
- `src-tauri/src/lib.rs` 수정
  - 헬퍼 추가: `expand_path()` (~, $HOME 확장), `try_key_auth()` (None → 키체인 passphrase → 프롬프트 순)
  - 키 passphrase 키체인 저장/조회 (`termy-sshkey` 서비스) 및 osascript 프롬프트 추가
  - `ssh_session()`, `create_sftp_session()` 두 지점에 커스텀 키 경로 우선 시도 로직 추가
- `cargo check` 통과 (기존 경고 2개 외 신규 경고 없음)

---
