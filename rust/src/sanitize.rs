//! Markdown → speakable segments (port of `SpeechSanitizer.swift`).
//!
//! Faithful port of `Sources/SmartVoicesCore/SpeechSanitizer.swift`: same
//! segment shapes, pauses, strings and order of operations, rebuilt over
//! `pulldown-cmark`'s event stream instead of swift-markdown's tree.

use std::mem::discriminant;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// Pause after a spoken segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechPause {
    None,
    Short,
    Medium,
    Long,
}

impl SpeechPause {
    /// Pause duration in seconds.
    #[must_use]
    pub fn seconds(self) -> f64 {
        match self {
            SpeechPause::None => 0.0,
            SpeechPause::Short => 0.15,
            SpeechPause::Medium => 0.35,
            SpeechPause::Long => 0.6,
        }
    }

    /// Lowercase label for the `clean` command (`<pause>\t<text>` lines).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            SpeechPause::None => "none",
            SpeechPause::Short => "short",
            SpeechPause::Medium => "medium",
            SpeechPause::Long => "long",
        }
    }
}

/// A speakable text segment with its trailing pause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechSegment {
    pub text: String,
    pub pause: SpeechPause,
}

/// What to do with fenced/indented code blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodeBlocks {
    /// Speak "Here's a … code snippet."
    #[default]
    Announce,
    /// Skip code blocks entirely.
    Skip,
    /// Read every non-empty line.
    ReadFull,
}

/// What to do with tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tables {
    /// Read each row as "first: Header value, …".
    #[default]
    ReadRows,
    /// Speak a one-line summary.
    Announce,
    /// Skip tables entirely.
    Skip,
}

/// What to do with bare URLs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BareUrls {
    /// Speak "link to <host>".
    #[default]
    DomainOnly,
    /// Drop bare URLs.
    Skip,
}

/// Options controlling sanitizer output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizerOptions {
    pub code_blocks: CodeBlocks,
    pub tables: Tables,
    pub bare_urls: BareUrls,
    pub max_table_rows_to_read: usize,
}

impl Default for SanitizerOptions {
    fn default() -> Self {
        Self {
            code_blocks: CodeBlocks::Announce,
            tables: Tables::ReadRows,
            bare_urls: BareUrls::DomainOnly,
            max_table_rows_to_read: 15,
        }
    }
}

/// Sanitize Markdown (or plain text) into speakable segments.
#[must_use]
pub fn sanitize(text: &str, options: &SanitizerOptions) -> Vec<SpeechSegment> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    let stripped = strip_footnote_definitions(text);
    if stripped.trim().is_empty() {
        return Vec::new();
    }
    let events: Vec<Event> = Parser::new_ext(&stripped, parser_options()).collect();
    let mut pos = 0;
    let blocks = parse_blocks(&events, &mut pos, None, options);
    let mut segments = Vec::new();
    append_blocks(&blocks, options, &mut segments);
    segments
}

/// Parser options: tables, strikethrough, task lists. No footnotes or smart
/// punctuation: footnote definitions and bare URLs are handled by hand,
/// like the Swift original.
fn parser_options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS
}

// MARK: - Block tree

/// Block structure rebuilt from pulldown's event stream (mirrors the
/// swift-markdown tree the original walks). Inline markup is already
/// reduced to raw text; cleaning happens in the segment phase.
enum Block {
    Heading(String),
    Paragraph(String),
    List {
        ordered: bool,
        start: u64,
        items: Vec<ListItem>,
    },
    Quote(Vec<Block>),
    Code {
        language: String,
        code: String,
    },
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
        columns: usize,
    },
    Html(String),
    Rule,
}

struct ListItem {
    paragraphs: Vec<String>,
    nested: Vec<Block>,
    checkbox: Option<bool>,
}

/// True when `event` closes a tag of the same kind as `end`
/// (payloads such as heading level are ignored).
fn is_end_for(event: &Event, end: &TagEnd) -> bool {
    matches!(event, Event::End(actual) if discriminant(actual) == discriminant(end))
}

/// Parse blocks until `end` (or input exhaustion when `None`).
fn parse_blocks(
    events: &[Event],
    pos: &mut usize,
    end: Option<TagEnd>,
    options: &SanitizerOptions,
) -> Vec<Block> {
    let mut blocks = Vec::new();
    while *pos < events.len() {
        if let Some(end) = &end
            && is_end_for(&events[*pos], end)
        {
            *pos += 1;
            break;
        }
        parse_block(events, pos, options, &mut blocks);
    }
    blocks
}

