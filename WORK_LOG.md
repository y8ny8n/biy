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

## 2026-06-27 23:12 ~ 23:19 (약 7분)

### 요청
- gstack `/health`로 프로젝트 전반 진단 후, 도출된 개선/버그 항목 전부 처리

### 처리 결과
- `/health` 진단: 컴포지트 5.6/10 (테스트 0개가 유일한 감점 요인, Rust 백엔드는 clippy 경고 4개뿐으로 양호)
- 변경 파일: `src-tauri/src/lib.rs`
- lib.rs:300 `authenticated = true` 죽은 대입 조사 → 버그 아님 확인 (`create_sftp_session`은 305번 `session.authenticated()`로 정상 판정). 군더더기 대입 제거
- clippy 3건 정리: 미사용 `Manager` import 제거, `io::Error::new(...Other...)` → `io::Error::other()` 2곳 현대화
- `expand_path`를 순수 헬퍼 `expand_path_with_home(p, home)`로 분리 (env 의존 제거, 호출부 변경 없음)
- 단위 테스트 8개 신규 추가 (`~`/`~/`/`$HOME`/절대·상대경로/공백trim/`~user` 미확장 케이스)
- 검증: `cargo clippy --all-targets` 경고 0건, `cargo test` 8 passed
- 개선 후 추정 점수: 테스트 0→8, lint 8→10

---

## 2026-06-27 23:20 ~ 23:29 (약 9분)

### 요청
- SSH 터미널에서 cd 하면 SFTP 파일트리가 자동으로 따라가도록 연동

### 처리 결과
- 방식: OSC 7 escape sequence 기반 자동 cwd 추적 (echo 주입 폴링 방식 대체, 화면 오염 없음)
- 변경 파일: `src/main.js`, `src-tauri/src/lib.rs`
- 프론트(접속 주입): SSH 접속 시 TMOUT=0와 함께 bash(`PROMPT_COMMAND`)/zsh(`chpwd` hook)에 OSC 7 출력 hook 주입. cd 시마다 `\033]7;file://$PWD\007` 자동 출력 (BEL 종료라 기존 OSC 파서 재사용)
- 백엔드(파싱): ssh_session OSC 상태머신에 `7;` 분기 추가 → `file://`에서 경로 추출 → `pty-cwd` 이벤트 emit
- 프론트(리스너): `pty-cwd` 리스너 추가. 활성 SSH 탭이고 트리 경로와 다를 때만 `fileTreeView.load` (엔터 연타 시 중복 SFTP 요청 방지 dedup)
- 검증: `cargo clippy` 경고 0, `cargo test` 8 passed, `esbuild` 번들 성공, 주입 문자열 바이트 검증(ESC/BEL 보존) 통과
- 비고: 접속 직후 주입 스니펫이 한 줄 echo로 잠깐 보임(기존 TMOUT 주입과 동일 수준의 cosmetic). 실서버 cd 추적은 수동 확인 필요

---

## 2026-06-27 23:30 ~ 23:54 (약 24분)

### 요청
- SSH/SFTP 기능 개선 4건: 인증 흐름, 접속 명령 줄 숨기기, 서버 신원 검증(보안), 자동 재연결

### 처리 결과
- 변경 파일: `src-tauri/src/lib.rs`, `src/main.js`

- (1) 비밀번호 모드 첫 접속 개선: 정밀 분석 결과 "두 번 묻기"가 아니라 "비번 모드 첫 접속 시 파일트리 미표시"가 실제 문제였음(SFTP가 비번 저장 전 먼저 시도→실패). create_ssh_pty 진입 시 비번을 한 번 받아 키체인 저장 → SFTP·SSH 둘 다 재사용. 프롬프트 1회 + 첫 접속에 트리 정상

- (2) 접속 설정 줄 숨기기: 원격 echo를 stty -echo/echo로 감싸 긴 OSC7 설정 줄을 화면에서 숨김. 배너 보존(짧은 'stty -echo' 한 줄만 남음). injectSshInit() 함수로 추출

