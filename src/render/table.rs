//! Tables whose column widths come from their contents, so columns always line up.

use std::fmt::Display;

use anstyle::Style;

const BOLD: Style = Style::new().bold();
const DIM: Style = Style::new().dimmed();
const INDENT: &str = "  ";
const GAP: &str = "  ";

pub fn paint(style: Style, text: impl Display) -> String {
    format!("{style}{text}{style:#}")
}

/// What to draw besides the data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Show {
    /// Column headers and totals rows.
    pub headers: bool,
    /// Explanation columns.
    pub help: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Align {
    Left,
    Right,
}

/// Whether a column holds data, or explanations that `--no-help` hides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Content {
    Data,
    Help,
}

#[derive(Debug, Clone, Copy)]
pub struct Column {
    header: Option<&'static str>,
    align: Align,
    content: Content,
}

impl Column {
    /// Unnamed columns, for tables without a header row.
    pub const LEFT: Self = Self { header: None, align: Align::Left, content: Content::Data };
    pub const RIGHT: Self = Self { header: None, align: Align::Right, content: Content::Data };
    pub const HELP: Self = Self { header: None, align: Align::Left, content: Content::Help };

    pub const fn left(header: &'static str) -> Self {
        Self { header: Some(header), ..Self::LEFT }
    }

    pub const fn right(header: &'static str) -> Self {
        Self { header: Some(header), ..Self::RIGHT }
    }

    pub const fn help(header: &'static str) -> Self {
        Self { header: Some(header), ..Self::HELP }
    }
}

#[derive(Debug, Clone)]
struct Span {
    text: String,
    style: Style,
}

/// Styled text. Widths are measured on the text alone, so styling never skews columns.
#[derive(Debug, Clone, Default)]
pub struct Cell {
    spans: Vec<Span>,
}

impl Cell {
    pub fn new(text: impl Display) -> Self {
        Self::styled(text, Style::new())
    }

    pub fn styled(text: impl Display, style: Style) -> Self {
        Self::spans([(text, style)])
    }

    /// Several differently styled pieces of text in one cell.
    pub fn spans<T: Display>(spans: impl IntoIterator<Item = (T, Style)>) -> Self {
        let spans = spans.into_iter().map(|(text, style)| Span { text: text.to_string(), style });
        Self { spans: spans.collect() }
    }

    /// The same text in a single style, replacing any styling it had.
    pub fn restyled(self, style: Style) -> Self {
        Self::styled(self.text(), style)
    }

    fn text(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }

    fn width(&self) -> usize {
        self.spans.iter().map(|span| span.text.chars().count()).sum()
    }