fn parse_block(
    events: &[Event],
    pos: &mut usize,
    options: &SanitizerOptions,
    out: &mut Vec<Block>,
) {
    if *pos >= events.len() {
        return;
    }
    match &events[*pos] {
        Event::Start(Tag::Paragraph) => {
            *pos += 1;
            let raw = collect_inline(events, pos, &TagEnd::Paragraph, options);
            out.push(Block::Paragraph(raw));
        }
        Event::Start(Tag::Heading { .. }) => {
            *pos += 1;
            let raw = collect_inline(events, pos, &TagEnd::Heading(HeadingLevel::H1), options);
            out.push(Block::Heading(raw));
        }
        Event::Start(Tag::BlockQuote(_)) => {
            *pos += 1;
            let children = parse_blocks(events, pos, Some(TagEnd::BlockQuote(None)), options);
            out.push(Block::Quote(children));
        }
        Event::Start(Tag::CodeBlock(kind)) => {
            let language = match kind {
                CodeBlockKind::Indented => String::new(),
                CodeBlockKind::Fenced(info) => info.to_string(),
            };
            *pos += 1;
            let mut code = String::new();
            while *pos < events.len() {
                if is_end_for(&events[*pos], &TagEnd::CodeBlock) {
                    *pos += 1;
                    break;
                }
                if let Event::Text(text) = &events[*pos] {
                    code.push_str(text);
                }
                *pos += 1;
            }
            out.push(Block::Code { language, code });
        }
        Event::Start(Tag::HtmlBlock) => {
            *pos += 1;
            let mut html = String::new();
            while *pos < events.len() {
                if is_end_for(&events[*pos], &TagEnd::HtmlBlock) {
                    *pos += 1;
                    break;
                }
                match &events[*pos] {
                    Event::Html(text)
                    | Event::InlineHtml(text)
                    | Event::Text(text)
                    | Event::Code(text) => html.push_str(text),
                    _ => {}
                }
                *pos += 1;
            }
            out.push(Block::Html(html));
        }
        Event::Start(Tag::List(_)) => parse_list(events, pos, options, out),
        Event::Start(Tag::Table(_)) => parse_table(events, pos, options, out),
        Event::Start(Tag::Item) => {
            // Stray item outside a list: handle gracefully like the Swift
            // fallback (recurse into children as plain blocks).
            *pos += 1;
            let item = parse_item(events, pos, options);
            for paragraph in item.paragraphs {
                out.push(Block::Paragraph(paragraph));
            }
            out.extend(item.nested);
        }
        Event::Start(_) => {
            // Anything else (metadata, footnotes, …): skip the subtree.
            skip_balanced(events, pos);
        }
        Event::Rule => {
            out.push(Block::Rule);
            *pos += 1;
        }
        _ => {
            *pos += 1;
        }
    }
}

fn parse_list(events: &[Event], pos: &mut usize, options: &SanitizerOptions, out: &mut Vec<Block>) {
    let (ordered, start) = match &events[*pos] {
        Event::Start(Tag::List(number)) => (number.is_some(), number.unwrap_or(1)),
        _ => return,
    };
    *pos += 1;
    let mut items = Vec::new();
    let end = TagEnd::List(true);
    while *pos < events.len() {
        if is_end_for(&events[*pos], &end) {
            *pos += 1;
            break;
        }
        if matches!(&events[*pos], Event::Start(Tag::Item)) {
            *pos += 1;
            items.push(parse_item(events, pos, options));
        } else {
            *pos += 1;
        }
    }
    out.push(Block::List {
        ordered,
        start,
        items,
    });
}

fn parse_item(events: &[Event], pos: &mut usize, options: &SanitizerOptions) -> ListItem {
    let mut item = ListItem {
        paragraphs: Vec::new(),
        nested: Vec::new(),
        checkbox: None,
    };
    while *pos < events.len() {
        match &events[*pos] {
            Event::End(TagEnd::Item) => {
                *pos += 1;
                break;
            }
            Event::TaskListMarker(checked) => {
                item.checkbox = Some(*checked);
                *pos += 1;
            }
            Event::Start(Tag::Paragraph) => {
                *pos += 1;
                item.paragraphs
                    .push(collect_inline(events, pos, &TagEnd::Paragraph, options));
            }
            Event::Start(Tag::List(_)) => parse_list(events, pos, options, &mut item.nested),
            Event::Start(_) => parse_block(events, pos, options, &mut item.nested),
            Event::End(_) => {
                *pos += 1;
            }
            _ => {
                if ends_item_inline(&events[*pos]) {
                    // Stray (e.g. a rule directly under an item): skip.
                    *pos += 1;
                } else {
                    // Tight items hold inline content directly, with no
                    // `Paragraph` wrapper.
                    item.paragraphs
                        .push(collect_inline_until(events, pos, None, options));
                }
            }
        }
    }
    item
}

/// True for events that end item-level inline content (block starts,
/// block ends, task markers, rules). Span tags and link/image tags are
/// inline and don't end it.
fn ends_item_inline(event: &Event) -> bool {
    match event {
        Event::Start(Tag::Link { .. })
        | Event::Start(Tag::Image { .. })
        | Event::Start(Tag::Emphasis)
        | Event::Start(Tag::Strong)
        | Event::Start(Tag::Strikethrough)
        | Event::Start(Tag::Superscript)
        | Event::Start(Tag::Subscript) => false,
        Event::Start(_) => true,
        Event::End(end) => !matches!(
            end,
            TagEnd::Emphasis
                | TagEnd::Strong
                | TagEnd::Strikethrough
                | TagEnd::Superscript
                | TagEnd::Subscript
                | TagEnd::Link
                | TagEnd::Image
        ),
        Event::TaskListMarker(_) | Event::Rule => true,
        _ => false,
    }
}

