import ImageIO
import SwiftUI
import UIKit

/// Loads an image once and keeps it. Images on our relay go through the
/// Rust client (it signs Blossom auth, and stops past the upload cap);
/// anything else is a plain GET with the same cap. Decoding is scaled down
/// to `maxPixels` on the long side, so a huge image never decodes whole.
actor Images {
    static let shared = Images()
    /// The relay's image upload limit (MAX_IMAGE_BYTES).
    static let maxBytes = 50 * 1024 * 1024
    private let cache: NSCache<NSString, UIImage> = {
        let c = NSCache<NSString, UIImage>()
        c.totalCostLimit = 96 * 1024 * 1024
        return c
    }()

    func load(_ url: String, client: Client, maxPixels: Int = 1600) async -> UIImage? {
        let key = "\(maxPixels) \(url)" as NSString
        if let hit = cache.object(forKey: key) { return hit }
        var data: Data?
        if let d = try? await client.image(url: url) {
            data = d
        } else if let u = URL(string: url), u.scheme == "https" {
            data = await Self.fetch(u)
        }
        guard let data, let img = Self.decode(data, maxPixels: maxPixels) else { return nil }
        let cost = Int(img.size.width * img.scale * img.size.height * img.scale) * 4
        cache.setObject(img, forKey: key, cost: cost)
        return img
    }

    private static func fetch(_ url: URL) async -> Data? {
        guard let got = try? await URLSession.shared.bytes(from: url),
              got.1.expectedContentLength <= maxBytes else { return nil }
        var out = Data()
        do {
            for try await b in got.0 {
                out.append(b)
                if out.count > maxBytes { return nil }
            }
        } catch { return nil }
        return out
    }

    /// A thumbnail decode: ImageIO reads only what the target size needs.
    private static func decode(_ data: Data, maxPixels: Int) -> UIImage? {
        guard let src = CGImageSourceCreateWithData(data as CFData, nil) else { return nil }
        let opts: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceShouldCacheImmediately: true,
            kCGImageSourceThumbnailMaxPixelSize: maxPixels,
        ]
        guard let cg = CGImageSourceCreateThumbnailAtIndex(src, 0, opts as CFDictionary) else { return nil }
        return UIImage(cgImage: cg)
    }
}

struct RelayImage: View {
    let client: Client
    let url: String
    let width: UInt32
    let height: UInt32
    /// Crop to fill the frame (avatars) instead of fitting inside it.
    var fill = false
    /// Long side to decode at, in pixels.
    var maxPixels = 1600
    @Environment(\.theme) private var theme
    @State private var image: UIImage?
    @State private var failed = false

    var body: some View {
        Group {
            if let image {
                Image(uiImage: image)
                    .resizable()
                    .aspectRatio(contentMode: fill ? .fill : .fit)
            } else {
                Rectangle()
                    .fill(theme.card)
                    .aspectRatio(width > 0 && height > 0 ? CGFloat(width) / CGFloat(height) : 4 / 3,
                                 contentMode: .fit)
                    .overlay {
                        if failed {
                            Image(systemName: "photo").foregroundStyle(theme.faint)
                        }
                    }
            }
        }
        .task(id: url) {
            image = await Images.shared.load(url, client: client, maxPixels: maxPixels)
            failed = image == nil
        }
    }
}

struct Viewer: Identifiable {
    let id = UUID()
    let urls: [String]
    let start: Int
}

/// Full screen: swipe sideways through the message's images, down to close.
/// Pinch or double-tap to zoom; while zoomed, drags pan the image.
struct ImagePager: View {
    let client: Client
    let viewer: Viewer
    @Environment(\.dismiss) private var dismiss
    @State private var page = 0
    @State private var drag: CGFloat = 0
    @State private var zoomed = false

    var body: some View {
        ZStack {
            Color.black.opacity(1 - min(abs(drag) / 400, 0.6)).ignoresSafeArea()
            TabView(selection: $page) {
                ForEach(Array(viewer.urls.enumerated()), id: \.offset) { i, u in
                    ZoomImage(client: client, url: u, zoomed: $zoomed)
                        .ignoresSafeArea()
                        .tag(i)
                }
            }
            .tabViewStyle(.page(indexDisplayMode: viewer.urls.count > 1 ? .automatic : .never))
            .offset(y: drag)
        }
        // Zoomed in, a drag pans the image instead of closing.
        .gesture(DragGesture()
            .onChanged { g in if g.translation.height > 0 { drag = g.translation.height } }
            .onEnded { g in
                if g.translation.height > 120 { dismiss() } else { withAnimation { drag = 0 } }
            }, including: zoomed ? .subviews : .all)
        .onAppear { page = viewer.start }
        .onChange(of: page) { _, _ in zoomed = false }
    }
}

/// One image in UIKit's zooming scroll view: pinch, double-tap and pan
/// behave as in Photos.
struct ZoomImage: UIViewRepresentable {
    let client: Client
    let url: String
    @Binding var zoomed: Bool

    func makeCoordinator() -> Coordinator { Coordinator(zoomed: $zoomed) }

    func makeUIView(context: Context) -> UIScrollView {
        let scroll = UIScrollView()
        scroll.delegate = context.coordinator
        scroll.maximumZoomScale = 5
        scroll.showsHorizontalScrollIndicator = false
        scroll.showsVerticalScrollIndicator = false
        scroll.contentInsetAdjustmentBehavior = .never
        scroll.backgroundColor = .clear
        let view = context.coordinator.view
        view.contentMode = .scaleAspectFit
        view.frame = scroll.bounds
        view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        scroll.addSubview(view)
        let tap = UITapGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.doubleTap(_:)))
        tap.numberOfTapsRequired = 2
        scroll.addGestureRecognizer(tap)
        Task { @MainActor in
            view.image = await Images.shared.load(url, client: client, maxPixels: 4096)
        }
        return scroll
    }

    func updateUIView(_ scroll: UIScrollView, context: Context) {}

    final class Coordinator: NSObject, UIScrollViewDelegate {
        let view = UIImageView()
        var zoomed: Binding<Bool>

        init(zoomed: Binding<Bool>) { self.zoomed = zoomed }

        func viewForZooming(in scrollView: UIScrollView) -> UIView? { view }

        func scrollViewDidEndZooming(_ scrollView: UIScrollView, with view: UIView?, atScale scale: CGFloat) {
            zoomed.wrappedValue = scale > 1.01
        }

        @objc func doubleTap(_ g: UITapGestureRecognizer) {
            guard let scroll = g.view as? UIScrollView else { return }
            if scroll.zoomScale > 1.01 {
                scroll.setZoomScale(1, animated: true)
            } else {
                let p = g.location(in: view)
                let w = scroll.bounds.width / 2.5, h = scroll.bounds.height / 2.5
                scroll.zoom(to: CGRect(x: p.x - w / 2, y: p.y - h / 2, width: w, height: h), animated: true)
            }
        }
    }
}
