import UIKit

/// The relay refuses any image that still carries metadata: no EXIF, XMP,
/// ICC profile, comment or pHYs, only canonical JFIF/Adobe headers. So an
/// image is redrawn in sRGB (orientation baked in), re-encoded, and every
/// segment or chunk the relay doesn't allow is dropped. Ported from Buzz's
/// own iOS app (mobile/ios/Runner/MediaSanitizer.swift).
enum Sanitize {
    /// PNG stays PNG (screenshots keep their sharp edges); anything else
    /// becomes JPEG. GIF and WebP lose animation.
    static func image(_ img: UIImage, png: Bool) -> Data? {
        guard img.size.width > 0, img.size.height > 0 else { return nil }
        let format = UIGraphicsImageRendererFormat()
        format.scale = img.scale
        format.opaque = false
        format.preferredRange = .standard
        let flat = UIGraphicsImageRenderer(size: img.size, format: format).image { _ in
            img.draw(in: CGRect(origin: .zero, size: img.size))
        }
        if png {
            return flat.pngData().flatMap(scrubPng)
        }
        return flat.jpegData(compressionQuality: 0.85).flatMap(scrubJpeg)
    }

    private static let pngSig = Data([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])
    private static let pngKeep: Set<String> = [
        "cHRM", "gAMA", "sBIT", "sRGB", "bKGD", "hIST", "tRNS", "sPLT", "acTL", "fcTL", "fdAT",
    ]

    /// Critical chunks and the relay's rendering chunks; nothing after IEND.
    static func scrubPng(_ input: Data) -> Data? {
        let d = Data(input)
        guard d.starts(with: pngSig) else { return nil }
        var out = pngSig
        var i = pngSig.count
        while d.count - i >= 12 {
            let len = Int(be32(d, i))
            guard len <= d.count - i - 12 else { return nil }
            let kind = d[(i + 4)..<(i + 8)]
            guard let name = String(bytes: kind, encoding: .ascii) else { return nil }
            let end = i + 12 + len
            if kind[kind.startIndex] & 0x20 == 0 || pngKeep.contains(name) {
                out.append(d[i..<end])
            }
            i = end
            if name == "IEND" { return out }
        }
        return nil
    }

    /// Drops APP1-APP13, APP15 and comments, and any APP0/APP14 that isn't
    /// a plain JFIF/Adobe header. Stops at EOI.
    static func scrubJpeg(_ input: Data) -> Data? {
        let d = Data(input)
        guard d.count >= 2, d[0] == 0xFF, d[1] == 0xD8 else { return nil }
        var out = Data([0xFF, 0xD8])
        var i = 2
        var inScan = false
        while i < d.count {
            if inScan, d[i] != 0xFF {
                let next = d[i...].firstIndex(of: 0xFF) ?? d.endIndex
                out.append(d[i..<next])
                i = next
                continue
            }
            guard d[i] == 0xFF else { return nil }
            let start = i
            while i < d.count, d[i] == 0xFF { i += 1 }
            guard i < d.count else { return nil }
            let marker = d[i]
            i += 1
            if (inScan && marker == 0x00) || (0xD0...0xD7).contains(marker) || marker == 0x01 {
                out.append(d[start..<i])
                continue
            }
            if marker == 0xD9 {
                out.append(d[start..<i])
                return out
            }
            guard marker != 0xD8, d.count - i >= 2 else { return nil }
            let len = Int(d[i]) << 8 | Int(d[i + 1])
            guard len >= 2, len <= d.count - i else { return nil }
            let end = i + len
            if keepJpeg(marker, d[(i + 2)..<end]) {
                out.append(d[start..<end])
            }
            i = end
            inScan = marker == 0xDA
        }
        return nil
    }

    private static func keepJpeg(_ marker: UInt8, _ p: Data) -> Bool {
        let b = Array(p)
        switch marker {
        case 0xE0:
            return b.count >= 14 && b[0..<5].elementsEqual(Array("JFIF\0".utf8))
                && b.count == 14 + 3 * Int(b[12]) * Int(b[13])
        case 0xEE:
            return b.count == 12 && b[0..<5].elementsEqual(Array("Adobe".utf8))
        case 0xE1...0xED, 0xEF, 0xFE:
            return false
        default:
            return true
        }
    }

    private static func be32(_ d: Data, _ i: Int) -> UInt32 {
        UInt32(d[i]) << 24 | UInt32(d[i + 1]) << 16 | UInt32(d[i + 2]) << 8 | UInt32(d[i + 3])
    }
}