- (3) 서버 신원 검증(TOFU): verify_host_key() 헬퍼 추가. ssh2 known_hosts API로 ~/.config/termy/known_hosts에 지문 저장. 처음 서버는 조용히 저장(accept-new), 지문 변경 시 MITM 경고+차단. create_sftp_session/ssh_session 양쪽 핸드셰이크 후 호출

- (4) 자동 재연결: SshOutcome enum(Closed/Disconnected) 추가, ssh_session이 종료 사유 반환. create_ssh_pty 스레드를 재시도 루프로 변경(최대 3회, 2/4/6초 백오프, 30초 이상 유지 시 카운터 리셋). 재연결 시 SFTP 재생성(setup_sftp 헬퍼로 추출) + pty-reconnected 이벤트 → 프론트가 OSC7 재주입 + 트리 재로드. 한계: 새 셸이라 이전 명령/화면은 복구 안 됨(tmux/mosh 영역)

- 검증: cargo clippy 경고 0, cargo test 8 passed, esbuild 번들 성공, cargo tauri build --debug 성공 후 실행. 실서버 동작(재연결·지문검증)은 수동 확인 필요

---

## 2026-06-28 00:00 ~ 00:17 (약 17분)

### 요청
- 그동안 작업을 커밋 단위로 정리
- SFTP 파일 업로드 중 SSH 터미널이 멈추는 현상 분석·개선·테스트

### 처리 결과
- 커밋 정리(2건): decf1e2 chore(health), 0fa02f1 feat(ssh/sftp). 깨끗한 트리에서 health만 재적용→커밋1, 최종본 복원→커밋2. 각 커밋 독립적으로 cargo clippy 0 + test 8 통과 확인. push 안 함

- SFTP 업로드 중 터미널 멈춤: 근본 원인 = 전역 Mutex(PtyManager) 경합. sftp_upload/download가 파일 전송 내내 PtyManager 락을 점유했고, pty_write(키 입력)도 같은 락을 써서 업로드 동안 키 입력이 블록됨
- 해결: SftpHandle 맵을 PtyManager에서 떼어내 별도 뮤텍스 `SftpState`로 분리
  - PtyManager는 ptys/ssh_inputs만 보유, sftps는 `Arc<Mutex<HashMap<u32, SftpHandle>>>`로 분리
  - 7개 sftp_* 커맨드 + setup_sftp + create_pty/create_ssh_pty + run() manage 수정 (총 13곳)
  - create_pty는 인자 8개가 되어 clippy too_many_arguments 발생 → Tauri 주입 인자라 #[allow] 처리
- 효과: pty_write→PtyState, sftp_upload→SftpState로 뮤텍스 분리 → 업로드가 키 입력을 막지 못함 (정적 grep으로 공유 없음 확인)
- 한계: SFTP 작업끼리는 SftpState로 직렬화(동일 세션 동시사용 방지, 안전). 업로드 중 파일트리 갱신은 대기하지만 터미널은 반응함
- 검증: cargo clippy 0, cargo test 8 passed, tauri build 성공, 앱 정상 부팅(두 State 등록 OK). 실제 업로드 멈춤 해소는 실서버 필요 — 코드/빌드/부팅 레벨까지 확인

---

## 2026-06-28 00:35 ~ 00:53 (약 18분)

### 요청
- UI 다듬기 4건: stty -echo 잔상 숨김, 드롭존 드래그 시에만, 주소창 cd버튼 색위계, 폴더 색 구분
- (+드래그로 SFTP 다운로드 요청 → 별도 검토)