#[allow(clippy::too_many_lines)]
fn parse_table(
    events: &[Event],
    pos: &mut usize,
    options: &SanitizerOptions,
    out: &mut Vec<Block>,
) {
    let mut columns = match &events[*pos] {
        Event::Start(Tag::Table(alignments)) => alignments.len(),
        _ => return,
    };
    *pos += 1;
    let mut headers = Vec::new();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut current_row: Option<Vec<String>> = None;
    let mut in_head = false;
    while *pos < events.len() {
        match &events[*pos] {
            Event::End(TagEnd::Table) => {
                *pos += 1;
                break;
            }
            Event::Start(Tag::TableHead) => {
                in_head = true;
                *pos += 1;
            }
            Event::End(TagEnd::TableHead) => {
                in_head = false;
                *pos += 1;
            }
            Event::Start(Tag::TableRow) => {
                if !in_head {
                    current_row = Some(Vec::new());
                }
                *pos += 1;
            }
            Event::End(TagEnd::TableRow) => {
                if let Some(row) = current_row.take() {
                    rows.push(row);
                }
                *pos += 1;
            }
            Event::Start(Tag::TableCell) => {
                *pos += 1;
                let text = collect_inline(events, pos, &TagEnd::TableCell, options);
                if in_head {
                    headers.push(text);
                } else if let Some(row) = current_row.as_mut() {
                    row.push(text);
                } else {
                    current_row = Some(vec![text]);
                }
            }
            _ => {
                *pos += 1;
            }
        }
    }
    if let Some(row) = current_row.take()
        && !row.is_empty()
    {
        rows.push(row);
    }
    columns = columns.max(headers.len());
    for row in &rows {
        columns = columns.max(row.len());
    }
    out.push(Block::Table {
        headers,
        rows,
        columns,
    });
}

/// Consume events from an opening `Start` (inclusive) through its
/// matching `End` (used for images: alt text is dropped, like Swift).
fn skip_balanced(events: &[Event], pos: &mut usize) {
    let mut depth: u32 = 0;
    while *pos < events.len() {
        let is_start = matches!(&events[*pos], Event::Start(_));
        let is_end = matches!(&events[*pos], Event::End(_));
        *pos += 1;
        if is_start {
            depth += 1;
        } else if is_end {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                break;
            }
        }
    }
}

// MARK: - Inline text

/// Collect raw inline text until `end`, reducing links (autolinks whose
/// text equals the destination become "link to …"), dropping images and
/// inline HTML (except `<br>`, which becomes a space) and turning line
/// breaks into spaces — mirroring Swift's `inlineText`.
fn collect_inline(
    events: &[Event],
    pos: &mut usize,
    end: &TagEnd,
    options: &SanitizerOptions,
) -> String {
    collect_inline_until(events, pos, Some(end), options)
}

/// Collect raw inline text until `end` (`Some`), or until a block
/// boundary (`None`, for tight list items with no `Paragraph` wrapper).
fn collect_inline_until(
    events: &[Event],
    pos: &mut usize,
    end: Option<&TagEnd>,
    options: &SanitizerOptions,
) -> String {
    // `buffers[0]` is the base text; each open link pushes a fresh buffer
    // plus its destination so the inner text can be compared on close.
    let mut buffers: Vec<String> = vec![String::new()];
    let mut destinations: Vec<String> = Vec::new();
    while *pos < events.len() {
        if let Some(end) = end {
            if is_end_for(&events[*pos], end) {
                *pos += 1;
                break;
            }
        } else if ends_item_inline(&events[*pos]) {
            break;
        }
        match &events[*pos] {
            Event::Text(text) | Event::Code(text) => {
                if let Some(top) = buffers.last_mut() {
                    top.push_str(text);
                }
                *pos += 1;
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(top) = buffers.last_mut() {
                    top.push(' ');
                }
                *pos += 1;
            }
            Event::InlineHtml(html) | Event::Html(html) => {
                if is_break_tag(html)
                    && let Some(top) = buffers.last_mut()
                {
                    top.push(' ');
                }
                *pos += 1;
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                destinations.push(dest_url.to_string());
                buffers.push(String::new());
                *pos += 1;
            }
            Event::End(TagEnd::Link) => {
                *pos += 1;
                if let (Some(inner), Some(dest)) = (buffers.pop(), destinations.pop()) {
                    let spoken = if inner == dest && is_http_url(&dest) {
                        bare_url_spoken_string(&dest, options)
                    } else {
                        inner
                    };
                    if let Some(top) = buffers.last_mut() {
                        top.push_str(&spoken);
                    }
                }
            }
            Event::Start(Tag::Image { .. }) => skip_balanced(events, pos),
            Event::Start(_) | Event::End(_) => {
                // Transparent span tags (emphasis, strong, …).
                *pos += 1;
            }
            _ => {
                *pos += 1;
            }
        }
    }
    buffers.concat()
}

// MARK: - Blocks

fn append_blocks(blocks: &[Block], options: &SanitizerOptions, segments: &mut Vec<SpeechSegment>) {
    for block in blocks {
        append_block(block, options, segments);
    }
}

