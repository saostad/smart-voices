import Foundation
import Testing
@testable import SmartVoicesCore

@Suite struct SpeechSanitizerTests {
    let sanitizer = SpeechSanitizer()

    @Test func headingAndParagraphPauses() {
        let result = sanitizer.sanitize("# Title\n\nBody text")
        #expect(result == [
            SpeechSegment(text: "Title.", pause: .long),
            SpeechSegment(text: "Body text.", pause: .medium),
        ])
    }

    @Test func inlineFormattingStripsToText() {
        let result = sanitizer.sanitize("Hello **bold** and *italic* and ~~gone~~.")
        #expect(result.count == 1)
        #expect(result[0].text == "Hello bold and italic and gone.")
    }

    @Test func inlineCodeAndLinkAndImage() {
        let result = sanitizer.sanitize("Use `print()` and a [link text](https://example.com) plus ![alt](img.png) done.")
        #expect(result.count == 1)
        #expect(result[0].text == "Use print() and a link text plus done.")
    }

    @Test func softAndHardBreaksBecomeSpaces() {
        let soft = sanitizer.sanitize("one\ntwo")
        #expect(soft[0].text == "one two.")
        let hard = sanitizer.sanitize("one  \ntwo")
        #expect(hard[0].text == "one two.")
    }

    @Test func bareURLBecomesLinkToHost() {
        let result = sanitizer.sanitize("See https://www.example.com/page for details.")
        #expect(result[0].text == "See link to example.com for details.")
    }

    @Test func bareURLSkipRemoves() {
        let s = SpeechSanitizer(options: SanitizerOptions(bareURLs: .skip))
        let result = s.sanitize("See https://example.com/page for details.")
        #expect(result[0].text == "See for details.")
    }

    @Test func autolinkBecomesLinkToHost() {
        let result = sanitizer.sanitize("See <https://example.com/x>.")
        #expect(result[0].text == "See link to example.com.")
    }

    @Test func unorderedListPauses() {
        let result = sanitizer.sanitize("- Apples\n- Oranges\n- Bananas")
        #expect(result == [
            SpeechSegment(text: "Apples.", pause: .short),
            SpeechSegment(text: "Oranges.", pause: .short),
            SpeechSegment(text: "Bananas.", pause: .medium),
        ])
    }

    @Test func orderedListPrefixes() {
        let md = (1...11).map { "\($0). Item \($0)" }.joined(separator: "\n")
        let result = sanitizer.sanitize(md)
        #expect(result.count == 11)
        #expect(result[0].text == "First, Item 1.")
        #expect(result[1].text == "Second, Item 2.")
        #expect(result[9].text == "Tenth, Item 10.")
        #expect(result[10].text == "Number 11, Item 11.")
        #expect(result[10].pause == .medium)
        #expect(result[0].pause == .short)
    }

    @Test func taskItemsPrefixes() {
        let result = sanitizer.sanitize("- [x] Ship it\n- [ ] Write docs")
        #expect(result.count == 2)
        #expect(result[0].text == "Done: Ship it.")
        #expect(result[1].text == "To do: Write docs.")
    }

    @Test func nestedListsFlattenInOrder() {
        let md = "- Item 1\n  - Nested A\n  - Nested B\n- Item 2"
        let result = sanitizer.sanitize(md)
        let texts = result.map(\.text)
        #expect(texts == ["Item 1.", "Nested A.", "Nested B.", "Item 2."])
    }

    @Test func listItemMultipleParagraphsJoin() {
        let md = "- First para.\n\n  Second para."
        let result = sanitizer.sanitize(md)
        #expect(result.count == 1)
        #expect(result[0].text == "First para. Second para.")
    }

    @Test func blockquoteUsesNormalRules() {
        let result = sanitizer.sanitize("> Quoted **bold** text.")
        #expect(result == [SpeechSegment(text: "Quoted bold text.", pause: .medium)])
    }

    @Test func codeBlockAnnounceDefault() {
        let result = sanitizer.sanitize("```\nlet x = 1\n```")
        #expect(result == [SpeechSegment(text: "Here's a code snippet.", pause: .medium)])
    }

    @Test func codeBlockLanguageMapping() {
        let cases: [(String, String)] = [
            ("py", "Here's a Python code snippet."),
            ("python", "Here's a Python code snippet."),
            ("js", "Here's a JavaScript code snippet."),
            ("ts", "Here's a TypeScript code snippet."),
            ("sh", "Here's a shell code snippet."),
            ("swift", "Here's a Swift code snippet."),
            ("json", "Here's a JSON code snippet."),
            ("yaml", "Here's a YAML code snippet."),
            ("sql", "Here's a SQL code snippet."),
            ("rust", "Here's a rust code snippet."),
            ("mermaid", "Here's a diagram."),
        ]
        for (lang, expected) in cases {
            let result = sanitizer.sanitize("```\(lang)\ncode\n```")
            #expect(result.count == 1, "\(lang)")
            #expect(result.first?.text == expected, "\(lang)")
        }
    }

    @Test func codeBlockReadFullAndSkip() {
        let s = SpeechSanitizer(options: SanitizerOptions(codeBlocks: .readFull))
        let full = s.sanitize("```\nline one\n\nline two\n```")
        #expect(full == [
            SpeechSegment(text: "line one.", pause: .short),
            SpeechSegment(text: "line two.", pause: .medium),
        ])
        let skip = SpeechSanitizer(options: SanitizerOptions(codeBlocks: .skip))
        #expect(skip.sanitize("```\ncode\n```") == [])
    }

    @Test func tableReadRows() {
        let md = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |"
        let result = sanitizer.sanitize(md)
        #expect(result == [
            SpeechSegment(text: "A table with 2 rows.", pause: .medium),
            SpeechSegment(text: "Alice: Age 30.", pause: .medium),
            SpeechSegment(text: "Bob: Age 25.", pause: .medium),
        ])
    }

    @Test func tableSingleRowSingular() {
        let md = "| Name | Age |\n| --- | --- |\n| Alice | 30 |"
        let result = sanitizer.sanitize(md)
        #expect(result[0].text == "A table with 1 row.")
    }

    @Test func tableEmptyHeadersJoinWithCommas() {
        let md = "| | |\n| --- | --- |\n| a | b |"
        let result = sanitizer.sanitize(md)
        #expect(result.count == 2)
        #expect(result[1].text == "a, b.")
    }

    @Test func tableAnnounceAndSkipAndMaxRows() {
        let md = "| A | B | C |\n| --- | --- | --- |\n| 1 | 2 | 3 |"
        let announce = SpeechSanitizer(options: SanitizerOptions(tables: .announce))
        let a = announce.sanitize(md)
        #expect(a == [SpeechSegment(text: "A table with 1 row and 3 columns, comparing A, B and C.", pause: .medium)])
        let skip = SpeechSanitizer(options: SanitizerOptions(tables: .skip))
        #expect(skip.sanitize(md) == [])
        let limited = SpeechSanitizer(options: SanitizerOptions(tables: .readRows, maxTableRowsToRead: 0))
        let l = limited.sanitize(md)
        #expect(l.count == 1)
        #expect(l[0].text.contains("A table with 1 row and 3 columns") == true)
    }

    @Test func tableAnnounceOmitsComparingWhenHeadersEmpty() {
        let md = "| | |\n| --- | --- |\n| a | b |"
        let announce = SpeechSanitizer(options: SanitizerOptions(tables: .announce))
        let result = announce.sanitize(md)
        #expect(result[0].text == "A table with 1 row and 2 columns.")
    }

    @Test func thematicBreakSetsPreviousPauseLong() {
        let result = sanitizer.sanitize("First para.\n\n---\n\nSecond para.")
        #expect(result.count == 2)
        #expect(result[0] == SpeechSegment(text: "First para.", pause: .long))
        #expect(result[1] == SpeechSegment(text: "Second para.", pause: .medium))
    }

    @Test func htmlBlockAndInline() {
        let block = sanitizer.sanitize("<div>Hello<br>world</div>")
        #expect(block.count == 1)
        #expect(block[0].text == "Hello world.")
        let inline = sanitizer.sanitize("a <b>bold</b> c")
        #expect(inline[0].text == "a bold c.")
        let empty = sanitizer.sanitize("<div></div>")
        #expect(empty == [])
    }

    @Test func footnotesAndCitationsRemoved() {
        #expect(sanitizer.sanitize("Text[^1] more.")[0].text == "Text more.")
        #expect(sanitizer.sanitize("Claim [1] done.")[0].text == "Claim done.")
        #expect(sanitizer.sanitize("Claim [12] done.")[0].text == "Claim done.")
        #expect(sanitizer.sanitize("Claim [1, 2] done.")[0].text == "Claim done.")
        #expect(sanitizer.sanitize("Claim [1-3] done.")[0].text == "Claim done.")
    }

    @Test func emojiRemovedButDigitsHashKept() {
        let result = sanitizer.sanitize("Hi 👋 there 123 # done.")
        #expect(result[0].text == "Hi there 123 # done.")
    }

    @Test func persianTextKept() {
        let result = sanitizer.sanitize("سلام دنیا 👋")
        #expect(result.count == 1)
        #expect(result[0].text.contains("سلام دنیا") == true)
        #expect(result[0].text.contains("👋") == false)
    }

    @Test func wordReplacements() {
        #expect(sanitizer.sanitize("Apples e.g. oranges.")[0].text == "Apples for example oranges.")
        #expect(sanitizer.sanitize("Apples i.e. oranges.")[0].text == "Apples that is oranges.")
        #expect(sanitizer.sanitize("Apples etc. done.")[0].text == "Apples et cetera done.")
        #expect(sanitizer.sanitize("Cats vs. dogs.")[0].text == "Cats versus dogs.")
        #expect(sanitizer.sanitize("Cats vs dogs.")[0].text == "Cats versus dogs.")
        #expect(sanitizer.sanitize("A → B.")[0].text == "A to B.")
        #expect(sanitizer.sanitize("A -> B.")[0].text == "A to B.")
        #expect(sanitizer.sanitize("Fish & chips.")[0].text == "Fish and chips.")
        #expect(sanitizer.sanitize("Tea w/ sugar.")[0].text == "Tea with sugar.")
    }

    @Test func leftoverMarkupRemovedAndTerminalPunctuation() {
        #expect(sanitizer.sanitize("Hello")[0].text == "Hello.")
        #expect(sanitizer.sanitize("Hello!")[0].text == "Hello!")
        #expect(sanitizer.sanitize("**Hello**")[0].text == "Hello.")
        #expect(sanitizer.sanitize("a | b")[0].text == "a b.")
        #expect(sanitizer.sanitize("keep file_name here.")[0].text == "keep file_name here.")
    }

    @Test func realisticChatGPTAnswer() {
        let md = """
        # Morning Briefing

        Hello **world**, this is a *test* with `code` and a [link](https://example.com) plus emoji 🎉 and more.

        - Apples
        - Oranges
        - Bananas

        | Name | Age | City |
        | ---- | --- | ---- |
        | Alice | 30 | Paris |
        | Bob | 25 | Rome |
        | Cara | 28 | Madrid |

        ```python
        print("hi")
        ```

        Visit https://www.example.com/page for more.

        ---
        """
        let result = sanitizer.sanitize(md)
        let expected = [
            SpeechSegment(text: "Morning Briefing.", pause: .long),
            SpeechSegment(text: "Hello world, this is a test with code and a link plus emoji and more.", pause: .medium),
            SpeechSegment(text: "Apples.", pause: .short),
            SpeechSegment(text: "Oranges.", pause: .short),
            SpeechSegment(text: "Bananas.", pause: .medium),
            SpeechSegment(text: "A table with 3 rows.", pause: .medium),
            SpeechSegment(text: "Alice: Age 30, City Paris.", pause: .medium),
            SpeechSegment(text: "Bob: Age 25, City Rome.", pause: .medium),
            SpeechSegment(text: "Cara: Age 28, City Madrid.", pause: .medium),
            SpeechSegment(text: "Here's a Python code snippet.", pause: .medium),
            SpeechSegment(text: "Visit link to example.com for more.", pause: .long),
        ]
        #expect(result == expected)
        for segment in result {
            #expect(!segment.text.contains("#"))
            #expect(!segment.text.contains("*"))
            #expect(!segment.text.contains("|"))
            #expect(!segment.text.contains("`"))
            #expect(!segment.text.contains("http"))
        }
    }

    @Test func plainTextPassesThrough() {
        let result = sanitizer.sanitize("Just plain text")
        #expect(result == [SpeechSegment(text: "Just plain text.", pause: .medium)])
    }

    @Test func hashtagMidSentenceKept() {
        let result = sanitizer.sanitize("Trend #hashtag here.")
        #expect(result[0].text == "Trend #hashtag here.")
    }

    @Test func emptyInputReturnsEmpty() {
        #expect(sanitizer.sanitize("") == [])
        #expect(sanitizer.sanitize("   \n  ") == [])
    }

    @Test func largeDocumentUnderHalfSecond() {
        var lines: [String] = []
        for index in 0..<2000 {
            lines.append("Paragraph \(index) with **bold** and a [link](https://example.com) plus emoji 🎉.")
        }
        let md = lines.joined(separator: "\n\n")
        let start = Date()
        let result = sanitizer.sanitize(md)
        let elapsed = Date().timeIntervalSince(start)
        #expect(result.count == 2000)
        #expect(elapsed < 0.5, "took \(elapsed)s")
    }
}
