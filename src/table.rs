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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

/// Whether a column holds data, or explanations that `--no-help` hides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Data,
    Help,
}

#[derive(Debug, Clone, Copy)]
pub struct Column {
    header: &'static str,
    align: Align,
    kind: Kind,
}

impl Column {
    pub const fn left(header: &'static str) -> Self {
        Self { header, align: Align::Left, kind: Kind::Data }
    }

    pub const fn right(header: &'static str) -> Self {
        Self { header, align: Align::Right, kind: Kind::Data }
    }

    pub const fn help(header: &'static str) -> Self {
        Self { header, align: Align::Left, kind: Kind::Help }
    }
}

/// Text plus its styled rendering; widths are measured on the plain text.
#[derive(Debug, Clone, Default)]
pub struct Cell {
    plain: String,
    styled: String,
}

impl Cell {
    pub fn new(text: impl Display) -> Self {
        let text = text.to_string();
        Self { styled: text.clone(), plain: text }
    }

    pub fn styled(text: impl Display, style: Style) -> Self {
        let text = text.to_string();
        // An empty cell stays empty, so trailing padding can be trimmed.
        let styled = if text.is_empty() { String::new() } else { paint(style, &text) };
        Self { styled, plain: text }
    }

    /// Several differently styled pieces of text in one cell.
    pub fn spans<'a>(spans: impl IntoIterator<Item = (&'a str, Style)>) -> Self {
        let (plain, styled): (Vec<&str>, Vec<String>) =
            spans.into_iter().map(|(text, style)| (text, paint(style, text))).unzip();
        Self { plain: plain.concat(), styled: styled.concat() }
    }

    /// The same text in a single style, replacing any styling it had.
    pub fn restyled(self, style: Style) -> Self {
        Self::styled(self.plain, style)
    }

    fn width(&self) -> usize {
        self.plain.chars().count()
    }

    fn padded(&self, width: usize, align: Align) -> String {
        let padding = " ".repeat(width.saturating_sub(self.width()));
        match align {
            Align::Left => format!("{}{padding}", self.styled),
            Align::Right => format!("{padding}{}", self.styled),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Row {
    Cells(Vec<Cell>),
    /// A horizontal line across the table, drawn only with headers on.
    Rule,
    /// A row of totals, drawn only with headers on.
    Footer(Vec<Cell>),
}

impl Row {
    /// The row with every cell in `style`.
    pub fn styled(cells: Vec<Cell>, style: Style) -> Self {
        Self::Cells(restyled(cells, style))
    }

    /// A row of totals, in bold.
    pub fn footer(cells: Vec<Cell>) -> Self {
        Self::Footer(restyled(cells, BOLD))
    }
}

fn restyled(cells: Vec<Cell>, style: Style) -> Vec<Cell> {
    cells.into_iter().map(|cell| cell.restyled(style)).collect()
}

/// What to draw besides the data.
#[derive(Debug, Clone, Copy)]
pub struct Show {
    pub headers: bool,
    pub help: bool,
}

#[derive(Debug, Clone)]
pub struct Table {
    columns: Vec<Column>,
    rows: Vec<Row>,
}

impl Table {
    pub fn new(columns: Vec<Column>, rows: Vec<Row>) -> Self {
        Self { columns, rows }
    }

    pub fn render(&self, show: Show) -> Vec<String> {
        let cells_of = |index: usize| {
            self.rows.iter().filter_map(move |row| match row {
                Row::Cells(cells) => Some(cells.get(index).map_or(0, Cell::width)),
                Row::Footer(cells) if show.headers => Some(cells.get(index).map_or(0, Cell::width)),
                Row::Footer(_) | Row::Rule => None,
            })
        };
        let has_header = show.headers && self.columns.iter().any(|c| !c.header.is_empty());
        // A column is shown if it's allowed and has any content; it is as wide as its
        // widest cell (or header).
        let visible: Vec<(usize, Column, usize)> = self
            .columns
            .iter()
            .enumerate()
            .filter(|(_, column)| show.help || column.kind == Kind::Data)
            .filter_map(|(index, column)| {
                let content = cells_of(index).max().unwrap_or(0);
                let header = if has_header { column.header.chars().count() } else { 0 };
                (content > 0).then_some((index, *column, content.max(header)))
            })
            .collect();

        let line = |cells: &[Cell]| {
            let text: Vec<String> = visible
                .iter()
                .map(|&(index, column, width)| {
                    let cell = cells.get(index).cloned().unwrap_or_default();
                    let cell = match column.kind {
                        Kind::Help => cell.restyled(DIM),
                        Kind::Data => cell,
                    };
                    cell.padded(width, column.align)
                })
                .collect();
            format!("{INDENT}{}", text.join(GAP)).trim_end().to_owned()
        };
        let total_width: usize = visible.iter().map(|&(_, _, width)| width).sum::<usize>()
            + GAP.len() * visible.len().saturating_sub(1);

        let header: Vec<Cell> =
            self.columns.iter().map(|column| Cell::styled(column.header, BOLD)).collect();
        let header_line = has_header.then(|| line(&header));
        let body = self.rows.iter().filter_map(|row| match row {
            Row::Cells(cells) => Some(line(cells)),
            Row::Footer(cells) => show.headers.then(|| line(cells)),
            Row::Rule => show.headers.then(|| format!("{INDENT}{}", "─".repeat(total_width))),
        });
        header_line.into_iter().chain(body).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOW_ALL: Show = Show { headers: true, help: true };

    #[test]
    fn sizes_columns_to_their_widest_cell() {
        let table = Table::new(
            vec![Column::right("PID"), Column::left("Name"), Column::help("Why")],
            vec![
                Row::Cells(vec![Cell::new(7), Cell::new("a"), Cell::new("x")]),
                Row::Cells(vec![Cell::new(2909719), Cell::new("long name"), Cell::new("")]),
                Row::Rule,
            ],
        );
        let plain = |show| -> Vec<String> {
            table
                .render(show)
                .into_iter()
                .map(|line| {
                    line.replace("\u{1b}[1m", "").replace("\u{1b}[2m", "").replace("\u{1b}[0m", "")
                })
                .collect()
        };
        assert_eq!(
            plain(SHOW_ALL),
            [
                "      PID  Name       Why",
                "        7  a          x",
                "  2909719  long name",
                "  ───────────────────────",
            ]
        );
        assert_eq!(
            plain(Show { headers: false, help: false }),
            ["        7  a", "  2909719  long name"]
        );
    }

    #[test]
    fn drops_empty_columns() {
        let table = Table::new(
            vec![Column::left("A"), Column::right("Empty"), Column::right("B")],
            vec![Row::Cells(vec![Cell::new("x"), Cell::default(), Cell::new("y")])],
        );
        assert_eq!(table.render(Show { headers: false, help: true }), ["  x  y"]);
    }
}