fn append_block(block: &Block, options: &SanitizerOptions, segments: &mut Vec<SpeechSegment>) {
    match block {
        Block::Heading(raw) => {
            if let Some(cleaned) = clean_text(raw, options) {
                segments.push(SpeechSegment {
                    text: cleaned,
                    pause: SpeechPause::Long,
                });
            }
        }
        Block::Paragraph(raw) => {
            if let Some(cleaned) = clean_text(raw, options) {
                segments.push(SpeechSegment {
                    text: cleaned,
                    pause: SpeechPause::Medium,
                });
            }
        }
        Block::List {
            ordered,
            start,
            items,
        } => segments.extend(list_segments(*ordered, *start, items, options)),
        Block::Quote(children) => append_blocks(children, options, segments),
        Block::Code { language, code } => {
            segments.extend(code_segments(language, code, options));
        }
        Block::Table {
            headers,
            rows,
            columns,
        } => segments.extend(table_segments(headers, rows, *columns, options)),
        Block::Html(raw) => {
            let stripped = strip_html(raw);
            if let Some(cleaned) = clean_text(&stripped, options) {
                segments.push(SpeechSegment {
                    text: cleaned,
                    pause: SpeechPause::Medium,
                });
            }
        }
        Block::Rule => {
            if let Some(last) = segments.last_mut() {
                last.pause = SpeechPause::Long;
            }
        }
    }
}

// MARK: - Lists

fn list_segments(
    ordered: bool,
    start: u64,
    items: &[ListItem],
    options: &SanitizerOptions,
) -> Vec<SpeechSegment> {
    let mut owned: Vec<Option<SpeechSegment>> = Vec::with_capacity(items.len());
    let mut nested: Vec<Vec<SpeechSegment>> = Vec::with_capacity(items.len());
    for (offset, item) in items.iter().enumerate() {
        let number = start.saturating_add(offset as u64);
        owned.push(list_item_segment(item, ordered, number, options));
        nested.push(list_item_nested_segments(item, options));
    }
    // Last owned segment of this list gets .medium, others .short.
    let mut last_owned: Option<usize> = None;
    for (index, own) in owned.iter().enumerate() {
        if own.is_some() {
            last_owned = Some(index);
        }
    }
    let mut result = Vec::new();
    for (index, (own, nest)) in owned.into_iter().zip(nested).enumerate() {
        if let Some(mut own) = own {
            own.pause = if Some(index) == last_owned {
                SpeechPause::Medium
            } else {
                SpeechPause::Short
            };
            result.push(own);
        }
        result.extend(nest);
    }
    result
}

fn list_item_segment(
    item: &ListItem,
    ordered: bool,
    number: u64,
    options: &SanitizerOptions,
) -> Option<SpeechSegment> {
    let joined = item.paragraphs.join(" ");
    let cleaned = clean_fragment(&joined, options)?;
    if cleaned.is_empty() {
        return None;
    }
    let mut prefix = String::new();
    if let Some(checked) = item.checkbox {
        prefix.push_str(if checked { "Done: " } else { "To do: " });
    }
    let mut item_text = cleaned;
    if ordered {
        prefix.push_str(&ordinal_prefix(number));
        item_text = decapitalize_after_ordinal(&item_text);
    }
    Some(SpeechSegment {
        text: with_terminal_punctuation(&(prefix + &item_text)),
        pause: SpeechPause::Short,
    })
}

fn list_item_nested_segments(item: &ListItem, options: &SanitizerOptions) -> Vec<SpeechSegment> {
    let mut result = Vec::new();
    append_blocks(&item.nested, options, &mut result);
    result
}

fn ordinal_prefix(number: u64) -> String {
    match number {
        1 => "First, ".to_string(),
        2 => "Second, ".to_string(),
        3 => "Third, ".to_string(),
        4 => "Fourth, ".to_string(),
        5 => "Fifth, ".to_string(),
        6 => "Sixth, ".to_string(),
        7 => "Seventh, ".to_string(),
        8 => "Eighth, ".to_string(),
        9 => "Ninth, ".to_string(),
        10 => "Tenth, ".to_string(),
        _ => format!("Number {number}, "),
    }
}

fn decapitalize_after_ordinal(text: &str) -> String {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return text.to_string();
    };
    if !first.is_uppercase() {
        return text.to_string();
    }
    if let Some(second) = chars.next()
        && second.is_uppercase()
    {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    for lower in first.to_lowercase() {
        out.push(lower);
    }
    out.push_str(&text[first.len_utf8()..]);
    out
}

// MARK: - Code blocks

fn code_segments(language: &str, code: &str, options: &SanitizerOptions) -> Vec<SpeechSegment> {
    match options.code_blocks {
        CodeBlocks::Skip => Vec::new(),
        CodeBlocks::Announce => vec![SpeechSegment {
            text: code_announcement(language),
            pause: SpeechPause::Medium,
        }],
        CodeBlocks::ReadFull => {
            let mut result = Vec::new();
            for line in code.split('\n') {
                if let Some(cleaned) = clean_text(line, options) {
                    result.push(SpeechSegment {
                        text: cleaned,
                        pause: SpeechPause::Short,
                    });
                }
            }
            if let Some(last) = result.last_mut() {
                last.pause = SpeechPause::Medium;
            }
            result
        }
    }
}