    fn padded(&self, width: usize, align: Align) -> String {
        let painted: String = self
            .spans
            .iter()
            .filter(|span| !span.text.is_empty())
            .map(|span| paint(span.style, &span.text))
            .collect();
        let padding = " ".repeat(width.saturating_sub(self.width()));
        match align {
            Align::Left => format!("{painted}{padding}"),
            Align::Right => format!("{padding}{painted}"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Row {
    Cells(Vec<Cell>),
    /// Totals, in bold under a rule; drawn only with headers on.
    Total(Vec<Cell>),
    /// An empty line.
    Gap,
}

impl Row {
    /// The row with every cell in `style`.
    pub fn styled(cells: Vec<Cell>, style: Style) -> Self {
        Self::Cells(cells.into_iter().map(|cell| cell.restyled(style)).collect())
    }

    /// The cells to draw, if the row has any under `show`.
    fn cells(&self, show: Show) -> Option<&[Cell]> {
        match self {
            Self::Cells(cells) => Some(cells),
            Self::Total(cells) if show.headers => Some(cells),
            Self::Total(_) | Self::Gap => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Table {
    columns: Vec<Column>,
    rows: Vec<Row>,
}

/// A column that made it onto the page, and how wide it is.
#[derive(Clone, Copy)]
struct Visible {
    index: usize,
    column: Column,
    width: usize,
}

impl Table {
    pub fn new(columns: Vec<Column>, rows: Vec<Row>) -> Self {
        Self { columns, rows }
    }

    pub fn render(&self, show: Show) -> Vec<String> {
        let has_header = show.headers && self.columns.iter().any(|c| c.header.is_some());
        // A column is shown if it's allowed and has content; it is as wide as its widest
        // cell, or its header.
        let visible: Vec<Visible> = self
            .columns
            .iter()
            .enumerate()
            .filter(|(_, column)| show.help || column.content == Content::Data)
            .filter_map(|(index, &column)| {
                let widest = self
                    .rows
                    .iter()
                    .filter_map(|row| row.cells(show))
                    .map(|cells| cells.get(index).map_or(0, Cell::width))
                    .max()
                    .unwrap_or(0);
                let header = column.header.filter(|_| has_header).map_or(0, |h| h.chars().count());
                (widest > 0).then_some(Visible { index, column, width: widest.max(header) })
            })
            .collect();

        let line = |cells: &[Cell]| {
            let text: Vec<String> = visible
                .iter()
                .map(|v| {
                    let cell = cells.get(v.index).cloned().unwrap_or_default();
                    let cell = match v.column.content {
                        Content::Help => cell.restyled(DIM),
                        Content::Data => cell,
                    };
                    cell.padded(v.width, v.column.align)
                })
                .collect();
            format!("{INDENT}{}", text.join(GAP)).trim_end().to_owned()
        };
        let rule = || {
            let width = visible.iter().map(|v| v.width).sum::<usize>()
                + GAP.chars().count() * visible.len().saturating_sub(1);
            format!("{INDENT}{}", "─".repeat(width))
        };

        let header: Vec<Cell> = self
            .columns
            .iter()
            .map(|column| Cell::styled(column.header.unwrap_or_default(), BOLD))
            .collect();
        let body = self.rows.iter().flat_map(|row| match row {
            Row::Cells(cells) => vec![line(cells)],
            Row::Total(cells) if show.headers => {
                let bold: Vec<Cell> = cells.iter().cloned().map(|c| c.restyled(BOLD)).collect();
                vec![rule(), line(&bold)]
            }
            Row::Total(_) => vec![],
            Row::Gap => vec![String::new()],
        });
        has_header.then(|| line(&header)).into_iter().chain(body).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: Vec<String>) -> Vec<String> {
        lines.into_iter().map(|line| crate::render::strip_ansi(&line)).collect()
    }

    #[test]
    fn sizes_columns_to_their_widest_cell() {
        let table = Table::new(
            vec![Column::right("PID"), Column::left("Name"), Column::help("Why")],
            vec![
                Row::Cells(vec![Cell::new(7), Cell::new("a"), Cell::new("x")]),
                Row::Cells(vec![Cell::new(2909719), Cell::new("long name"), Cell::new("")]),
                Row::Total(vec![Cell::new(""), Cell::new("all")]),
            ],
        );
        assert_eq!(
            plain(table.render(Show { headers: true, help: true })),
            [
                "      PID  Name       Why",
                "        7  a          x",
                "  2909719  long name",
                "  ───────────────────────",
                "           all",
            ]
        );
        assert_eq!(
            plain(table.render(Show { headers: false, help: false })),
            ["        7  a", "  2909719  long name"]
        );
    }

    #[test]
    fn drops_empty_columns() {
        let table = Table::new(
            vec![Column::LEFT, Column::RIGHT, Column::RIGHT],
            vec![Row::Cells(vec![Cell::new("x"), Cell::default(), Cell::new("y")])],
        );
        assert_eq!(table.render(Show { headers: false, help: true }), ["  x  y"]);
    }

    #[test]
    fn measures_styled_text_by_its_characters() {
        let table = Table::new(
            vec![Column::LEFT, Column::RIGHT],
            vec![
                Row::Cells(vec![Cell::styled("ab", BOLD), Cell::new(1)]),
                Row::Cells(vec![Cell::new("abcd"), Cell::new(2)]),
            ],
        );
        assert_eq!(
            plain(table.render(Show { headers: true, help: true })),
            ["  ab    1", "  abcd  2"]
        );
    }
}
