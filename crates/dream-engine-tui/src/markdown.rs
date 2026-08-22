use pulldown_cmark::{Alignment as TableAlignment, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Clone, Copy)]
pub(super) struct MarkdownTheme {
    pub(super) body: Style,
    pub(super) inline_code: Style,
    pub(super) code_block: Style,
    pub(super) marker: Style,
    pub(super) rule: Style,
}

pub(super) fn render_markdown(input: &str, width: u16, theme: MarkdownTheme) -> Vec<Line<'static>> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(input, options);
    let mut renderer = MarkdownRenderer::new(width.max(1), theme);
    renderer.render(parser);
    renderer.finish()
}

#[derive(Debug)]
struct CodeBlock {
    content: String,
}

#[derive(Debug)]
struct ListState {
    next: Option<u64>,
    marker: Option<String>,
    indent: usize,
}

impl ListState {
    fn new(next: Option<u64>) -> Self {
        Self {
            next,
            marker: None,
            indent: 2,
        }
    }

    fn begin_item(&mut self) {
        let marker = if let Some(next) = &mut self.next {
            let marker = format!("{next}. ");
            *next = next.saturating_add(1);
            marker
        } else {
            "• ".to_string()
        };
        self.indent = UnicodeWidthStr::width(marker.as_str());
        self.marker = Some(marker);
    }
}

#[derive(Debug, Default)]
struct InlineState {
    strong: usize,
    emphasis: usize,
    strikethrough: usize,
    link: usize,
    heading: Option<HeadingLevel>,
}

impl InlineState {
    fn style(&self, base: Style) -> Style {
        let mut modifiers = Modifier::empty();
        if self.strong > 0 || self.heading.is_some() {
            modifiers.insert(Modifier::BOLD);
        }
        if self.emphasis > 0 {
            modifiers.insert(Modifier::ITALIC);
        }
        if self.strikethrough > 0 {
            modifiers.insert(Modifier::CROSSED_OUT);
        }
        if self.link > 0 {
            modifiers.insert(Modifier::UNDERLINED);
        }
        if matches!(self.heading, Some(HeadingLevel::H1)) {
            modifiers.insert(Modifier::UNDERLINED);
        }
        base.add_modifier(modifiers)
    }
}

#[derive(Debug)]
struct LogicalLine {
    prefix: Vec<Span<'static>>,
    continuation_prefix: Vec<Span<'static>>,
    spans: Vec<Span<'static>>,
    fill: Option<Style>,
}

#[derive(Debug, Default)]
struct TableCell {
    spans: Vec<Span<'static>>,
}

#[derive(Debug)]
struct TableRow {
    cells: Vec<TableCell>,
    header: bool,
}

#[derive(Debug)]
struct TableState {
    alignments: Vec<TableAlignment>,
    rows: Vec<TableRow>,
    current_row: Option<TableRow>,
    current_cell: Option<TableCell>,
}

impl TableState {
    fn new(alignments: Vec<TableAlignment>) -> Self {
        Self {
            alignments,
            rows: Vec::new(),
            current_row: None,
            current_cell: None,
        }
    }

    fn begin_row(&mut self, header: bool) {
        self.finish_row();
        self.current_row = Some(TableRow {
            cells: Vec::new(),
            header,
        });
    }

    fn begin_cell(&mut self) {
        self.finish_cell();
        self.current_cell = Some(TableCell::default());
    }

    fn finish_cell(&mut self) {
        let Some(cell) = self.current_cell.take() else {
            return;
        };
        if let Some(row) = self.current_row.as_mut() {
            row.cells.push(cell);
        }
    }

    fn finish_row(&mut self) {
        self.finish_cell();
        if let Some(row) = self.current_row.take() {
            self.rows.push(row);
        }
    }
}

