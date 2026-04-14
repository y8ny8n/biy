import Foundation

struct TermyConfig {
    var fontSize: Double = 16.0
    var windowWidth: Int = 820
    var windowHeight: Int = 560
    var windowOpacity: Double = 1.0
    var bgColor: String = "#0D1117"
    var fgColor: String = "#D9D9D9"
    var cursorColor: String = "#5A5A6A"
    var shellProgram: String = ""
    var scrollback: Int = 10000

    static var configPath: URL {
        let appSupport = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        return appSupport.appendingPathComponent("termy/config.toml")
    }

    // ~/Library/Application Support 와 ~/.config 둘 다 체크
    static var configPathAlt: URL {
        let home = FileManager.default.homeDirectoryForCurrentUser
        return home.appendingPathComponent(".config/termy/config.toml")
    }

    static func findConfigPath() -> URL {
        // ~/.config/termy/config.toml 우선
        let alt = configPathAlt
        if FileManager.default.fileExists(atPath: alt.path) {
            return alt
        }
        let primary = configPath
        if FileManager.default.fileExists(atPath: primary.path) {
            return primary
        }
        // 기본: ~/.config/termy/config.toml 생성
        return alt
    }

    static func load() -> TermyConfig {
        let path = findConfigPath()
        var config = TermyConfig()

        guard let content = try? String(contentsOf: path, encoding: .utf8) else {
            return config
        }

        var currentSection = ""
        for line in content.components(separatedBy: "\n") {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.isEmpty || trimmed.hasPrefix("#") { continue }

            // [section]
            if trimmed.hasPrefix("[") && trimmed.hasSuffix("]") {
                currentSection = String(trimmed.dropFirst().dropLast())
                continue
            }

            // key = value
            guard let eqIdx = trimmed.firstIndex(of: "=") else { continue }
            let key = trimmed[trimmed.startIndex..<eqIdx].trimmingCharacters(in: .whitespaces)
            var value = trimmed[trimmed.index(after: eqIdx)...].trimmingCharacters(in: .whitespaces)

            // 문자열의 따옴표 제거
            if value.hasPrefix("\"") && value.hasSuffix("\"") {
                value = String(value.dropFirst().dropLast())
            }

            switch (currentSection, key) {
            case ("font", "size"): config.fontSize = Double(value) ?? 16.0
            case ("window", "width"): config.windowWidth = Int(value) ?? 820
            case ("window", "height"): config.windowHeight = Int(value) ?? 560
            case ("window", "opacity"): config.windowOpacity = Double(value) ?? 1.0
            case ("colors", "background"): config.bgColor = value
            case ("colors", "foreground"): config.fgColor = value
            case ("colors", "cursor"): config.cursorColor = value
            case ("shell", "program"): config.shellProgram = value
            case ("", "scrollback"): config.scrollback = Int(value) ?? 10000
            default: break
            }
        }

        return config
    }

    func save() {
        let path = TermyConfig.findConfigPath()
        let dir = path.deletingLastPathComponent()
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)

        let content = """
        [font]
        size = \(fontSize)

        [window]
        width = \(windowWidth)
        height = \(windowHeight)
        opacity = \(windowOpacity)

        [colors]
        background = "\(bgColor)"
        foreground = "\(fgColor)"
        cursor = "\(cursorColor)"

        [shell]
        program = "\(shellProgram)"
        args = []

        scrollback = \(scrollback)
        """

        try? content.write(to: path, atomically: true, encoding: .utf8)
    }
}

// ── SSH 연결 목록 ──

struct SshConnection: Codable, Identifiable {
    var id = UUID()
    var name: String
    var host: String
    var port: Int
    var username: String
    var authType: String  // "password", "key", "agent"
    var keyPath: String

    static func load() -> [SshConnection] {
        let path = sshConfigPath()
        guard let data = try? Data(contentsOf: path),
              let connections = try? JSONDecoder().decode([SshConnection].self, from: data) else {
            return []
        }
        return connections
    }

    static func save(_ connections: [SshConnection]) {
        let path = sshConfigPath()
        let dir = path.deletingLastPathComponent()
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        if let data = try? JSONEncoder().encode(connections) {
            try? data.write(to: path)
        }
    }
}

func sshConfigPath() -> URL {
    let home = FileManager.default.homeDirectoryForCurrentUser
    return home.appendingPathComponent(".config/termy/ssh_connections.json")
}

// ── 테마 프리셋 ──

struct Theme {
    let name: String
    let bg: String
    let fg: String
    let cursor: String
}

let THEMES: [Theme] = [
    Theme(name: "termy Dark",   bg: "#0D1117", fg: "#D9D9D9", cursor: "#5A5A6A"),
    Theme(name: "Dracula",      bg: "#282A36", fg: "#F8F8F2", cursor: "#BD93F9"),
    Theme(name: "Nord",         bg: "#2E3440", fg: "#D8DEE9", cursor: "#88C0D0"),
    Theme(name: "Solarized Dark", bg: "#002B36", fg: "#839496", cursor: "#268BD2"),
    Theme(name: "Solarized Light", bg: "#FDF6E3", fg: "#657B83", cursor: "#268BD2"),
    Theme(name: "Monokai",      bg: "#272822", fg: "#F8F8F2", cursor: "#F92672"),
    Theme(name: "One Dark",     bg: "#282C34", fg: "#ABB2BF", cursor: "#528BFF"),
    Theme(name: "Tokyo Night",  bg: "#1A1B26", fg: "#C0CAF5", cursor: "#7AA2F7"),
    Theme(name: "Gruvbox Dark", bg: "#282828", fg: "#EBDBB2", cursor: "#FE8019"),
    Theme(name: "Catppuccin Mocha", bg: "#1E1E2E", fg: "#CDD6F4", cursor: "#B4BEFE"),
    Theme(name: "GitHub Dark",  bg: "#0D1117", fg: "#C9D1D9", cursor: "#58A6FF"),
    Theme(name: "Rosé Pine",    bg: "#191724", fg: "#E0DEF4", cursor: "#C4A7E7"),
]

// Hex 색상 변환
extension String {
    func hexToColor() -> (Double, Double, Double) {
        let hex = self.trimmingCharacters(in: CharacterSet(charactersIn: "#"))
        guard hex.count == 6 else { return (0, 0, 0) }
        let scanner = Scanner(string: hex)
        var rgb: UInt64 = 0
        scanner.scanHexInt64(&rgb)
        let r = Double((rgb >> 16) & 0xFF) / 255.0
        let g = Double((rgb >> 8) & 0xFF) / 255.0
        let b = Double(rgb & 0xFF) / 255.0
        return (r, g, b)
    }
}

func colorToHex(_ r: Double, _ g: Double, _ b: Double) -> String {
    return String(format: "#%02X%02X%02X", Int(r * 255), Int(g * 255), Int(b * 255))
}
