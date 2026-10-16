import SwiftUI

/// The terminal client's ramp, read from the Rust core so every step is the
/// same RGB. One 24-step brightness ramp; every hue sits at the luminance of
/// the gray it replaces, so the iPhone's grayscale filter shows calm grays.
struct Theme {
    let light: Bool

    private static var cache: [String: Color] = [:]

    private func cached(_ key: String, _ make: () -> Rgb) -> Color {
        if let c = Theme.cache[key] { return c }
        let rgb = make()
        let c = Color(.sRGB, red: Double(rgb.r) / 255, green: Double(rgb.g) / 255,
                      blue: Double(rgb.b) / 255, opacity: 1)
        Theme.cache[key] = c
        return c
    }

    func tone(_ t: Tone, _ level: UInt8) -> Color {
        cached("\(t)-\(level)-\(light)") { BuzzVim.tone(role: t, level: level, light: light) }
    }

    /// A stable color for a person or channel.
    func ink(_ key: String, _ level: UInt8 = 18) -> Color {
        cached("ink-\(key)-\(level)-\(light)") { BuzzVim.ink(key: key, level: level, light: light) }
    }

    var canvas: Color { tone(.surface, 0) }
    var card: Color { tone(.surface, 2) }
    var idle: Color { tone(.surface, 4) }
    var rule: Color { tone(.surface, 6) }
    var text: Color { tone(.text, 21) }
    var bright: Color { tone(.text, 23) }
    var muted: Color { tone(.muted, 13) }
    var faint: Color { tone(.muted, 8) }
    var accent: Color { tone(.accent, 22) }
    var badge: Color { tone(.accent, 18) }
    var link: Color { tone(.accent, 20) }
    var code: Color { tone(.code, 19) }
    var codeLabel: Color { tone(.code, 10) }
    /// Text on a filled accent chip.
    var onAccent: Color { tone(.surface, 1) }
    /// The "online" dot: the code hue (a green) at the muted step.
    var live: Color { tone(.code, 14) }
}

private struct ThemeKey: EnvironmentKey {
    static let defaultValue = Theme(light: false)
}

extension EnvironmentValues {
    var theme: Theme {
        get { self[ThemeKey.self] }
        set { self[ThemeKey.self] = newValue }
    }
}

/// Puts the right ramp in the environment for the system appearance.
struct Themed: ViewModifier {
    @Environment(\.colorScheme) private var scheme

    func body(content: Content) -> some View {
        let t = Theme(light: scheme == .light)
        content
            .environment(\.theme, t)
            .tint(t.link)
    }
}

extension Font {
    static let name = Font.subheadline.weight(.semibold)
    static let stamp = Font.caption
    static let mono = Font.system(.footnote, design: .monospaced)
}
