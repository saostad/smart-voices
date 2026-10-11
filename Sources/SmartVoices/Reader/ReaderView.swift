import SmartVoicesCore
import SwiftUI

/// Reading panel content: chunk list with live word highlighting plus controls.
@MainActor
struct ReaderView: View {
    @Bindable var playback: PlaybackController
    @Bindable var header: ReaderHeaderModel
    var onJump: (Int) -> Void
    var onTogglePlayPause: () -> Void
    var onNext: () -> Void
    var onPrevious: () -> Void
    var onSpeed: (Double) -> Void
    var onClose: () -> Void
    /// Forces the highlighted word (used by --render-reader).
    var previewWordIndex: Int?

    private let speeds = [0.75, 1.0, 1.25, 1.5, 1.75, 2.0]

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text(header.title)
                    .font(.headline)
                    .lineLimit(1)
                    .truncationMode(.tail)
                Spacer()
                Button(action: onClose) {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            Divider()
            TimelineView(.animation(minimumInterval: 1 / 15)) { timeline in
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 6) {
                            if playback.chunks.isEmpty {
                                Text("Nothing to read yet.")
                                    .foregroundStyle(.secondary)
                                    .padding(8)
                            } else {
                                ForEach(playback.chunks.indices, id: \.self) { index in
                                    chunkRow(index, now: timeline.date)
                                }
                            }
                        }
                        .padding(12)
                    }
                    .onChange(of: playback.currentChunkIndex) { _, newIndex in
                        withAnimation {
                            proxy.scrollTo(newIndex, anchor: .center)
                        }
                    }
                    .onAppear {
                        proxy.scrollTo(playback.currentChunkIndex, anchor: .center)
                    }
                }
            }
            Divider()
            footer
        }
    }

    @ViewBuilder
    private func chunkRow(_ index: Int, now: Date) -> some View {
        let isCurrent = index == playback.currentChunkIndex && playback.state != .idle
        let isPast = index < playback.currentChunkIndex && playback.state != .idle
        Button {
            onJump(index)
        } label: {
            chunkText(playback.chunks[index], highlighted: isCurrent, now: now)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(8)
                .background(
                    isCurrent ? Color.accentColor.opacity(0.15) : Color.clear,
                    in: RoundedRectangle(cornerRadius: 8))
        }
        .buttonStyle(.plain)
        .foregroundStyle(isPast ? .secondary : .primary)
        .id(index)
    }

    private func chunkText(_ chunk: SpeechChunk, highlighted: Bool, now: Date) -> Text {
        guard highlighted else {
            return Text(chunk.text)
        }
        let wordIndex: Int? =
            if let previewWordIndex {
                previewWordIndex
            } else if let startedAt = playback.currentChunkStartedAt {
                WordTiming.wordIndex(
                    in: chunk.text, elapsed: now.timeIntervalSince(startedAt),
                    duration: playback.currentChunkDuration)
            } else {
                nil
            }
        guard let wordIndex else {
            return Text(chunk.text)
        }
        return Text(highlightedText(chunk.text, word: wordIndex))
    }

    private func highlightedText(_ text: String, word: Int) -> AttributedString {
        var result = AttributedString(text)
        var seen = -1
        var cursor = text.startIndex
        var target: Range<String.Index>?
        while cursor < text.endIndex {
            while cursor < text.endIndex, text[cursor].isWhitespace {
                cursor = text.index(after: cursor)
            }
            if cursor >= text.endIndex {
                break
            }
            let start = cursor
            while cursor < text.endIndex, !text[cursor].isWhitespace {
                cursor = text.index(after: cursor)
            }
            seen += 1
            if seen == word {
                target = start..<cursor
                break
            }
        }
        guard let target, let range = Range(target, in: result) else {
            return result
        }
        result[range].font = .body.bold()
        result[range].foregroundColor = .accentColor
        return result
    }

    private var footer: some View {
        VStack(spacing: 6) {
            HStack {
                Button(action: onPrevious) {
                    Image(systemName: "chevron.backward")
                        .font(.title3)
                }
                .buttonStyle(.plain)
                .disabled(!canGoPrevious)
                Button(action: onTogglePlayPause) {
                    Image(
                        systemName: playback.state == .playing
                            ? "pause.fill" : "play.fill"
                    )
                    .font(.title)
                }
                .buttonStyle(.plain)
                .disabled(playback.chunks.isEmpty)
                Button(action: onNext) {
                    Image(systemName: "chevron.forward")
                        .font(.title3)
                }
                .buttonStyle(.plain)
                .disabled(!canGoNext)
                Spacer()
                Menu {
                    ForEach(speeds, id: \.self) { value in
                        Button(String(format: "%g×", value)) {
                            onSpeed(value)
                        }
                    }
                } label: {
                    Text(String(format: "%g×", playback.speed))
                }
                .menuStyle(.borderlessButton)
            }
            ProgressView(value: progress)
                .controlSize(.small)
            Text(progressLabel)
                .font(.caption)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(12)
    }

    private var canGoPrevious: Bool {
        playback.state != .idle && !playback.chunks.isEmpty && playback.currentChunkIndex > 0
    }

    private var canGoNext: Bool {
        playback.state != .idle && !playback.chunks.isEmpty
            && playback.currentChunkIndex < playback.chunks.count - 1
    }

    private var progress: Double {
        let count = playback.chunks.count
        if count == 0 {
            return 0
        }
        return Double(min(playback.currentChunkIndex + 1, count)) / Double(count)
    }

    private var progressLabel: String {
        let count = playback.chunks.count
        if count == 0 {
            return "No sentences"
        }
        return "Sentence \(min(playback.currentChunkIndex + 1, count)) of \(count)"
    }
}