fn code_announcement(language: &str) -> String {
    let info = language.trim();
    if info.eq_ignore_ascii_case("mermaid") {
        return "Here's a diagram.".to_string();
    }
    if info.is_empty() {
        return "Here's a code snippet.".to_string();
    }
    let display = match info.to_lowercase().as_str() {
        "py" | "python" => "Python",
        "js" | "javascript" => "JavaScript",
        "ts" | "typescript" => "TypeScript",
        "sh" | "bash" | "zsh" | "shell" => "shell",
        "swift" => "Swift",
        "json" => "JSON",
        "yaml" | "yml" => "YAML",
        "sql" => "SQL",
        _ => info,
    };
    format!("Here's a {display} code snippet.")
}

// MARK: - Tables

fn table_segments(
    headers: &[String],
    rows: &[Vec<String>],
    columns: usize,
    options: &SanitizerOptions,
) -> Vec<SpeechSegment> {
    match options.tables {
        Tables::Skip => Vec::new(),
        Tables::Announce => vec![table_summary(headers, rows.len(), columns, options)],
        Tables::ReadRows => {
            if rows.len() > options.max_table_rows_to_read {
                return vec![table_summary(headers, rows.len(), columns, options)];
            }
            let mut result = vec![SpeechSegment {
                text: table_intro(rows.len()),
                pause: SpeechPause::Medium,
            }];
            let cleaned_headers = header_fragments(headers, options);
            let all_headers_empty = cleaned_headers.iter().all(Option::is_none);
            for row in rows {
                let cells: Vec<Option<String>> = row
                    .iter()
                    .map(|cell| clean_fragment(cell, options))
                    .collect();
                if let Some(text) = table_row_text(&cells, &cleaned_headers, all_headers_empty) {
                    result.push(SpeechSegment {
                        text,
                        pause: SpeechPause::Medium,
                    });
                }
            }
            result
        }
    }
}

fn header_fragments(headers: &[String], options: &SanitizerOptions) -> Vec<Option<String>> {
    headers
        .iter()
        .map(|header| match clean_fragment(header, options) {
            Some(value) if !value.is_empty() => Some(value),
            _ => None,
        })
        .collect()
}

fn table_intro(row_count: usize) -> String {
    if row_count == 1 {
        "A table with 1 row.".to_string()
    } else {
        format!("A table with {row_count} rows.")
    }
}

fn table_summary(
    headers: &[String],
    row_count: usize,
    columns: usize,
    options: &SanitizerOptions,
) -> SpeechSegment {
    let row_word = if row_count == 1 {
        "1 row".to_string()
    } else {
        format!("{row_count} rows")
    };
    let col_word = if columns == 1 {
        "1 column".to_string()
    } else {
        format!("{columns} columns")
    };
    let mut text = format!("A table with {row_word} and {col_word}");
    let names: Vec<String> = header_fragments(headers, options)
        .into_iter()
        .flatten()
        .filter(|name| !name.is_empty())
        .collect();
    if !names.is_empty() {
        text.push_str(", comparing ");
        text.push_str(&and_list(&names));
    }
    text.push('.');
    SpeechSegment {
        text,
        pause: SpeechPause::Medium,
    }
}

fn and_list(names: &[String]) -> String {
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        2 => format!("{} and {}", names[0], names[1]),
        _ => {
            let head = names[..names.len() - 1].join(", ");
            format!("{head} and {}", names[names.len() - 1])
        }
    }
}

fn table_row_text(
    cells: &[Option<String>],
    headers: &[Option<String>],
    all_headers_empty: bool,
) -> Option<String> {
    let cleaned_cells: Vec<&str> = cells
        .iter()
        .map(|cell| cell.as_deref().unwrap_or("").trim())
        .collect();
    let cleaned_headers: Vec<&str> = headers
        .iter()
        .map(|header| header.as_deref().unwrap_or("").trim())
        .collect();
    let non_empty: Vec<&str> = cleaned_cells
        .iter()
        .filter(|cell| !cell.is_empty())
        .copied()
        .collect();
    if non_empty.is_empty() {
        return None;
    }
    if all_headers_empty {
        return Some(with_terminal_punctuation(&non_empty.join(", ")));
    }
    // First cell is the subject; remaining pair with headers.
    let first = cleaned_cells.first().copied().unwrap_or("");
    let mut parts: Vec<String> = Vec::new();
    for (index, cell) in cleaned_cells.iter().enumerate().skip(1) {
        if cell.is_empty() {
            continue;
        }
        let header = cleaned_headers.get(index).copied().unwrap_or("");
        if header.is_empty() {
            parts.push((*cell).to_string());
        } else {
            parts.push(format!("{header} {cell}"));
        }
    }
    if first.is_empty() {
        return Some(with_terminal_punctuation(&parts.join(", ")));
    }
    if parts.is_empty() {
        return Some(with_terminal_punctuation(first));
    }
    Some(with_terminal_punctuation(&format!(
        "{first}: {}",
        parts.join(", ")
    )))
}