#[derive(Debug)]
struct MarkdownRenderer {
    width: u16,
    theme: MarkdownTheme,
    lines: Vec<Line<'static>>,
    current: Option<LogicalLine>,
    inline: InlineState,
    lists: Vec<ListState>,
    quote_depth: usize,
    code_block: Option<CodeBlock>,
    table: Option<TableState>,
    pending_separator: bool,
}

impl MarkdownRenderer {
    fn new(width: u16, theme: MarkdownTheme) -> Self {
        Self {
            width,
            theme,
            lines: Vec::new(),
            current: None,
            inline: InlineState::default(),
            lists: Vec::new(),
            quote_depth: 0,
            code_block: None,
            table: None,
            pending_separator: false,
        }
    }

    fn render<'a>(&mut self, parser: impl Iterator<Item = Event<'a>>) {
        for event in parser {
            if let Some(code_block) = self.code_block.as_mut() {
                match event {
                    Event::Text(text) | Event::Code(text) => code_block.content.push_str(&text),
                    Event::SoftBreak | Event::HardBreak => code_block.content.push('\n'),
                    Event::End(TagEnd::CodeBlock) => self.finish_code_block(),
                    _ => {}
                }
                continue;
            }

            if self.table.is_some() {
                self.render_table_event(event);
                continue;
            }

            match event {
                Event::Start(tag) => self.start_tag(tag),
                Event::End(tag) => self.end_tag(tag),
                Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => self.push_text(&text),
                Event::Code(code) => {
                    let style = self.inline.style(self.theme.inline_code);
                    self.push_span(Span::styled(code.to_string(), style));
                }
                Event::SoftBreak => self.push_soft_break(),
                Event::HardBreak => self.flush_current(),
                Event::Rule => self.push_rule(),
                Event::TaskListMarker(checked) => {
                    self.push_text(if checked { "[x] " } else { "[ ] " });
                }
                Event::FootnoteReference(label) => self.push_text(&format!("[{label}]")),
                Event::InlineMath(math) => self.push_text(&format!("${math}$")),
                Event::DisplayMath(math) => {
                    self.start_block();
                    self.push_text(&math);
                    self.flush_current();
                    self.pending_separator = true;
                }
            }
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.start_block(),
            Tag::Heading { level, .. } => {
                self.start_block();
                self.inline.heading = Some(level);
            }
            Tag::BlockQuote(_) => {
                self.start_block();
                self.quote_depth = self.quote_depth.saturating_add(1);
            }
            Tag::CodeBlock(_) => {
                self.start_block();
                self.code_block = Some(CodeBlock { content: String::new() });
            }
            Tag::Table(alignments) => {
                self.start_block();
                self.table = Some(TableState::new(alignments));
            }
            Tag::List(next) => {
                if self.lists.is_empty() {
                    self.start_block();
                } else {
                    self.flush_current();
                }
                self.lists.push(ListState::new(next));
            }
            Tag::Item => {
                self.flush_current();
                if let Some(list) = self.lists.last_mut() {
                    list.begin_item();
                }
            }
            Tag::Emphasis => self.inline.emphasis = self.inline.emphasis.saturating_add(1),
            Tag::Strong => self.inline.strong = self.inline.strong.saturating_add(1),
            Tag::Strikethrough => self.inline.strikethrough = self.inline.strikethrough.saturating_add(1),
            Tag::Link { .. } => self.inline.link = self.inline.link.saturating_add(1),
            Tag::Image { .. } => self.inline.emphasis = self.inline.emphasis.saturating_add(1),
            Tag::HtmlBlock => self.start_block(),
            _ => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_current();
                if self.lists.is_empty() {
                    self.pending_separator = true;
                }
            }
            TagEnd::Heading(_) => {
                self.flush_current();
                self.inline.heading = None;
                self.pending_separator = true;
            }
            TagEnd::BlockQuote(_) => {
                self.flush_current();
                self.quote_depth = self.quote_depth.saturating_sub(1);
                self.pending_separator = true;
            }
            TagEnd::CodeBlock => self.finish_code_block(),
            TagEnd::List(_) => {
                self.flush_current();
                self.lists.pop();
                if self.lists.is_empty() {
                    self.pending_separator = true;
                }
            }
            TagEnd::Item => {
                self.flush_current();
                if let Some(list) = self.lists.last_mut() {
                    list.marker = None;
                }
            }
            TagEnd::Emphasis => self.inline.emphasis = self.inline.emphasis.saturating_sub(1),
            TagEnd::Strong => self.inline.strong = self.inline.strong.saturating_sub(1),
            TagEnd::Strikethrough => self.inline.strikethrough = self.inline.strikethrough.saturating_sub(1),
            TagEnd::Link => self.inline.link = self.inline.link.saturating_sub(1),
            TagEnd::Image => self.inline.emphasis = self.inline.emphasis.saturating_sub(1),
            TagEnd::HtmlBlock => {
                self.flush_current();
                self.pending_separator = true;
            }
            _ => {}
        }
    }

    fn render_table_event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start_table_tag(tag),
            Event::End(tag) => self.end_table_tag(tag),
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                self.push_table_text(&text);
            }
            Event::Code(code) => {
                let style = self.inline.style(self.theme.inline_code);
                self.push_table_span(Span::styled(code.to_string(), style));
            }
            Event::SoftBreak | Event::HardBreak => self.push_table_text(" "),
            Event::Rule => self.push_table_text("──"),
            Event::TaskListMarker(checked) => {
                self.push_table_text(if checked { "[x] " } else { "[ ] " });
            }
            Event::FootnoteReference(label) => self.push_table_text(&format!("[{label}]")),
            Event::InlineMath(math) => self.push_table_text(&format!("${math}$")),
            Event::DisplayMath(math) => self.push_table_text(&math),
        }
    }

    fn start_table_tag(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::TableHead => {
                if let Some(table) = self.table.as_mut() {
                    table.begin_row(true);
                }
            }
            Tag::TableRow => {
                if let Some(table) = self.table.as_mut() {
                    table.begin_row(false);
                }
            }
            Tag::TableCell => {
                if let Some(table) = self.table.as_mut() {
                    table.begin_cell();
                }
            }
            Tag::Emphasis => self.inline.emphasis = self.inline.emphasis.saturating_add(1),
            Tag::Strong => self.inline.strong = self.inline.strong.saturating_add(1),
            Tag::Strikethrough => self.inline.strikethrough = self.inline.strikethrough.saturating_add(1),
            Tag::Link { .. } => self.inline.link = self.inline.link.saturating_add(1),
            Tag::Image { .. } => self.inline.emphasis = self.inline.emphasis.saturating_add(1),
            _ => {}
        }
    }

    fn end_table_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Table => self.finish_table(),
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(table) = self.table.as_mut() {
                    table.finish_row();
                }
            }
            TagEnd::TableCell => {
                if let Some(table) = self.table.as_mut() {
                    table.finish_cell();
                }
            }
            TagEnd::Emphasis => self.inline.emphasis = self.inline.emphasis.saturating_sub(1),
            TagEnd::Strong => self.inline.strong = self.inline.strong.saturating_sub(1),
            TagEnd::Strikethrough => self.inline.strikethrough = self.inline.strikethrough.saturating_sub(1),
            TagEnd::Link => self.inline.link = self.inline.link.saturating_sub(1),
            TagEnd::Image => self.inline.emphasis = self.inline.emphasis.saturating_sub(1),
            _ => {}
        }
    }

    fn push_table_text(&mut self, text: &str) {
        let normalized = text.replace('\n', " ");
        let style = self.inline.style(self.theme.body);
        self.push_table_span(Span::styled(normalized, style));
    }

    fn push_table_span(&mut self, span: Span<'static>) {
        if let Some(cell) = self.table.as_mut().and_then(|table| table.current_cell.as_mut()) {
            cell.spans.push(span);
        }
    }

    fn finish_table(&mut self) {
        let Some(mut table) = self.table.take() else {
            return;
        };
        table.finish_row();
        let column_count = table
            .rows
            .iter()
            .map(|row| row.cells.len())
            .max()
            .unwrap_or(0)
            .max(table.alignments.len());
        if column_count == 0 {
            return;
        }

        let border_width = column_count.saturating_mul(3).saturating_add(1);
        let content_budget = usize::from(self.width).saturating_sub(border_width);
        if content_budget < column_count.saturating_mul(3) {
            self.render_stacked_table(&table, column_count);
        } else {
            self.render_bordered_table(&table, column_count, content_budget);
        }
        self.pending_separator = true;
    }

    fn render_bordered_table(&mut self, table: &TableState, column_count: usize, content_budget: usize) {
        let mut natural_widths = vec![1; column_count];
        for row in &table.rows {
            for (index, cell) in row.cells.iter().enumerate() {
                natural_widths[index] = natural_widths[index].max(spans_width(&cell.spans));
            }
        }
        let mut widths = vec![1; column_count];
        let mut remaining = content_budget.saturating_sub(column_count);
        while remaining > 0 {
            let mut grew = false;
            for index in 0..column_count {
                if remaining == 0 {
                    break;
                }
                if widths[index] < natural_widths[index] {
                    widths[index] += 1;
                    remaining -= 1;
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }

        self.lines.push(table_border("┌", "┬", "┐", &widths, self.theme.marker));
        for row in &table.rows {
            self.lines.extend(table_row_lines(
                row,
                &table.alignments,
                &widths,
                self.theme.body,
                self.theme.marker,
            ));
            if row.header {
                self.lines.push(table_border("├", "┼", "┤", &widths, self.theme.marker));
            }
        }
        self.lines.push(table_border("└", "┴", "┘", &widths, self.theme.marker));
    }

    fn render_stacked_table(&mut self, table: &TableState, column_count: usize) {
        let header = table.rows.iter().find(|row| row.header);
        let data_rows = table.rows.iter().filter(|row| !row.header).collect::<Vec<_>>();
        if data_rows.is_empty() {
            if let Some(header) = header {
                for cell in &header.cells {
                    self.push_logical(LogicalLine {
                        prefix: Vec::new(),
                        continuation_prefix: Vec::new(),
                        spans: bold_spans(&cell.spans, self.theme.body),
                        fill: None,
                    });
                }
            }
            return;
        }

        for (row_index, row) in data_rows.into_iter().enumerate() {
            if row_index > 0 {
                self.lines.push(Line::default());
            }
            for index in 0..column_count {
                let label = header
                    .and_then(|header| header.cells.get(index))
                    .map(|cell| cell.spans.iter().map(|span| span.content.as_ref()).collect::<String>())
                    .filter(|label| !label.trim().is_empty())
                    .unwrap_or_else(|| format!("Column {}", index + 1));
                let mut spans = vec![Span::styled(
                    format!("{label}: "),
                    self.theme.body.add_modifier(Modifier::BOLD),
                )];
                if let Some(cell) = row.cells.get(index) {
                    spans.extend(cell.spans.clone());
                }
                self.push_logical(LogicalLine {
                    prefix: Vec::new(),
                    continuation_prefix: vec![Span::raw("  ")],
                    spans,
                    fill: None,
                });
            }
        }
    }

    fn start_block(&mut self) {
        self.flush_current();
        if self.pending_separator && self.lines.last().is_some_and(|line| !line.spans.is_empty()) {
            self.lines.push(Line::default());
        }
        self.pending_separator = false;
    }

    fn ensure_current(&mut self) -> &mut LogicalLine {
        if self.current.is_none() {
            let (prefix, continuation_prefix) = self.take_prefixes();
            self.current = Some(LogicalLine {
                prefix,
                continuation_prefix,
                spans: Vec::new(),
                fill: None,
            });
        }
        self.current.as_mut().expect("current markdown line is initialized")
    }

    fn take_prefixes(&mut self) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
        let mut prefix = Vec::new();
        let mut continuation = Vec::new();
        for _ in 0..self.quote_depth {
            prefix.push(Span::styled("│ ", self.theme.marker));
            continuation.push(Span::styled("│ ", self.theme.marker));
        }
        let list_count = self.lists.len();
        for (index, list) in self.lists.iter_mut().enumerate() {
            if index + 1 == list_count
                && let Some(marker) = list.marker.take()
            {
                continuation.push(Span::raw(" ".repeat(list.indent)));
                prefix.push(Span::styled(marker, self.theme.marker));
            } else {
                let indent = " ".repeat(list.indent);
                prefix.push(Span::raw(indent.clone()));
                continuation.push(Span::raw(indent));
            }
        }
        (prefix, continuation)
    }

    fn push_text(&mut self, text: &str) {
        let style = self.inline.style(self.theme.body);
        let mut parts = text.split('\n').peekable();
        while let Some(part) = parts.next() {
            if !part.is_empty() {
                self.push_span(Span::styled(part.to_string(), style));
            }
            if parts.peek().is_some() {
                self.flush_current();
            }
        }
    }

    fn push_span(&mut self, span: Span<'static>) {
        self.ensure_current().spans.push(span);
    }

    fn push_soft_break(&mut self) {
        let needs_space = self
            .current
            .as_ref()
            .and_then(|line| line.spans.last())
            .is_some_and(|span| {
                span.content
                    .chars()
                    .last()
                    .is_some_and(|character| !character.is_whitespace())
            });
        if needs_space {
            self.push_text(" ");
        }
    }

    fn push_rule(&mut self) {
        self.start_block();
        let width = usize::from(self.width).min(32);
        self.lines
            .push(Line::from(Span::styled("─".repeat(width), self.theme.rule)));
        self.pending_separator = true;
    }

    fn finish_code_block(&mut self) {
        let Some(code_block) = self.code_block.take() else {
            return;
        };
        let content = code_block.content.strip_suffix('\n').unwrap_or(&code_block.content);
        for code_line in content.split('\n') {
            let (mut prefix, mut continuation_prefix) = self.take_prefixes();
            prefix.push(Span::styled(" ", self.theme.code_block));
            continuation_prefix.push(Span::styled(" ", self.theme.code_block));
            self.push_logical(LogicalLine {
                prefix,
                continuation_prefix,
                spans: vec![Span::styled(code_line.to_string(), self.theme.code_block)],
                fill: Some(self.theme.code_block),
            });
        }
        self.pending_separator = true;
    }

    fn flush_current(&mut self) {
        if let Some(line) = self.current.take() {
            self.push_logical(line);
        }
    }

    fn push_logical(&mut self, line: LogicalLine) {
        self.lines.extend(wrap_line(line, self.width));
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        if self.code_block.is_some() {
            self.finish_code_block();
        }
        if self.table.is_some() {
            self.finish_table();
        }
        self.flush_current();
        while self.lines.last().is_some_and(|line| line.spans.is_empty()) {
            self.lines.pop();
        }
        self.lines
    }
}

