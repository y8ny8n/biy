import SwiftUI

struct SshEditView: View {
    @State var connection: SshConnection
    let onSave: (SshConnection) -> Void
    let onCancel: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            // 헤더
            HStack {
                Text(connection.name.isEmpty ? "새 SSH 연결" : "연결 편집")
                    .font(.headline)
                Spacer()
            }
            .padding()

            Divider()

            // 폼
            Form {
                Section("기본 정보") {
                    HStack {
                        Text("이름")
                            .frame(width: 80, alignment: .leading)
                        TextField("내 서버", text: $connection.name)
                            .textFieldStyle(.roundedBorder)
                    }
                    HStack {
                        Text("호스트")
                            .frame(width: 80, alignment: .leading)
                        TextField("example.com", text: $connection.host)
                            .textFieldStyle(.roundedBorder)
                    }
                    HStack {
                        Text("포트")
                            .frame(width: 80, alignment: .leading)
                        TextField("22", value: $connection.port, format: .number)
                            .textFieldStyle(.roundedBorder)
                            .frame(width: 80)
                    }
                    HStack {
                        Text("사용자")
                            .frame(width: 80, alignment: .leading)
                        TextField("username", text: $connection.username)
                            .textFieldStyle(.roundedBorder)
                    }
                }

                Section("인증") {
                    Picker("인증 방식", selection: $connection.authType) {
                        Text("SSH Agent (추천)").tag("agent")
                        Text("SSH 키 파일").tag("key")
                        Text("비밀번호").tag("password")
                    }
                    .pickerStyle(.segmented)

                    if connection.authType == "key" {
                        HStack {
                            Text("키 경로")
                                .frame(width: 80, alignment: .leading)
                            TextField("~/.ssh/id_rsa", text: $connection.keyPath)
                                .textFieldStyle(.roundedBorder)
                        }
                        Text("기본값: ~/.ssh/id_rsa")
                            .font(.caption)
                            .foregroundStyle(.tertiary)
                    }

                    if connection.authType == "agent" {
                        Text("macOS의 ssh-agent에 등록된 키를 자동으로 사용합니다")
                            .font(.caption)
                            .foregroundStyle(.tertiary)
                    }
                }
            }
            .formStyle(.grouped)

            Divider()

            // 버튼
            HStack {
                Button("취소") {
                    onCancel()
                }
                .keyboardShortcut(.cancelAction)

                Spacer()

                Button("저장") {
                    onSave(connection)
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
                .disabled(connection.host.isEmpty || connection.username.isEmpty)
            }
            .padding()
        }
        .frame(width: 450, height: 420)
    }
}