// MARK: - Cleaning

fn clean_text(raw: &str, options: &SanitizerOptions) -> Option<String> {
    let fragment = clean_fragment(raw, options)?;
    if fragment.is_empty() {
        return None;
    }
    Some(with_terminal_punctuation(&fragment))
}

fn clean_fragment(raw: &str, options: &SanitizerOptions) -> Option<String> {
    let mut value = remove_citations(raw);
    value = remove_space_before_punctuation(&value);
    value = remove_emoji(&value);
    value = replace_bare_urls(&value, options);
    value = apply_word_replacements(&value);
    value = collapse_whitespace(&value);
    value = remove_leftover_markup(&value);
    value = collapse_whitespace(&value);
    let trimmed = value.trim().to_string();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed)
}

fn with_terminal_punctuation(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Some(last) = trimmed.chars().last()
        && matches!(last, '.' | '!' | '?' | ':' | ';' | '…')
    {
        return trimmed.to_string();
    }
    format!("{trimmed}.")
}

fn collapse_whitespace(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut in_space = false;
    for c in value.chars() {
        if c.is_whitespace() {
            if !in_space {
                result.push(' ');
                in_space = true;
            }
        } else {
            result.push(c);
            in_space = false;
        }
    }
    result.trim().to_string()
}

// MARK: Citations

fn remove_citations(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut result = String::with_capacity(value.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '['
            && let Some(distance) = chars[index..].iter().position(|&c| c == ']')
        {
            // Swift bounds the scan to 60 chars (its `limitedBy:` makes
            // the bound vacuous when fewer than 60 chars remain).
            let remaining = chars.len() - index;
            if distance < 60 || remaining < 60 {
                let inside: String = chars[index + 1..index + distance].iter().collect();
                if is_footnote_ref(&inside) || is_numeric_citation(&inside) {
                    index += distance + 1;
                    continue;
                }
            }
        }
        result.push(chars[index]);
        index += 1;
    }
    result
}

fn remove_space_before_punctuation(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut result = String::with_capacity(value.len());
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if c == ' ' || c == '\t' {
            let mut end = index + 1;
            while end < chars.len() && (chars[end] == ' ' || chars[end] == '\t') {
                end += 1;
            }
            if end < chars.len() && matches!(chars[end], '.' | ',' | ';' | ':' | '!' | '?') {
                index = end;
                continue;
            }
            for &space in &chars[index..end] {
                result.push(space);
            }
            index = end;
        } else {
            result.push(c);
            index += 1;
        }
    }
    result
}

fn is_footnote_ref(inside: &str) -> bool {
    inside.starts_with('^') && inside.chars().count() > 1
}

fn is_numeric_citation(inside: &str) -> bool {
    let trimmed = inside.trim();
    if trimmed.is_empty() {
        return false;
    }
    let mut has_digit = false;
    for c in trimmed.chars() {
        if c.is_numeric() {
            has_digit = true;
            continue;
        }
        // Swift scans `decimalDigits` here and `isNumber` for the ends;
        // `char::is_numeric` covers both (plus exotic number letters).
        if matches!(c, ',' | '-' | '–' | '—') || c.is_whitespace() {
            continue;
        }
        return false;
    }
    if !has_digit {
        return false;
    }
    let first = trimmed.chars().next();
    let last = trimmed.chars().last();
    matches!((first, last), (Some(f), Some(l)) if f.is_numeric() && l.is_numeric())
}

// MARK: Emoji

fn remove_emoji(value: &str) -> String {
    value
        .chars()
        .filter(|c| !is_emoji_presentation(*c))
        .collect()
}

/// True for scalars with Unicode `Emoji_Presentation=Yes` (UTS #51).
///
/// Hand-rolled table (no Unicode-data crate allowed). It approximates the
/// Swift original, which works per grapheme cluster: lone text-default
/// symbols in emoji blocks (e.g. ★, ♠) and keycap/ZWJ leftovers may differ,
/// but plain emoji, digits, `#` and all scripts behave the same.
fn is_emoji_presentation(c: char) -> bool {
    matches!(c,
        '\u{A9}' | '\u{AE}' | '\u{203C}' | '\u{2049}' | '\u{2122}' | '\u{2139}'
        | '\u{2194}'..='\u{2199}' | '\u{21A9}'..='\u{21AA}'
        | '\u{231A}'..='\u{231B}' | '\u{2328}' | '\u{2388}'
        | '\u{23CF}' | '\u{23E9}'..='\u{23F3}' | '\u{23F8}'..='\u{23FA}'
        | '\u{24C2}'
        | '\u{25AA}'..='\u{25AB}' | '\u{25B6}' | '\u{25C0}' | '\u{25FB}'..='\u{25FE}'
        | '\u{2600}'..='\u{27BF}'
        | '\u{2B1B}'..='\u{2B1C}' | '\u{2B50}' | '\u{2B55}'
        | '\u{3030}' | '\u{303D}' | '\u{3297}' | '\u{3299}'
        | '\u{1F000}'..='\u{1FAFF}')
}

// MARK: Bare URLs