fn table_border(left: &str, separator: &str, right: &str, widths: &[usize], style: Style) -> Line<'static> {
    let mut border = String::from(left);
    for (index, width) in widths.iter().enumerate() {
        border.push_str(&"─".repeat(width.saturating_add(2)));
        border.push_str(if index + 1 == widths.len() { right } else { separator });
    }
    Line::from(Span::styled(border, style))
}

fn table_row_lines(
    row: &TableRow,
    alignments: &[TableAlignment],
    widths: &[usize],
    body_style: Style,
    border_style: Style,
) -> Vec<Line<'static>> {
    let wrapped = widths
        .iter()
        .enumerate()
        .map(|(index, width)| {
            let spans = row.cells.get(index).map(|cell| cell.spans.clone()).unwrap_or_default();
            let spans = if row.header {
                bold_spans(&spans, body_style)
            } else {
                spans
            };
            wrap_line(
                LogicalLine {
                    prefix: Vec::new(),
                    continuation_prefix: Vec::new(),
                    spans,
                    fill: None,
                },
                (*width).min(u16::MAX as usize) as u16,
            )
        })
        .collect::<Vec<_>>();
    let row_height = wrapped.iter().map(Vec::len).max().unwrap_or(1);
    let mut output = Vec::with_capacity(row_height);
    for line_index in 0..row_height {
        let mut spans = vec![Span::styled("│", border_style)];
        for (column, width) in widths.iter().enumerate() {
            let content = wrapped
                .get(column)
                .and_then(|lines| lines.get(line_index))
                .map(|line| line.spans.clone())
                .unwrap_or_default();
            let content_width = spans_width(&content).min(*width);
            let free = width.saturating_sub(content_width);
            let alignment = alignments.get(column).copied().unwrap_or(TableAlignment::None);
            let (left_padding, right_padding) = match alignment {
                TableAlignment::Center => (free / 2, free.saturating_sub(free / 2)),
                TableAlignment::Right => (free, 0),
                TableAlignment::None | TableAlignment::Left => (0, free),
            };
            spans.push(Span::styled(format!(" {}", " ".repeat(left_padding)), body_style));
            spans.extend(content);
            spans.push(Span::styled(format!("{} ", " ".repeat(right_padding)), body_style));
            spans.push(Span::styled("│", border_style));
        }
        output.push(Line::from(spans));
    }
    output
}

