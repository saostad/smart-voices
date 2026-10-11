import Foundation
import Markdown

/// Pause after a spoken segment.
public enum SpeechPause: Sendable, Equatable {
    case none, short, medium, long

    /// Pause duration in seconds.
    public var seconds: Double {
        switch self {
        case .none: 0
        case .short: 0.15
        case .medium: 0.35
        case .long: 0.6
        }
    }
}

/// A speakable text segment with its trailing pause.
public struct SpeechSegment: Sendable, Equatable {
    public var text: String
    public var pause: SpeechPause

    public init(text: String, pause: SpeechPause) {
        self.text = text
        self.pause = pause
    }
}

/// Options controlling sanitizer output.
public struct SanitizerOptions: Sendable, Equatable {
    public enum CodeBlocks: Sendable, Equatable {
        case announce, skip, readFull
    }

    public enum Tables: Sendable, Equatable {
        case readRows, announce, skip
    }

    public enum BareURLs: Sendable, Equatable {
        case domainOnly, skip
    }

    public var codeBlocks: CodeBlocks
    public var tables: Tables
    public var bareURLs: BareURLs
    public var maxTableRowsToRead: Int

    public init(
        codeBlocks: CodeBlocks = .announce,
        tables: Tables = .readRows,
        bareURLs: BareURLs = .domainOnly,
        maxTableRowsToRead: Int = 15
    ) {
        self.codeBlocks = codeBlocks
        self.tables = tables
        self.bareURLs = bareURLs
        self.maxTableRowsToRead = maxTableRowsToRead
    }
}

/// Converts Markdown into speakable segments.
public struct SpeechSanitizer: Sendable {
    private let options: SanitizerOptions

    public init(options: SanitizerOptions = .init()) {
        self.options = options
    }