fn is_http_url(value: &str) -> bool {
    let lower = value.to_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

fn bare_url_spoken_string(url: &str, options: &SanitizerOptions) -> String {
    match options.bare_urls {
        BareUrls::Skip => String::new(),
        BareUrls::DomainOnly => match host_for_url(url) {
            Some(host) if !host.is_empty() => format!("link to {host}"),
            _ => String::new(),
        },
    }
}

fn host_for_url(url: &str) -> Option<String> {
    let trimmed = url.trim();
    // Manual parse (Swift prefers `URL.host`, then this same fallback).
    let mut rest = match trimmed.find("://") {
        Some(index) => &trimmed[index + 3..],
        None => trimmed,
    };
    for separator in ['/', '?', '#'] {
        if let Some(end) = rest.find(separator) {
            rest = &rest[..end];
        }
    }
    if let Some(at) = rest.rfind('@') {
        rest = &rest[at + 1..];
    }
    if let Some(colon) = rest.find(':') {
        rest = &rest[..colon];
    }
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    Some(strip_www(rest))
}

fn strip_www(host: &str) -> String {
    if host.to_lowercase().starts_with("www.") {
        // "www." is ASCII: byte and char boundaries coincide.
        host[4..].to_string()
    } else {
        host.to_string()
    }
}

fn replace_bare_urls(value: &str, options: &SanitizerOptions) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut result = String::with_capacity(value.len());
    let mut index = 0;
    while index < chars.len() {
        if (chars[index] == 'h' || chars[index] == 'H') && match_url_scheme(&chars, index) {
            let mut end = index;
            while end < chars.len() && !is_url_terminator(chars[end]) {
                end += 1;
            }
            let mut url: Vec<char> = chars[index..end].to_vec();
            // Trim trailing sentence punctuation, keep it in output.
            let mut trailing = String::new();
            while let Some(&last) = url.last() {
                if matches!(
                    last,
                    '.' | ','
                        | ';'
                        | ':'
                        | '!'
                        | '?'
                        | '('
                        | ')'
                        | '['
                        | ']'
                        | '{'
                        | '}'
                        | '"'
                        | '\''
                ) {
                    trailing.insert(0, last);
                    url.pop();
                } else {
                    break;
                }
            }
            let url_string: String = url.iter().collect();
            result.push_str(&bare_url_spoken_string(&url_string, options));
            result.push_str(&trailing);
            index = end;
        } else {
            result.push(chars[index]);
            index += 1;
        }
    }
    result
}

fn match_url_scheme(chars: &[char], index: usize) -> bool {
    let head: String = chars[index..]
        .iter()
        .take(8)
        .collect::<String>()
        .to_lowercase();
    if !head.starts_with("https://") && !head.starts_with("http://") {
        return false;
    }
    // Require a word boundary before the scheme.
    if index > 0 {
        let previous = chars[index - 1];
        if previous.is_alphabetic() || previous.is_numeric() {
            return false;
        }
    }
    true
}

fn is_url_terminator(c: char) -> bool {
    if c.is_whitespace() {
        return true;
    }
    matches!(c, '<' | '>' | '"' | '`' | '|')
}

// MARK: Word replacements

fn apply_word_replacements(value: &str) -> String {
    let mut result = replace_word(value, "e.g.", "for example");
    result = replace_word(&result, "i.e.", "that is");
    result = replace_word(&result, "etc.", "et cetera");
    result = replace_word(&result, "vs.", "versus");
    result = replace_word(&result, "vs", "versus");
    result = replace_word(&result, "w/", "with");
    result = replace_tilde_before_digit(&result);
    result = result.replace("->", " to ");
    result = result.replace('→', " to ");
    result = result.replace('&', " and ");
    result
}

fn replace_tilde_before_digit(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut result = String::with_capacity(value.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '~' && index + 1 < chars.len() && chars[index + 1].is_numeric() {
            result.push_str("about ");
            index += 1;
            continue;
        }
        result.push(chars[index]);
        index += 1;
    }
    result
}

fn replace_word(value: &str, target: &str, replacement: &str) -> String {
    if value.is_empty() || target.is_empty() {
        return value.to_string();
    }
    // Targets are ASCII, so ASCII case-folding matches Swift's lowering here.
    let target: Vec<char> = target.chars().collect();
    let chars: Vec<char> = value.chars().collect();
    let mut result = String::with_capacity(value.len());
    let mut index = 0;
    while index < chars.len() {
        if chars.len() - index >= target.len()
            && chars[index..index + target.len()]
                .iter()
                .zip(target.iter())
                .all(|(&c, &t)| c.eq_ignore_ascii_case(&t))
            && is_word_boundary_before(&chars, index)
            && is_word_boundary_after(&chars, index + target.len())
        {
            result.push_str(replacement);
            index += target.len();
        } else {
            result.push(chars[index]);
            index += 1;
        }
    }
    result
}

fn is_word_boundary_before(chars: &[char], index: usize) -> bool {
    if index == 0 {
        return true;
    }
    let previous = chars[index - 1];
    !previous.is_alphabetic() && !previous.is_numeric()
}

fn is_word_boundary_after(chars: &[char], index: usize) -> bool {
    if index >= chars.len() {
        return true;
    }
    let next = chars[index];
    !next.is_alphabetic() && !next.is_numeric()
}