fn bold_spans(spans: &[Span<'static>], fallback_style: Style) -> Vec<Span<'static>> {
    if spans.is_empty() {
        return vec![Span::styled(String::new(), fallback_style.add_modifier(Modifier::BOLD))];
    }
    spans
        .iter()
        .cloned()
        .map(|mut span| {
            span.style = span.style.add_modifier(Modifier::BOLD);
            span
        })
        .collect()
}

fn wrap_line(line: LogicalLine, width: u16) -> Vec<Line<'static>> {
    let max_width = usize::from(width.max(1));
    let initial_prefix_width = spans_width(&line.prefix);
    let continuation_prefix_width = spans_width(&line.continuation_prefix);
    let mut output = Vec::new();
    let mut row = line.prefix;
    let mut row_width = initial_prefix_width;
    let mut continuation = false;

    for span in line.spans {
        let style = span.style;
        let mut fragment = String::new();
        for character in span.content.chars() {
            let character_width = character.width().unwrap_or(1).max(1);
            let content_start = if continuation {
                continuation_prefix_width
            } else {
                initial_prefix_width
            };
            if row_width > content_start && row_width.saturating_add(character_width) > max_width {
                push_fragment(&mut row, &mut fragment, style);
                finish_row(&mut output, row, row_width, max_width, line.fill);
                row = line.continuation_prefix.clone();
                row_width = spans_width(&row);
                continuation = true;
            }
            fragment.push(character);
            row_width = row_width.saturating_add(character_width);
        }
        push_fragment(&mut row, &mut fragment, style);
    }

    finish_row(&mut output, row, row_width, max_width, line.fill);
    output
}

fn push_fragment(row: &mut Vec<Span<'static>>, fragment: &mut String, style: Style) {
    if !fragment.is_empty() {
        row.push(Span::styled(std::mem::take(fragment), style));
    }
}

fn finish_row(
    output: &mut Vec<Line<'static>>,
    mut row: Vec<Span<'static>>,
    row_width: usize,
    max_width: usize,
    fill: Option<Style>,
) {
    if let Some(fill) = fill
        && row_width < max_width
    {
        row.push(Span::styled(" ".repeat(max_width - row_width), fill));
    }
    output.push(Line::from(row));
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|span| span.width()).sum()
}

#[cfg(test)]
#[path = "markdown_test.rs"]
mod markdown_test;
