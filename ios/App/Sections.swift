import SwiftUI

/// Which of Desktop's sidebar sections become tabs. Long-press the status line above the tabs.
struct SectionsView: View {
    let client: Client
    @Environment(Model.self) private var model
    @Environment(\.theme) private var theme

    var body: some View {
        let _ = model.version
        let sections = client.sections()
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                Text("tabs come from these sections")
                    .font(.small)
                    .foregroundStyle(theme.muted)
                    .padding(.bottom, 12)
                ForEach(sections, id: \.id) { s in
                    HStack {
                        Text(s.name).font(.label).foregroundStyle(theme.text)
                        Text("\(s.channels)").font(.small).foregroundStyle(theme.faint)
                        Spacer()
                        Image(systemName: s.chosen ? "checkmark.circle.fill" : "circle")
                            .foregroundStyle(s.chosen ? theme.accent : theme.faint)
                    }
                    .padding(.vertical, 12)
                    .contentShape(Rectangle())
                    .onTapGesture { toggle(s, in: sections) }
                    Rectangle().fill(theme.rule).frame(height: 1)
                }
                HStack {
                    Text("signed in as \(client.myName())")
                        .font(.small)
                        .foregroundStyle(theme.muted)
                    Spacer()
                    Button("Log out") { model.logout() }
                        .font(.small)
                        .foregroundStyle(theme.badge)
                }
                .padding(.top, 28)
            }
            .padding(.horizontal, 22)
            .padding(.top, 26)
        }
        .background(theme.sheet)
    }

    private func toggle(_ s: SectionInfo, in all: [SectionInfo]) {
        var ids = all.filter(\.chosen).map(\.id)
        if s.chosen {
            ids.removeAll { $0 == s.id }
        } else {
            ids.append(s.id)
        }
        model.setChosen(ids)
    }
}