### 처리 결과
- 변경 파일: `src/main.js`, `src/styles.css`
- (1) stty -echo 잔상 숨김: injectSshInit을 단일 라인 주입으로 재설계 + 끝에 보이지 않는 sentinel(OSC 1337) 출력. pty-output 리스너가 sentinel 볼 때까지 출력을 버퍼링→숨김, 그 뒤만 표시(앞에 \r\n 하나). 2.5초 안전장치. 결과는 "엔터 한 번" 친 듯 깔끔
- (2) 드롭존: position absolute 오버레이로 변경, opacity 0 기본 → .active일 때만 표시. 평소 공간 차지 안 함(파일 리스트 넓어짐)
- (3) cd 버튼: background accent(파랑 채움) → 외곽선 보조색, hover 시에만 accent. 사이드바 색 위계 정리
- (4) 폴더 색: :root에 --folder(#7fb0e6) 추가, 렌더 시 폴더 행에 is-dir 클래스. 파일명=text-secondary(디밍), 폴더명=--folder(블루)로 대비
- 검증: esbuild 번들 성공, tauri build 성공, 앱 재실행 후 스크린샷으로 (2)(3)(4) 육안 확인. (1)은 다음 SSH 접속 시 확인 필요
- 드래그 다운로드(앱→Finder): Tauri WKWebView가 드래그아웃(DownloadURL/파일프로미스) 미지원 → 네이티브 작업 필요, 별도 스코프로 보류

---

## 2026-06-28 01:00 ~ 01:11 (약 11분)

### 요청
- SSH에서 su 등으로 계정 전환 시 SFTP 파일트리도 그 계정 경로를 따라가게

### 처리 결과
- 배경: SFTP 세션은 항상 접속 유저로 인증됨(su 안 됨). 단 접속 유저가 대상 계정 파일을 읽을 수 있으면(권한 OK) SFTP가 그 경로를 볼 수 있음. 따라서 필요한 건 "su 후 트리가 그 경로로 자동 이동"
- 변경 파일: `src/main.js`
- trackShellSwitch() 추가: pty-output(에코된 명령)에서 `프롬프트 + su/sudo -i/sudo su/bash/zsh...` 감지 → OSC7 훅 재주입 대기
  - 같은 호스트 fs 셸만 대상(ssh/docker exec 제외)
  - 비밀번호 프롬프트(Password/passphrase/암호) 감지 시 대기 → 설정 명령이 비번으로 들어가는 사고 방지
  - 출력이 잠잠해지면(프롬프트 표시 완료) 900ms 디바운스 후 injectSshInit 재호출(suppression으로 잔상도 숨김)
  - 20초 안전 타임아웃
- 정규식 sanity 8케이스 통과(su/sudo-su/sudo -i/bash 감지, ls/systemctl/ssh/echo sudo 미감지)
- 검증: esbuild 번들 + tauri build 성공, 앱 정상 실행. 실제 su 추적은 실서버 필요 — 코드/빌드 레벨 확인

---

## 2026-06-28 01:20 ~ (원격 파일 Sublime 편집)

### 요청
- vi 대신 원격 텍스트 파일을 Sublime Text로 열어 수정→저장 시 서버 자동 반영

### 처리 결과
- 변경 파일: `src-tauri/src/lib.rs`, `src/main.js`, `src/index.html`
- 백엔드 sftp_edit 커맨드: 원격 파일을 <tmp>/termy-edit/<tabId>/<파일명>으로 다운로드 → `open -a "Sublime Text"`로 열기 → 별도 스레드가 임시파일 mtime을 1초 폴링, 변경 시 서버 업로드(edit-synced/edit-error 이벤트). SFTP 세션 사라지면 감시 종료
- 헬퍼 sftp_read_to_local / sftp_write_from_local 추가(SftpState 락 짧게 점유)
- 프론트: 우클릭 메뉴에 "Sublime으로 편집"(원격 파일만 노출) + 핸들러 + edit-synced/error 토스트
- 검증: cargo clippy 0, cargo test 8, esbuild 번들 성공, 정식 빌드 후 /Applications 갱신·실행. Sublime Text 설치 확인. 실제 편집-저장-반영은 실서버 필요
- /Applications 단일 설치 유지, target/bundle 정리

---