// MARK: Leftover markup

fn remove_leftover_markup(value: &str) -> String {
    let result = remove_star_runs(value);
    let result = result.replace('`', "");
    let result = result.replace("~~", "");
    let result = result.replace('|', " ");
    strip_leading_markers(&result)
}

fn remove_star_runs(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut result = String::with_capacity(value.len());
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if c == '*' || c == '_' {
            let mut end = index;
            while end < chars.len() && chars[end] == c {
                end += 1;
            }
            let before = if index > 0 {
                Some(chars[index - 1])
            } else {
                None
            };
            let after = chars.get(end).copied();
            let before_is_word = before.is_some_and(|b| b.is_alphabetic() || b.is_numeric());
            let after_is_word = after.is_some_and(|a| a.is_alphabetic() || a.is_numeric());
            if before_is_word && after_is_word {
                for &kept in &chars[index..end] {
                    result.push(kept);
                }
            }
            index = end;
        } else {
            result.push(c);
            index += 1;
        }
    }
    result
}

fn strip_leading_markers(value: &str) -> String {
    let mut result = value.trim().to_string();
    loop {
        if let Some(rest) = result.strip_prefix("> ") {
            result = rest.trim().to_string();
            continue;
        }
        if result.starts_with('#') {
            let count = result.chars().take_while(|&c| c == '#').count();
            if (1..=6).contains(&count) {
                let rest: String = result.chars().skip(count).collect();
                if let Some(after) = rest.strip_prefix(' ') {
                    result = after.trim().to_string();
                    continue;
                }
            }
        }
        break;
    }
    result
}

// MARK: - HTML

fn is_break_tag(html: &str) -> bool {
    let trimmed = html.trim().to_lowercase();
    if !trimmed.starts_with('<') || !trimmed.ends_with('>') {
        return false;
    }
    // Anchored on ASCII `<`/`>`: byte slicing is on char boundaries.
    let inner = trimmed[1..trimmed.len() - 1]
        .trim()
        .replace('/', "")
        .trim()
        .to_string();
    inner == "br"
}

fn strip_html(html: &str) -> String {
    let mut result = html.to_string();
    // Comments first.
    while let Some(start) = result.find("<!--") {
        let Some(relative) = result[start + 4..].find("-->") else {
            break;
        };
        result.replace_range(start..start + 4 + relative + 3, " ");
    }
    // <br> variants to space.
    let result = replace_html_breaks(&result);
    // Remaining tags removed.
    let chars: Vec<char> = result.chars().collect();
    let mut stripped = String::with_capacity(result.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '<'
            && let Some(distance) = chars[index..].iter().position(|&c| c == '>')
        {
            let tag: String = chars[index..index + distance + 1].iter().collect();
            // Only treat letter-led tags as HTML to keep "a < b".
            let inner = tag[1..].trim_start();
            if let Some(first) = inner.chars().next()
                && (first.is_alphabetic() || matches!(first, '/' | '!' | '?'))
            {
                index += distance + 1;
                continue;
            }
        }
        stripped.push(chars[index]);
        index += 1;
    }
    decode_html_entities(&stripped)
}

fn replace_html_breaks(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut result = String::with_capacity(value.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '<'
            && let Some(distance) = chars[index..].iter().position(|&c| c == '>')
        {
            let remaining = chars.len() - index;
            if distance < 12 || remaining < 12 {
                let tag: String = chars[index..index + distance + 1].iter().collect();
                if is_break_tag(&tag) {
                    result.push(' ');
                    index += distance + 1;
                    continue;
                }
            }
        }
        result.push(chars[index]);
        index += 1;
    }
    result
}

fn decode_html_entities(value: &str) -> String {
    let mut result = value.to_string();
    result = result.replace("&nbsp;", " ");
    result = result.replace("&amp;", "&");
    result = result.replace("&lt;", "<");
    result = result.replace("&gt;", ">");
    result = result.replace("&quot;", "\"");
    result = result.replace("&#39;", "'");
    result = result.replace("&#x27;", "'");
    result
}

// MARK: - Footnote definitions

fn strip_footnote_definitions(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut kept = Vec::with_capacity(lines.len());
    for line in lines {
        if !is_footnote_definition(line) {
            kept.push(line);
        }
    }
    kept.join("\n")
}

fn is_footnote_definition(line: &str) -> bool {
    let chars: Vec<char> = line.chars().collect();
    let mut index = 0;
    while index < chars.len() && (chars[index] == ' ' || chars[index] == '\t') {
        index += 1;
    }
    if index >= chars.len() || chars[index] != '[' {
        return false;
    }
    index += 1;
    if index >= chars.len() || chars[index] != '^' {
        return false;
    }
    index += 1;
    let mut count = 0;
    while index < chars.len() && chars[index] != ']' {
        index += 1;
        count += 1;
    }
    if count == 0 {
        return false;
    }
    if index >= chars.len() || chars[index] != ']' {
        return false;
    }
    index += 1;
    if index >= chars.len() || chars[index] != ':' {
        return false;
    }
    true
}
