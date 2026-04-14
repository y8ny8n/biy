import SwiftUI
import AppKit

@main
struct TermySettingsApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var appDelegate

    var body: some Scene {
        Window("termy 환경설정", id: "settings") {
            SettingsView()
                .frame(minWidth: 600, minHeight: 450)
        }
        .defaultSize(width: 680, height: 500)
        .commands {
            // ⌘+W로 창 닫기
            CommandGroup(replacing: .newItem) {}
        }
    }
}

// 앱 활성화 (최상단 + 정상 macOS 앱으로 등록)
class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationWillFinishLaunching(_ notification: Notification) {
        // CLI 실행이라도 정상 앱으로 등록 → ⌘+W, ⌘+Q 등 단축키 동작
        NSApp.setActivationPolicy(.regular)
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        // 최상단으로 가져오기
        NSApp.activate(ignoringOtherApps: true)
        // 첫 번째 윈도우 포커스
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) {
            NSApp.windows.first?.makeKeyAndOrderFront(nil)
            NSApp.windows.first?.orderFrontRegardless()
        }
    }
}

enum SettingsSection: String, CaseIterable, Identifiable {
    case appearance = "외관"
    case window = "윈도우"
    case ssh = "SSH 연결"
    case shell = "셸 / 고급"

    var id: String { rawValue }

    var icon: String {
        switch self {
        case .appearance: return "paintpalette.fill"
        case .window: return "macwindow"
        case .ssh: return "network"
        case .shell: return "terminal.fill"
        }
    }
}

struct SettingsView: View {
    @State private var config = TermyConfig.load()
    @State private var selectedSection: SettingsSection? = .appearance
    @State private var saved = false

    @State private var bgColor: Color = .black
    @State private var fgColor: Color = .white
    @State private var cursorColor: Color = .gray

    // SSH
    @State private var sshConnections: [SshConnection] = SshConnection.load()
    @State private var editingConnection: SshConnection? = nil
    @State private var showingNewConnection = false

