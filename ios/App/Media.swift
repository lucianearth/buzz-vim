import SwiftUI
import UIKit

/// Loads an image once and keeps it. Images on our relay go through the
/// Rust client (it signs Blossom auth); anything else is a plain GET.
actor Images {
    static let shared = Images()
    private let cache = NSCache<NSString, UIImage>()

    func load(_ url: String, client: Client) async -> UIImage? {
        if let hit = cache.object(forKey: url as NSString) { return hit }
        var data: Data?
        if let d = try? await client.image(url: url) {
            data = d
        } else if let u = URL(string: url), u.scheme == "https" {
            data = try? await URLSession.shared.data(from: u).0
        }
        guard let data, let img = UIImage(data: data) else { return nil }
        cache.setObject(img, forKey: url as NSString)
        return img
    }
}

struct RelayImage: View {
    let client: Client
    let url: String
    let width: UInt32
    let height: UInt32
    @Environment(\.theme) private var theme
    @State private var image: UIImage?
    @State private var failed = false

    var body: some View {
        Group {
            if let image {
                Image(uiImage: image)
                    .resizable()
                    .scaledToFit()
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
            image = await Images.shared.load(url, client: client)
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
struct ImagePager: View {
    let client: Client
    let viewer: Viewer
    @Environment(\.dismiss) private var dismiss
    @State private var page = 0
    @State private var drag: CGFloat = 0

    var body: some View {
        ZStack {
            Color.black.opacity(1 - min(abs(drag) / 400, 0.6)).ignoresSafeArea()
            TabView(selection: $page) {
                ForEach(Array(viewer.urls.enumerated()), id: \.offset) { i, u in
                    RelayImage(client: client, url: u, width: 0, height: 0)
                        .tag(i)
                }
            }
            .tabViewStyle(.page(indexDisplayMode: viewer.urls.count > 1 ? .automatic : .never))
            .offset(y: drag)
        }
        .gesture(DragGesture()
            .onChanged { g in if g.translation.height > 0 { drag = g.translation.height } }
            .onEnded { g in
                if g.translation.height > 120 { dismiss() } else { withAnimation { drag = 0 } }
            })
        .onAppear { page = viewer.start }
    }
}