    /// Sanitize Markdown (or plain text) into speakable segments.
    public func sanitize(_ text: String) -> [SpeechSegment] {
        if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            return []
        }
        let document = Document(parsing: text)
        var segments: [SpeechSegment] = []
        appendBlocks(document.children, to: &segments)
        return segments
    }

    // MARK: - Blocks

    private func appendBlocks(_ children: MarkupChildren, to segments: inout [SpeechSegment]) {
        for child in children {
            appendBlock(child, to: &segments)
        }
    }

    private func appendBlock(_ block: Markup, to segments: inout [SpeechSegment]) {
        if let heading = block as? Heading {
            let raw = inlineText(for: heading.children)
            if let cleaned = cleanText(raw) {
                segments.append(SpeechSegment(text: cleaned, pause: .long))
            }
            return
        }
        if let paragraph = block as? Paragraph {
            let raw = inlineText(for: paragraph.children)
            if let cleaned = cleanText(raw) {
                segments.append(SpeechSegment(text: cleaned, pause: .medium))
            }
            return
        }
        if block is OrderedList || block is UnorderedList {
            segments.append(contentsOf: listSegments(for: block))
            return
        }
        if let quote = block as? BlockQuote {
            appendBlocks(quote.children, to: &segments)
            return
        }
        if let code = block as? CodeBlock {
            segments.append(contentsOf: codeSegments(for: code))
            return
        }
        if let table = block as? Table {
            segments.append(contentsOf: tableSegments(for: table))
            return
        }
        if block is ThematicBreak {
            if !segments.isEmpty {
                segments[segments.count - 1].pause = .long
            }
            return
        }
        if let html = block as? HTMLBlock {
            let stripped = stripHTML(html.rawHTML)
            if let cleaned = cleanText(stripped) {
                segments.append(SpeechSegment(text: cleaned, pause: .medium))
            }
            return
        }
        if let item = block as? ListItem {
            // Should only occur inside lists, but handle gracefully.
            appendBlocks(item.children, to: &segments)
            return
        }
        // Generic block container fallback: recurse into children.
        if block.childCount > 0 {
            appendBlocks(block.children, to: &segments)
        }
    }

    // MARK: - Lists

    private func listSegments(for list: Markup) -> [SpeechSegment] {
        let isOrdered = list is OrderedList
        var startIndex = 1
        if let ordered = list as? OrderedList {
            startIndex = Int(ordered.startIndex)
        }
        var items: [ListItem] = []
        for child in list.children {
            if let item = child as? ListItem {
                items.append(item)
            }
        }
        // Collect (own segment, nested segments) per item.
        var owned: [SpeechSegment?] = []
        var nested: [[SpeechSegment]] = []
        for (offset, item) in items.enumerated() {
            let number = startIndex + offset
            owned.append(listItemSegment(for: item, isOrdered: isOrdered, number: number))
            nested.append(listItemNestedSegments(for: item))
        }
        // Last owned segment of this list gets .medium, others .short.
        var lastOwned = -1
        for index in owned.indices {
            if owned[index] != nil {
                lastOwned = index
            }
        }
        var result: [SpeechSegment] = []
        for index in items.indices {
            if var own = owned[index] {
                own.pause = (index == lastOwned) ? .medium : .short
                result.append(own)
            }
            result.append(contentsOf: nested[index])
        }
        return result
    }

    private func listItemSegment(for item: ListItem, isOrdered: Bool, number: Int) -> SpeechSegment? {
        var raws: [String] = []
        for child in item.children {
            if let paragraph = child as? Paragraph {
                raws.append(inlineText(for: paragraph.children))
            }
        }
        let joined = raws.joined(separator: " ")
        guard let cleaned = cleanFragment(joined), !cleaned.isEmpty else {
            return nil
        }
        var prefix = ""
        if let checkbox = item.checkbox {
            switch checkbox {
            case .checked: prefix += "Done: "
            case .unchecked: prefix += "To do: "
            }
        }
        if isOrdered {
            prefix += ordinalPrefix(number)
        }
        let text = withTerminalPunctuation(prefix + cleaned)
        return SpeechSegment(text: text, pause: .short)
    }

    private func listItemNestedSegments(for item: ListItem) -> [SpeechSegment] {
        var result: [SpeechSegment] = []
        for child in item.children {
            if child is Paragraph {
                continue
            }
            if child is OrderedList || child is UnorderedList {
                result.append(contentsOf: listSegments(for: child))
            } else {
                appendBlock(child, to: &result)
            }
        }
        return result
    }

    private func ordinalPrefix(_ number: Int) -> String {
        switch number {
        case 1: return "First, "
        case 2: return "Second, "
        case 3: return "Third, "
        case 4: return "Fourth, "
        case 5: return "Fifth, "
        case 6: return "Sixth, "
        case 7: return "Seventh, "
        case 8: return "Eighth, "
        case 9: return "Ninth, "
        case 10: return "Tenth, "
        default: return "Number \(number), "
        }
    }

    // MARK: - Code blocks

    private func codeSegments(for code: CodeBlock) -> [SpeechSegment] {
        switch options.codeBlocks {
        case .skip:
            return []
        case .announce:
            return [SpeechSegment(text: codeAnnouncement(language: code.language), pause: .medium)]
        case .readFull:
            let lines = code.code.components(separatedBy: .newlines)
            var result: [SpeechSegment] = []
            for line in lines {
                if let cleaned = cleanText(line) {
                    result.append(SpeechSegment(text: cleaned, pause: .short))
                }
            }
            if !result.isEmpty {
                result[result.count - 1].pause = .medium
            }
            return result
        }
    }

    private func codeAnnouncement(language: String?) -> String {
        let info = (language ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        if info.lowercased() == "mermaid" {
            return "Here's a diagram."
        }
        if info.isEmpty {
            return "Here's a code snippet."
        }
        let key = info.lowercased()
        let display: String = {
            switch key {
            case "py", "python": return "Python"
            case "js", "javascript": return "JavaScript"
            case "ts", "typescript": return "TypeScript"
            case "sh", "bash", "zsh", "shell": return "shell"
            case "swift": return "Swift"
            case "json": return "JSON"
            case "yaml", "yml": return "YAML"
            case "sql": return "SQL"
            default: return info
            }
        }()
        return "Here's a \(display) code snippet."
    }

    // MARK: - Tables

    private func tableSegments(for table: Table) -> [SpeechSegment] {
        switch options.tables {
        case .skip:
            return []
        case .announce:
            return [tableSummary(for: table)]
        case .readRows:
            let rowCount = table.body.childCount
            if rowCount > options.maxTableRowsToRead {
                return [tableSummary(for: table)]
            }
            var result: [SpeechSegment] = []
            result.append(SpeechSegment(text: tableIntro(rowCount: rowCount), pause: .medium))
            let headers = headerFragments(for: table)
            let allHeadersEmpty = headers.allSatisfy { $0 == nil }
            for row in table.body.rows {
                var cells: [String?] = []
                for cell in row.cells {
                    cells.append(cleanFragment(inlineText(for: cell.children)))
                }
                if let text = tableRowText(cells: cells, headers: headers, allHeadersEmpty: allHeadersEmpty) {
                    result.append(SpeechSegment(text: text, pause: .medium))
                }
            }
            return result
        }
    }

    private func headerFragments(for table: Table) -> [String?] {
        table.head.cells.map { cleanFragment(inlineText(for: $0.children)) }.map {
            guard let value = $0, !value.isEmpty else { return nil }
            return value
        }
    }

    private func tableIntro(rowCount: Int) -> String {
        if rowCount == 1 {
            return "A table with 1 row."
        }
        return "A table with \(rowCount) rows."
    }

    private func tableSummary(for table: Table) -> SpeechSegment {
        let rows = table.body.childCount
        let columns = table.maxColumnCount
        let rowWord = rows == 1 ? "1 row" : "\(rows) rows"
        let colWord = columns == 1 ? "1 column" : "\(columns) columns"
        var text = "A table with \(rowWord) and \(colWord)"
        let names = headerFragments(for: table).compactMap { $0 }.filter { !$0.isEmpty }
        if !names.isEmpty {
            text += ", comparing \(andList(names))"
        }
        text += "."
        return SpeechSegment(text: text, pause: .medium)
    }

    private func andList(_ names: [String]) -> String {
        switch names.count {
        case 0: return ""
        case 1: return names[0]
        case 2: return "\(names[0]) and \(names[1])"
        default:
            let head = names.dropLast().joined(separator: ", ")
            return "\(head) and \(names[names.count - 1])"
        }
    }

    private func tableRowText(cells: [String?], headers: [String?], allHeadersEmpty: Bool) -> String? {
        let cleanedCells = cells.map { ($0 ?? "").trimmingCharacters(in: .whitespacesAndNewlines) }
        let cleanedHeaders = headers.map { ($0 ?? "").trimmingCharacters(in: .whitespacesAndNewlines) }
        let nonEmptyCells = cleanedCells.filter { !$0.isEmpty }
        if nonEmptyCells.isEmpty {
            return nil
        }
        if allHeadersEmpty {
            return withTerminalPunctuation(nonEmptyCells.joined(separator: ", "))
        }
        // First cell is the subject; remaining pair with headers.
        let first = cleanedCells.first ?? ""
        var parts: [String] = []
        for index in cleanedCells.indices.dropFirst() {
            let cell = cleanedCells[index]
            if cell.isEmpty {
                continue
            }
            let header = index < cleanedHeaders.count ? cleanedHeaders[index] : ""
            if header.isEmpty {
                parts.append(cell)
            } else {
                parts.append("\(header) \(cell)")
            }
        }
        if first.isEmpty {
            return withTerminalPunctuation(parts.joined(separator: ", "))
        }
        if parts.isEmpty {
            return withTerminalPunctuation(first)
        }
        return withTerminalPunctuation("\(first): \(parts.joined(separator: ", "))")
    }

    // MARK: - Inline text

    private func inlineText(for children: MarkupChildren) -> String {
        var result = ""
        for child in children {
            result += inlineText(for: child)
        }
        return result
    }

    private func inlineText(for markup: Markup) -> String {
        if let text = markup as? Text {
            return text.string
        }
        if let code = markup as? InlineCode {
            return code.code
        }
        if let link = markup as? Link {
            var inner = ""
            for child in link.children {
                inner += inlineText(for: child)
            }
            if let destination = link.destination,
                inner == destination,
                isHTTPURL(destination)
            {
                return bareURLSpokenString(for: destination)
            }
            return inner
        }
        if markup is Image {
            return ""
        }
        if markup is SoftBreak || markup is LineBreak {
            return " "
        }
        if let html = markup as? InlineHTML {
            return isBreakTag(html.rawHTML) ? " " : ""
        }
        if markup.childCount > 0 {
            var result = ""
            for child in markup.children {
                result += inlineText(for: child)
            }
            return result
        }
        return ""
    }

    // MARK: - Cleaning

    private func cleanText(_ raw: String) -> String? {
        guard let fragment = cleanFragment(raw), !fragment.isEmpty else {
            return nil
        }
        return withTerminalPunctuation(fragment)
    }

    private func cleanFragment(_ raw: String) -> String? {
        var value = raw
        value = removeCitations(value)
        value = removeEmoji(value)
        value = replaceBareURLs(value)
        value = applyWordReplacements(value)
        value = collapseWhitespace(value)
        value = removeLeftoverMarkup(value)
        value = collapseWhitespace(value)
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty {
            return nil
        }
        return trimmed
    }

    private func withTerminalPunctuation(_ text: String) -> String {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty {
            return trimmed
        }
        if let last = trimmed.last, ".!?:;…".contains(last) {
            return trimmed
        }
        return trimmed + "."
    }

    private func collapseWhitespace(_ value: String) -> String {
        var result = ""
        result.reserveCapacity(value.count)
        var inSpace = false
        for scalar in value.unicodeScalars {
            if CharacterSet.whitespacesAndNewlines.contains(scalar) {
                if !inSpace {
                    result.append(" ")
                    inSpace = true
                }
            } else {
                result.unicodeScalars.append(scalar)
                inSpace = false
            }
        }
        return result.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    // MARK: Citations

    private func removeCitations(_ value: String) -> String {
        var result = ""
        result.reserveCapacity(value.count)
        var index = value.startIndex
        while index < value.endIndex {
            if value[index] == "[",
                let close = value[index...].firstIndex(of: "]"),
                close < value.index(index, offsetBy: 60, limitedBy: value.endIndex) ?? value.endIndex
            {
                let inside = String(value[value.index(after: index)..<close])
                if isFootnoteRef(inside) || isNumericCitation(inside) {
                    index = value.index(after: close)
                    continue
                }
            }
            result.append(value[index])
            index = value.index(after: index)
        }
        return result
    }

    private func isFootnoteRef(_ inside: String) -> Bool {
        inside.hasPrefix("^") && inside.count > 1
    }

    private func isNumericCitation(_ inside: String) -> Bool {
        let trimmed = inside.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty {
            return false
        }
        var hasDigit = false
        for scalar in trimmed.unicodeScalars {
            if CharacterSet.decimalDigits.contains(scalar) {
                hasDigit = true
                continue
            }
            let char = Character(scalar)
            if char == "," || char == "-" || char == "–" || char == "—"
                || CharacterSet.whitespaces.contains(scalar)
            {
                continue
            }
            return false
        }
        if !hasDigit {
            return false
        }
        guard let first = trimmed.first, let last = trimmed.last else {
            return false
        }
        return first.isNumber && last.isNumber
    }

    // MARK: Emoji

    private func removeEmoji(_ value: String) -> String {
        var result = ""
        result.reserveCapacity(value.count)
        for character in value {
            let scalars = Array(character.unicodeScalars)
            guard let first = scalars.first else {
                continue
            }
            if first.properties.isEmojiPresentation {
                continue
            }
            if scalars.count > 1 && first.properties.isEmoji {
                continue
            }
            result.append(character)
        }
        return result
    }

    // MARK: Bare URLs

    private func isHTTPURL(_ value: String) -> Bool {
        let lower = value.lowercased()
        return lower.hasPrefix("http://") || lower.hasPrefix("https://")
    }

    private func bareURLSpokenString(for url: String) -> String {
        switch options.bareURLs {
        case .skip:
            return ""
        case .domainOnly:
            if let host = hostForURL(url), !host.isEmpty {
                return "link to \(host)"
            }
            return ""
        }
    }

    private func hostForURL(_ url: String) -> String? {
        let trimmed = url.trimmingCharacters(in: .whitespacesAndNewlines)
        if let parsed = URL(string: trimmed), let host = parsed.host, !host.isEmpty {
            return stripWWW(host)
        }
        // Manual fallback: strip scheme, take authority.
        var rest = trimmed
        if let range = rest.range(of: "://") {
            rest = String(rest[range.upperBound...])
        }
        for separator in ["/", "?", "#"] {
            if let end = rest.firstIndex(of: Character(separator)) {
                rest = String(rest[..<end])
            }
        }
        if let colon = rest.firstIndex(of: ":") {
            rest = String(rest[..<colon])
        }
        rest = rest.trimmingCharacters(in: .whitespacesAndNewlines)
        if rest.isEmpty {
            return nil
        }
        return stripWWW(rest)
    }

    private func stripWWW(_ host: String) -> String {
        if host.lowercased().hasPrefix("www.") {
            return String(host.dropFirst(4))
        }
        return host
    }

    private func replaceBareURLs(_ value: String) -> String {
        var result = ""
        result.reserveCapacity(value.count)
        var index = value.startIndex
        while index < value.endIndex {
            if let match = matchURLScheme(at: index, in: value) {
                let urlStart = match
                var urlEnd = urlStart
                while urlEnd < value.endIndex, !isURLTerminator(value[urlEnd]) {
                    urlEnd = value.index(after: urlEnd)
                }
                var url = String(value[urlStart..<urlEnd])
                // Trim trailing sentence punctuation, keep it in output.
                var trailing = ""
                while let last = url.last, ".,;:!?()[]{}\"'".contains(last) {
                    trailing = String(last) + trailing
                    url.removeLast()
                }
                result += bareURLSpokenString(for: url)
                result += trailing
                index = urlEnd
            } else {
                result.append(value[index])
                index = value.index(after: index)
            }
        }
        return result
    }

    private func matchURLScheme(at index: String.Index, in value: String) -> String.Index? {
        let remaining = value[index...]
        let lower = remaining.lowercased()
        if lower.hasPrefix("https://") || lower.hasPrefix("http://") {
            // Require a word boundary before the scheme.
            if index > value.startIndex {
                let previous = value[value.index(before: index)]
                if previous.isLetter || previous.isNumber {
                    return nil
                }
            }
            return index
        }
        return nil
    }

    private func isURLTerminator(_ character: Character) -> Bool {
        if character.isWhitespace || character.isNewline {
            return true
        }
        return character == "<" || character == ">" || character == "\"" || character == "`" || character == "|"
    }

    // MARK: Word replacements

    private func applyWordReplacements(_ value: String) -> String {
        var result = value
        result = replaceWord(result, target: "e.g.", replacement: "for example")
        result = replaceWord(result, target: "i.e.", replacement: "that is")
        result = replaceWord(result, target: "etc.", replacement: "et cetera")
        result = replaceWord(result, target: "vs.", replacement: "versus")
        result = replaceWord(result, target: "vs", replacement: "versus")
        result = replaceWord(result, target: "w/", replacement: "with")
        result = result.replacingOccurrences(of: "->", with: " to ")
        result = result.replacingOccurrences(of: "→", with: " to ")
        result = result.replacingOccurrences(of: "&", with: " and ")
        return result
    }

    private func replaceWord(_ value: String, target: String, replacement: String) -> String {
        if value.isEmpty || target.isEmpty {
            return value
        }
        let lowerValue = value.lowercased()
        let lowerTarget = target.lowercased()
        var result = ""
        result.reserveCapacity(value.count)
        var index = value.startIndex
        while index < value.endIndex {
            let remaining = value.distance(from: index, to: value.endIndex)
            if remaining >= target.count,
                lowerValue[index...].hasPrefix(lowerTarget),
                isWordBoundary(value, before: index),
                isWordBoundary(value, after: value.index(index, offsetBy: target.count))
            {
                result += replacement
                index = value.index(index, offsetBy: target.count)
            } else {
                result.append(value[index])
                index = value.index(after: index)
            }
        }
        return result
    }

    private func isWordBoundary(_ value: String, before index: String.Index) -> Bool {
        if index == value.startIndex {
            return true
        }
        let previous = value[value.index(before: index)]
        return !previous.isLetter && !previous.isNumber
    }

    private func isWordBoundary(_ value: String, after index: String.Index) -> Bool {
        if index >= value.endIndex {
            return true
        }
        let next = value[index]
        return !next.isLetter && !next.isNumber
    }

    // MARK: Leftover markup

    private func removeLeftoverMarkup(_ value: String) -> String {
        var result = removeStarRuns(value)
        result = result.replacingOccurrences(of: "`", with: "")
        result = result.replacingOccurrences(of: "~~", with: "")
        result = result.replacingOccurrences(of: "|", with: " ")
        result = stripLeadingMarkers(result)
        return result
    }

    private func removeStarRuns(_ value: String) -> String {
        var result = ""
        result.reserveCapacity(value.count)
        let chars = Array(value)
        var index = 0
        while index < chars.count {
            let char = chars[index]
            if char == "*" || char == "_" {
                var end = index
                while end < chars.count, chars[end] == char {
                    end += 1
                }
                let before: Character? = index > 0 ? chars[index - 1] : nil
                let after: Character? = end < chars.count ? chars[end] : nil
                let beforeIsWord = before?.isLetter == true || before?.isNumber == true
                let afterIsWord = after?.isLetter == true || after?.isNumber == true
                if beforeIsWord && afterIsWord {
                    result += String(chars[index..<end])
                }
                index = end
            } else {
                result.append(char)
                index += 1
            }
        }
        return result
    }

    private func stripLeadingMarkers(_ value: String) -> String {
        var result = value.trimmingCharacters(in: .whitespacesAndNewlines)
        var changed = true
        while changed {
            changed = false
            if result.hasPrefix("> ") {
                result = String(result.dropFirst(2)).trimmingCharacters(in: .whitespacesAndNewlines)
                changed = true
                continue
            }
            if result.hasPrefix("#") {
                var count = 0
                for char in result {
                    if char == "#" {
                        count += 1
                    } else {
                        break
                    }
                    if count > 6 {
                        break
                    }
                }
                if count >= 1, count <= 6 {
                    let dropIndex = result.index(result.startIndex, offsetBy: count, limitedBy: result.endIndex)
                    if let dropIndex, dropIndex < result.endIndex, result[dropIndex] == " " {
                        result = String(result[result.index(after: dropIndex)...])
                            .trimmingCharacters(in: .whitespacesAndNewlines)
                        changed = true
                    }
                }
            }
        }
        return result
    }

    // MARK: - HTML

    private func isBreakTag(_ html: String) -> Bool {
        let trimmed = html.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if !trimmed.hasPrefix("<") || !trimmed.hasSuffix(">") {
            return false
        }
        let inner = trimmed.dropFirst().dropLast()
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: "/", with: "")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return inner == "br"
    }

    private func stripHTML(_ html: String) -> String {
        var result = html
        // Comments first.
        while let start = result.range(of: "<!--"),
            let end = result.range(of: "-->", range: start.upperBound..<result.endIndex)
        {
            result.replaceSubrange(start.lowerBound..<end.upperBound, with: " ")
        }
        // <br> variants to space.
        result = replaceHTMLBreaks(result)
        // Remaining tags removed.
        var stripped = ""
        stripped.reserveCapacity(result.count)
        var index = result.startIndex
        while index < result.endIndex {
            if result[index] == "<",
                let close = result[index...].firstIndex(of: ">")
            {
                let tag = String(result[index...close])
                // Only treat letter-led tags as HTML to keep "a < b".
                let inner = tag.dropFirst().trimmingCharacters(in: .whitespacesAndNewlines)
                if let first = inner.first, first.isLetter || first == "/" || first == "!" || first == "?" {
                    index = result.index(after: close)
                    continue
                }
            }
            stripped.append(result[index])
            index = result.index(after: index)
        }
        return decodeHTMLEntities(stripped)
    }

    private func replaceHTMLBreaks(_ value: String) -> String {
        var result = ""
        result.reserveCapacity(value.count)
        var index = value.startIndex
        while index < value.endIndex {
            if value[index] == "<",
                let close = value[index...].firstIndex(of: ">"),
                close < value.index(index, offsetBy: 12, limitedBy: value.endIndex) ?? value.endIndex
            {
                let tag = String(value[index...close])
                if isBreakTag(tag) {
                    result += " "
                    index = value.index(after: close)
                    continue
                }
            }
            result.append(value[index])
            index = value.index(after: index)
        }
        return result
    }

    private func decodeHTMLEntities(_ value: String) -> String {
        var result = value
        result = result.replacingOccurrences(of: "&nbsp;", with: " ")
        result = result.replacingOccurrences(of: "&amp;", with: "&")
        result = result.replacingOccurrences(of: "&lt;", with: "<")
        result = result.replacingOccurrences(of: "&gt;", with: ">")
        result = result.replacingOccurrences(of: "&quot;", with: "\"")
        result = result.replacingOccurrences(of: "&#39;", with: "'")
        result = result.replacingOccurrences(of: "&#x27;", with: "'")
        return result
    }
}