    var body: some View {
        NavigationSplitView {
            List(SettingsSection.allCases, selection: $selectedSection) { section in
                NavigationLink(value: section) {
                    Label(section.rawValue, systemImage: section.icon)
                        .font(.system(size: 14))
                }
            }
            .listStyle(.sidebar)
            .navigationSplitViewColumnWidth(170)
        } detail: {
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    switch selectedSection {
                    case .appearance:
                        appearanceSection
                    case .window:
                        windowSection
                    case .ssh:
                        sshSection
                    case .shell:
                        shellSection
                    case nil:
                        appearanceSection
                    }
                }
                .padding(24)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .toolbar {
                ToolbarItem(placement: .primaryAction) {
                    Button(action: saveConfig) {
                        HStack(spacing: 4) {
                            Image(systemName: saved ? "checkmark.circle.fill" : "square.and.arrow.down")
                            Text(saved ? "저장됨" : "저장")
                        }
                    }
                    .buttonStyle(.borderedProminent)
                }
            }
        }
        .onAppear {
            loadColors()
        }
    }

    // MARK: - 외관

    private var appearanceSection: some View {
        VStack(alignment: .leading, spacing: 20) {
            sectionHeader("테마", subtitle: "미리 설정된 색상 조합을 선택하세요")

            LazyVGrid(columns: [
                GridItem(.flexible(), spacing: 10),
                GridItem(.flexible(), spacing: 10),
                GridItem(.flexible(), spacing: 10),
            ], spacing: 10) {
                ForEach(Array(THEMES.enumerated()), id: \.offset) { _, theme in
                    themeCard(theme)
                }
            }

            sectionHeader("글꼴", subtitle: "터미널에 표시되는 글꼴 설정")

            settingsCard {
                HStack {
                    Text("크기")
                        .frame(width: 80, alignment: .leading)
                        .foregroundStyle(.secondary)
                    Slider(value: $config.fontSize, in: 8...32, step: 0.5)
                    Text("\(config.fontSize, specifier: "%.1f") pt")
                        .monospacedDigit()
                        .frame(width: 60)
                }
            }

            sectionHeader("색상 커스텀", subtitle: "직접 색상을 지정할 수도 있습니다")

            settingsCard {
                VStack(spacing: 12) {
                    colorRow("배경색", color: $bgColor, hex: $config.bgColor)
                    Divider()
                    colorRow("글자색", color: $fgColor, hex: $config.fgColor)
                    Divider()
                    colorRow("커서색", color: $cursorColor, hex: $config.cursorColor)
                }
            }

            sectionHeader("미리보기", subtitle: "현재 설정이 적용된 모습")

            previewCard
        }
    }

    private func themeCard(_ theme: Theme) -> some View {
        let (bgR, bgG, bgB) = theme.bg.hexToColor()
        let (fgR, fgG, fgB) = theme.fg.hexToColor()
        let (crR, crG, crB) = theme.cursor.hexToColor()
        let themeBg = Color(red: bgR, green: bgG, blue: bgB)
        let themeFg = Color(red: fgR, green: fgG, blue: fgB)
        let themeCursor = Color(red: crR, green: crG, blue: crB)
        let isSelected = config.bgColor == theme.bg && config.fgColor == theme.fg

        return Button(action: {
            config.bgColor = theme.bg
            config.fgColor = theme.fg
            config.cursorColor = theme.cursor
            bgColor = themeBg
            fgColor = themeFg
            cursorColor = themeCursor
        }) {
            VStack(spacing: 0) {
                // 미니 프리뷰
                VStack(alignment: .leading, spacing: 2) {
                    Text("~ % ls")
                        .font(.system(size: 9, design: .monospaced))
                        .foregroundColor(themeFg)
                    HStack(spacing: 3) {
                        RoundedRectangle(cornerRadius: 1)
                            .fill(themeCursor)
                            .frame(width: 6, height: 10)
                        Text("app")
                            .font(.system(size: 8, design: .monospaced))
                            .foregroundColor(themeFg.opacity(0.7))
                    }
                }
                .padding(8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(themeBg)
                .clipShape(RoundedRectangle(cornerRadius: 6))

                // 테마 이름
                Text(theme.name)
                    .font(.system(size: 11))
                    .foregroundStyle(isSelected ? .primary : .secondary)
                    .padding(.top, 4)
            }
            .padding(6)
            .background(
                RoundedRectangle(cornerRadius: 8)
                    .stroke(isSelected ? Color.blue : Color.clear, lineWidth: 2)
            )
            .background(
                RoundedRectangle(cornerRadius: 8)
                    .fill(isSelected ? Color.blue.opacity(0.1) : Color.clear)
            )
        }
        .buttonStyle(.plain)
    }

    // MARK: - 윈도우

    private var windowSection: some View {
        VStack(alignment: .leading, spacing: 20) {
            sectionHeader("크기", subtitle: "터미널 창의 기본 크기")

            settingsCard {
                VStack(spacing: 12) {
                    HStack {
                        Text("너비")
                            .frame(width: 80, alignment: .leading)
                            .foregroundStyle(.secondary)
                        TextField("820", value: $config.windowWidth, format: .number)
                            .textFieldStyle(.roundedBorder)
                            .frame(width: 100)
                        Text("px")
                            .foregroundStyle(.secondary)
                    }
                    HStack {
                        Text("높이")
                            .frame(width: 80, alignment: .leading)
                            .foregroundStyle(.secondary)
                        TextField("560", value: $config.windowHeight, format: .number)
                            .textFieldStyle(.roundedBorder)
                            .frame(width: 100)
                        Text("px")
                            .foregroundStyle(.secondary)
                    }
                }
            }

            sectionHeader("투명도", subtitle: "창의 투명도 조절")

            settingsCard {
                HStack {
                    Text("투명도")
                        .frame(width: 80, alignment: .leading)
                        .foregroundStyle(.secondary)
                    Slider(value: $config.windowOpacity, in: 0.3...1.0)
                    Text("\(Int(config.windowOpacity * 100))%")
                        .monospacedDigit()
                        .frame(width: 45)
                }
            }
        }
    }

    // MARK: - 셸 / 고급

    private var shellSection: some View {
        VStack(alignment: .leading, spacing: 20) {
            sectionHeader("셸 프로그램", subtitle: "터미널에서 실행할 셸")

            settingsCard {
                VStack(alignment: .leading, spacing: 8) {
                    HStack {
                        Text("프로그램")
                            .frame(width: 80, alignment: .leading)
                            .foregroundStyle(.secondary)
                        TextField("비어있으면 $SHELL 사용", text: $config.shellProgram)
                            .textFieldStyle(.roundedBorder)
                    }
                    Text("기본값: 시스템 셸 ($SHELL 환경변수)")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                }
            }

            sectionHeader("스크롤백", subtitle: "터미널 히스토리 설정")

            settingsCard {
                VStack(alignment: .leading, spacing: 8) {
                    HStack {
                        Text("히스토리")
                            .frame(width: 80, alignment: .leading)
                            .foregroundStyle(.secondary)
                        TextField("10000", value: $config.scrollback, format: .number)
                            .textFieldStyle(.roundedBorder)
                            .frame(width: 100)
                        Text("줄")
                            .foregroundStyle(.secondary)
                    }
                    Text("터미널에서 위로 스크롤할 수 있는 최대 줄 수")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                }
            }

            sectionHeader("정보", subtitle: "")

            settingsCard {
                VStack(alignment: .leading, spacing: 4) {
                    Text("설정 파일")
                        .foregroundStyle(.secondary)
                    Text(TermyConfig.findConfigPath().path)
                        .font(.system(.caption, design: .monospaced))
                        .foregroundStyle(.tertiary)
                        .textSelection(.enabled)
                }
            }
        }
    }

    // MARK: - SSH 연결

    private var sshSection: some View {
        VStack(alignment: .leading, spacing: 20) {
            HStack {
                sectionHeader("SSH 연결", subtitle: "저장된 서버 연결을 관리합니다")
                Spacer()
                Button(action: {
                    showingNewConnection = true
                    editingConnection = SshConnection(
                        name: "", host: "", port: 22,
                        username: NSUserName(), authType: "agent", keyPath: ""
                    )
                }) {
                    Label("새 연결", systemImage: "plus")
                }
                .buttonStyle(.borderedProminent)
            }

            if sshConnections.isEmpty {
                settingsCard {
                    VStack(spacing: 8) {
                        Image(systemName: "network.slash")
                            .font(.largeTitle)
                            .foregroundStyle(.secondary)
                        Text("저장된 SSH 연결이 없습니다")
                            .foregroundStyle(.secondary)
                        Text("위의 '새 연결' 버튼으로 서버를 추가하세요")
                            .font(.caption)
                            .foregroundStyle(.tertiary)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(20)
                }
            } else {
                ForEach(sshConnections) { conn in
                    settingsCard {
                        HStack {
                            VStack(alignment: .leading, spacing: 4) {
                                Text(conn.name.isEmpty ? "\(conn.username)@\(conn.host)" : conn.name)
                                    .font(.headline)
                                HStack(spacing: 12) {
                                    Label(conn.host, systemImage: "server.rack")
                                    Label(":\(conn.port)", systemImage: "number")
                                    Label(conn.username, systemImage: "person")
                                    Label(conn.authType, systemImage: conn.authType == "key" ? "key" : "person.badge.key")
                                }
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            }
                            Spacer()
                            Button("편집") {
                                editingConnection = conn
                                showingNewConnection = true
                            }
                            .buttonStyle(.bordered)
                            Button(role: .destructive) {
                                sshConnections.removeAll { $0.id == conn.id }
                                SshConnection.save(sshConnections)
                            } label: {
                                Image(systemName: "trash")
                            }
                            .buttonStyle(.bordered)
                        }
                    }
                }
            }

            settingsCard {
                VStack(alignment: .leading, spacing: 4) {
                    Text("사용 방법")
                        .font(.headline)
                    Text("termy 터미널에서 ⌘+Shift+N 으로 SSH 연결을 선택할 수 있습니다")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }

            // 인라인 편집 폼
            if showingNewConnection, let conn = editingConnection {
                sshEditForm(conn)
            }
        }
    }

    private func sshEditForm(_ conn: SshConnection) -> some View {
        @State var editing = conn

        return settingsCard {
            VStack(alignment: .leading, spacing: 12) {
                HStack {
                    Text(conn.name.isEmpty ? "새 SSH 연결" : "연결 편집")
                        .font(.headline)
                    Spacer()
                    Button("취소") {
                        showingNewConnection = false
                    }
                }

                Divider()

                HStack {
                    Text("이름").frame(width: 70, alignment: .leading).foregroundStyle(.secondary)
                    TextField("내 서버", text: Binding(
                        get: { editingConnection?.name ?? "" },
                        set: { editingConnection?.name = $0 }
                    )).textFieldStyle(.roundedBorder)
                }
                HStack {
                    Text("호스트").frame(width: 70, alignment: .leading).foregroundStyle(.secondary)
                    TextField("example.com", text: Binding(
                        get: { editingConnection?.host ?? "" },
                        set: { editingConnection?.host = $0 }
                    )).textFieldStyle(.roundedBorder)
                }
                HStack {
                    Text("포트").frame(width: 70, alignment: .leading).foregroundStyle(.secondary)
                    TextField("22", value: Binding(
                        get: { editingConnection?.port ?? 22 },
                        set: { editingConnection?.port = $0 }
                    ), format: .number).textFieldStyle(.roundedBorder).frame(width: 80)
                }
                HStack {
                    Text("사용자").frame(width: 70, alignment: .leading).foregroundStyle(.secondary)
                    TextField("username", text: Binding(
                        get: { editingConnection?.username ?? "" },
                        set: { editingConnection?.username = $0 }
                    )).textFieldStyle(.roundedBorder)
                }

                Divider()

                Text("인증 방식").foregroundStyle(.secondary)
                Picker("", selection: Binding(
                    get: { editingConnection?.authType ?? "agent" },
                    set: { editingConnection?.authType = $0 }
                )) {
                    Text("SSH Agent").tag("agent")
                    Text("SSH 키").tag("key")
                    Text("비밀번호").tag("password")
                }
                .pickerStyle(.segmented)
                .labelsHidden()

                if editingConnection?.authType == "key" {
                    HStack {
                        Text("키 경로").frame(width: 70, alignment: .leading).foregroundStyle(.secondary)
                        TextField("~/.ssh/id_rsa", text: Binding(
                            get: { editingConnection?.keyPath ?? "" },
                            set: { editingConnection?.keyPath = $0 }
                        )).textFieldStyle(.roundedBorder)
                    }
                }

                Divider()

                HStack {
                    Spacer()
                    Button("저장") {
                        if var updated = editingConnection {
                            if let idx = sshConnections.firstIndex(where: { $0.id == updated.id }) {
                                sshConnections[idx] = updated
                            } else {
                                sshConnections.append(updated)
                            }
                            SshConnection.save(sshConnections)
                            showingNewConnection = false
                        }
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(editingConnection?.host.isEmpty ?? true || editingConnection?.username.isEmpty ?? true)
                }
            }
        }
    }

    // MARK: - 공통 컴포넌트

    private func sectionHeader(_ title: String, subtitle: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title)
                .font(.headline)
            if !subtitle.isEmpty {
                Text(subtitle)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private func settingsCard<Content: View>(@ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading) {
            content()
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 10))
    }

    private func colorRow(_ label: String, color: Binding<Color>, hex: Binding<String>) -> some View {
        HStack {
            Text(label)
                .frame(width: 80, alignment: .leading)
                .foregroundStyle(.secondary)
            ColorPicker("", selection: color, supportsOpacity: false)
                .labelsHidden()
                .onChange(of: color.wrappedValue) { _, newVal in
                    if let components = newVal.cgColor?.components, components.count >= 3 {
                        hex.wrappedValue = colorToHex(
                            Double(components[0]),
                            Double(components[1]),
                            Double(components[2])
                        )
                    }
                }
            Text(hex.wrappedValue)
                .font(.system(.body, design: .monospaced))
                .foregroundStyle(.secondary)
        }
    }

    private var previewCard: some View {
        let (bgR, bgG, bgB) = config.bgColor.hexToColor()
        let (fgR, fgG, fgB) = config.fgColor.hexToColor()
        let bg = Color(red: bgR, green: bgG, blue: bgB)
        let fg = Color(red: fgR, green: fgG, blue: fgB)

        return VStack(alignment: .leading, spacing: 4) {
            Text("kyoungyoon@mac ~ % ls -la")
            Text("drwxr-xr-x  5 user  staff  160 Mar 31 09:00 .")
            Text("total 42")
        }
        .font(.system(size: config.fontSize, design: .monospaced))
        .foregroundColor(fg)
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(bg, in: RoundedRectangle(cornerRadius: 10))
        .overlay(
            RoundedRectangle(cornerRadius: 10)
                .stroke(Color.gray.opacity(0.3), lineWidth: 1)
        )
    }

    // MARK: - 로직

    private func loadColors() {
        let (r1, g1, b1) = config.bgColor.hexToColor()
        bgColor = Color(red: r1, green: g1, blue: b1)
        let (r2, g2, b2) = config.fgColor.hexToColor()
        fgColor = Color(red: r2, green: g2, blue: b2)
        let (r3, g3, b3) = config.cursorColor.hexToColor()
        cursorColor = Color(red: r3, green: g3, blue: b3)
    }

    private func saveConfig() {
        config.save()
        saved = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
            saved = false
        }
    }
}
