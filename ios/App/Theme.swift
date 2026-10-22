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

    // Dark surfaces follow the original dark mock: a blue charcoal rather
    // than the terminal's near-black, so sheets and cards read as layers.
    var canvas: Color { light ? tone(.surface, 0) : hex(0x0F1419) }
    var card: Color { light ? tone(.surface, 2) : hex(0x181C26) }
    var sheet: Color { light ? tone(.surface, 0) : hex(0x121721) }
    var idle: Color { light ? tone(.surface, 4) : hex(0x262B34) }
    var rule: Color { light ? tone(.surface, 6) : hex(0x2C313A) }
    /// Code blocks sit a step darker than the canvas, inside a rule.
    var codeBg: Color { light ? tone(.surface, 2) : hex(0x0A0E14) }
    var text: Color { tone(.text, 21) }
    var bright: Color { tone(.text, 23) }
    var muted: Color { tone(.muted, 15) }
    var faint: Color { tone(.muted, 8) }
    var accent: Color { tone(.accent, 22) }
    var badge: Color { tone(.accent, 18) }
    var link: Color { tone(.accent, 20) }
    var code: Color { tone(.code, 19) }
    var codeLabel: Color { tone(.code, 10) }
    /// The selected tab: the mock's light pink on dark, deep plum on light.
    var chipOn: Color { light ? tone(.accent, 22) : hex(0xEFBCF8) }
    /// An unselected tab.
    var chipOff: Color { light ? tone(.surface, 3) : hex(0x22262E) }
    /// Text on a filled accent chip.
    var onAccent: Color { light ? tone(.surface, 1) : hex(0x1A1320) }
    /// The "online" dot: the code hue (a green) at the muted step.
    var live: Color { tone(.code, 14) }
    /// Routing's two ends that post, as in its mock: a nudge to the driver
    /// blue, a ping to you orange.
    var routeNudge: Color { light ? hex(0x2F6BC9) : hex(0x4A90F0) }
    var routePing: Color { light ? hex(0xC8621A) : hex(0xFF8A3D) }

    /// The Control view's colors: the terminal Control tab's OKLab hues
    /// (src/ui/control.rs), on the same ramp. ok green, warn amber, page
    /// red, and cyan for data that carries no status.
    func hue(_ h: Float, _ level: UInt8 = 19) -> Color {
        cached("hue-\(h)-\(level)-\(light)") { hueInk(hue: h, level: level, light: light) }
    }
    var ok: Color { hue(145) }
    var warn: Color { hue(75) }
    var page: Color { hue(25) }
    var data: Color { hue(215) }

    private func hex(_ v: UInt32) -> Color {
        Color(.sRGB, red: Double(v >> 16 & 0xFF) / 255, green: Double(v >> 8 & 0xFF) / 255,
              blue: Double(v & 0xFF) / 255, opacity: 1)
    }
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

/// Fira Mono for everything you read, Fira Sans for names, tabs and
/// buttons, as in the dark mock. Sizes follow Dynamic Type.
extension Font {
    private static func fira(_ face: String, _ size: CGFloat, _ style: Font.TextStyle) -> Font {
        .custom(face, size: size, relativeTo: style)
    }

    /// Message text and the compose box.
    static let text = fira("FiraMono-Regular", 15, .subheadline)
    static let textBold = fira("FiraMono-Bold", 15, .subheadline)
    /// Times, reply counts, hints, status lines.
    static let small = fira("FiraMono-Regular", 13, .footnote)
    static let stamp = small
    /// Code blocks and tool calls.
    static let mono = fira("FiraMono-Regular", 14, .footnote)
    static let tiny = fira("FiraMono-Regular", 11, .caption2)
    static let name = fira("FiraSans-SemiBold", 18, .body)
    static let chip = fira("FiraSans-Regular", 15, .subheadline)
    static let chipOn = fira("FiraSans-Medium", 15, .subheadline)
    static let label = fira("FiraSans-Regular", 17, .body)
    static let button = fira("FiraSans-SemiBold", 17, .body)
    static let header = fira("FiraMono-Regular", 17, .body)
    static let heading = fira("FiraSans-Bold", 21, .title3)
}
