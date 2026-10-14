//! Full-screen live display (ratatui + crossterm), drawn on its own thread so
//! terminal writes never block the async runtime.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Cell, Paragraph, Row, Sparkline, Table};

use crate::monitor::{RegionRow, View};
use crate::runner::Mode;
use crate::stats::{Class, NCLASS};
use crate::util::{fmt_clock, fmt_int, fmt_ms, fmt_rate};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Quit,
    Faster,
    Slower,
}

enum UiMsg {
    View(Box<View>),
    Quit,
}

pub struct Ui {
    tx: Sender<UiMsg>,
    handle: Option<JoinHandle<()>>,
}

impl Ui {
    pub fn start(cmds: tokio::sync::mpsc::UnboundedSender<Command>) -> Ui {
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("loadgen-ui".into())
            .spawn(move || ui_loop(rx, cmds))
            .expect("spawn UI thread");
        Ui {
            tx,
            handle: Some(handle),
        }
    }

    pub fn show(&self, view: View) {
        let _ = self.tx.send(UiMsg::View(Box::new(view)));
    }

    /// Restore the terminal and wait for the UI thread.
    pub fn stop(&mut self) {
        if let Some(h) = self.handle.take() {
            let _ = self.tx.send(UiMsg::Quit);
            let _ = h.join();
        }
    }
}

impl Drop for Ui {
    fn drop(&mut self) {
        self.stop();
    }
}

fn ui_loop(rx: Receiver<UiMsg>, cmds: tokio::sync::mpsc::UnboundedSender<Command>) {
    let mut terminal = ratatui::init();
    let mut view: Option<Box<View>> = None;
    let mut dirty = false;
    loop {
        while event::poll(Duration::ZERO).unwrap_or(false) {
            match event::read() {
                Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => {
                    let cmd = match k.code {
                        KeyCode::Char('q') | KeyCode::Esc => Some(Command::Quit),
                        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                            Some(Command::Quit)
                        }
                        KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Up => {
                            Some(Command::Faster)
                        }
                        KeyCode::Char('-') | KeyCode::Char('_') | KeyCode::Down => {
                            Some(Command::Slower)
                        }
                        _ => None,
                    };
                    if let Some(c) = cmd {
                        let _ = cmds.send(c);
                    }
                }
                Ok(Event::Resize(..)) => dirty = true,
                _ => {}
            }
        }
        match rx.recv_timeout(Duration::from_millis(40)) {
            Ok(UiMsg::View(v)) => {
                view = Some(v);
                dirty = true;
            }
            Ok(UiMsg::Quit) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if dirty && let Some(v) = &view {
            let _ = terminal.draw(|f| render(f, v));
            dirty = false;
        }
    }
    ratatui::restore();
}

// ---------------------------------------------------------------------------
// Big digits: a 3x5 pixel font, each pixel two cells wide (cells are tall).

fn glyph(c: char) -> &'static [&'static str; 5] {
    match c {
        '0' => &["###", "# #", "# #", "# #", "###"],
        '1' => &[" # ", "## ", " # ", " # ", "###"],
        '2' => &["###", "  #", "###", "#  ", "###"],
        '3' => &["###", "  #", " ##", "  #", "###"],
        '4' => &["# #", "# #", "###", "  #", "  #"],
        '5' => &["###", "#  ", "###", "  #", "###"],
        '6' => &["###", "#  ", "###", "# #", "###"],
        '7' => &["###", "  #", "  #", "  #", "  #"],
        '8' => &["###", "# #", "###", "# #", "###"],
        '9' => &["###", "# #", "###", "  #", "###"],
        ',' => &[" ", " ", " ", "#", "#"],
        '.' => &[" ", " ", " ", " ", "#"],
        _ => &["   ", "   ", "   ", "   ", "   "],
    }
}

pub fn big_text(s: &str, scale: usize) -> Vec<String> {
    let mut rows = vec![String::new(); 5 * scale];
    for (i, ch) in s.chars().enumerate() {
        for (r, pattern) in glyph(ch).iter().enumerate() {
            let mut seg = String::new();
            if i > 0 {
                seg.push_str(&" ".repeat(2 * scale));
            }
            for px in pattern.chars() {
                let cell = if px == '#' { "█" } else { " " };
                for _ in 0..2 * scale {
                    seg.push_str(cell);
                }
            }
            for k in 0..scale {
                rows[r * scale + k].push_str(&seg);
            }
        }
    }
    rows
}

fn big_width(s: &str, scale: usize) -> usize {
    big_text(s, scale)
        .first()
        .map(|r| r.chars().count())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------

const GREEN: Color = Color::LightGreen;
const DIM: Color = Color::DarkGray;

fn bold(c: Color) -> Style {
    Style::new().fg(c).add_modifier(Modifier::BOLD)
}

pub fn render(f: &mut Frame, v: &View) {
    let area = f.area();
    let tps_text = fmt_int(v.total_tps.round() as u64);
    let scale = if area.height >= 46 && (big_width(&tps_text, 2) + 40) as u16 <= area.width {
        2
    } else {
        1
    };
    let table_h = v.regions.len() as u16 + 3 + u16::from(v.regions.len() > 1);
    let spark_h = if area.height >= 40 { 9 } else { 6 };
    let [header, big, spark, table, events, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(5 * scale as u16 + 2),
        Constraint::Length(spark_h),
        Constraint::Length(table_h),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    render_header(f, header, v);
    render_big(f, big, v, &tps_text, scale);
    render_spark(f, spark, v);
    render_table(f, table, v);
    if events.height >= 3 {
        render_events(f, events, v);
    }
    render_footer(f, footer, v);
}

fn state(v: &View) -> (String, Color) {
    if let Some(n) = v.stopping {
        (format!("STOPPING · {n} in flight"), Color::Yellow)
    } else if v.backing_off {
        ("BACKING OFF · throttled".into(), Color::Yellow)
    } else if v.ramp_left > 0.0 {
        (
            format!("RAMPING · {:.0} s to target", v.ramp_left.ceil()),
            Color::Cyan,
        )
    } else {
        ("STEADY".into(), GREEN)
    }
}

fn render_header(f: &mut Frame, area: Rect, v: &View) {
    let (state, color) = state(v);
    let mut clock = fmt_clock(v.elapsed);
    if let Some(d) = v.info.duration {
        clock = format!("{clock} / {}", fmt_clock(d));
    }
    let right = Line::from(vec![
        Span::styled(state, bold(color)),
        Span::raw("   "),
        Span::styled(clock, bold(Color::White)),
        Span::raw(" "),
    ]);
    let [l, r] = Layout::horizontal([Constraint::Min(0), Constraint::Length(right.width() as u16)])
        .areas(area);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(v.info.title.clone(), bold(Color::White)),
        ])),
        l,
    );
    f.render_widget(Paragraph::new(right), r);
}

fn render_big(f: &mut Frame, area: Rect, v: &View, tps_text: &str, scale: usize) {
    let width = big_width(tps_text, scale) as u16;
    let [left, right] =
        Layout::horizontal([Constraint::Length(width + 4), Constraint::Min(0)]).areas(area);
    let mut lines = vec![Line::raw("")];
    if width + 4 <= area.width {
        for row in big_text(tps_text, scale) {
            lines.push(Line::styled(format!("  {row}"), bold(GREEN)));
        }
    } else {
        lines.push(Line::styled(format!("  {tps_text}"), bold(GREEN)));
    }
    f.render_widget(Paragraph::new(lines), left);

    let unit = match v.mode {
        Mode::Rate => "/s",
        Mode::Concurrency => " workers",
    };
    let mut info = vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled("TPS", bold(Color::White)),
            Span::styled(
                "  successful API calls per second, all Regions",
                Style::new().fg(Color::Gray),
            ),
        ]),
        Line::styled(
            "     every call reads or writes Aurora DSQL",
            Style::new().fg(DIM),
        ),
        Line::raw(""),
        Line::from(vec![
            Span::styled("target ", Style::new().fg(Color::Gray)),
            Span::styled(format!("{}{unit}", fmt_rate(v.target)), bold(Color::White)),
            Span::styled("   sending ", Style::new().fg(Color::Gray)),
            Span::styled(format!("{}{unit}", fmt_rate(v.rate)), bold(Color::White)),
            Span::styled("   peak ", Style::new().fg(Color::Gray)),
            Span::styled(format!("{} TPS", fmt_rate(v.peak)), bold(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("in flight ", Style::new().fg(Color::Gray)),
            Span::styled(fmt_int(v.all.in_flight as u64), bold(Color::White)),
            Span::styled("   posts ", Style::new().fg(Color::Gray)),
            Span::styled(fmt_int(v.posts as u64), Style::new().fg(Color::White)),
            Span::styled("   users ", Style::new().fg(Color::Gray)),
            Span::styled(fmt_int(v.users as u64), Style::new().fg(Color::White)),
        ]),
    ];
    if v.all.skipped > 0 {
        info.push(Line::styled(
            format!(
                "skipped {} sends at the --connections cap",
                fmt_int(v.all.skipped)
            ),
            bold(Color::Yellow),
        ));
    }
    f.render_widget(Paragraph::new(info), right);
}

/// Resample to `width` columns, keeping peaks.
fn resample(data: &[u64], width: usize) -> Vec<u64> {
    if width == 0 || data.is_empty() {
        return Vec::new();
    }
    (0..width)
        .map(|i| {
            let start = i * data.len() / width;
            let end = ((i + 1) * data.len() / width)
                .max(start + 1)
                .min(data.len());
            data[start..end].iter().copied().max().unwrap_or(0)
        })
        .collect()
}

fn render_spark(f: &mut Frame, area: Rect, v: &View) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(DIM))
        .title(Span::styled(" TPS · last 60 s ", bold(Color::White)))
        .title(
            Line::from(Span::styled(
                format!(" peak {} ", fmt_rate(v.peak)),
                Style::new().fg(Color::Gray),
            ))
            .right_aligned(),
        );
    let inner = block.inner(area);
    let data = resample(&v.spark, inner.width as usize);
    let max = data.iter().copied().max().unwrap_or(0).max(1);
    f.render_widget(
        Sparkline::default()
            .block(block)
            .data(&data)
            .max(max + max / 10)
            .style(Style::new().fg(GREEN)),
        area,
    );
}

fn num(s: String, style: Style) -> Cell<'static> {
    Cell::from(Line::styled(s, style).right_aligned())
}

fn error_cell(total: u64, recent: u64, class: Class) -> Cell<'static> {
    let style = if recent > 0 {
        match class {
            Class::Throttled => bold(Color::Yellow),
            _ => bold(Color::LightRed),
        }
    } else if total > 0 {
        Style::new().fg(Color::White)
    } else {
        Style::new().fg(DIM)
    };
    num(fmt_int(total), style)
}

fn region_cells(r: &RegionRow, v: &View, total_row: bool, show_db: bool) -> Vec<Cell<'static>> {
    let label_style = if total_row {
        bold(Color::Gray)
    } else {
        bold(Color::White)
    };
    let unit = match v.mode {
        Mode::Rate => "/s",
        Mode::Concurrency => " w",
    };
    let rate_style = if r.backing_off {
        bold(Color::Yellow)
    } else {
        Style::new().fg(Color::Gray)
    };
    let lat = Style::new().fg(Color::White);
    let mut cells = vec![
        Cell::from(Span::styled(r.label.clone(), label_style)),
        num(fmt_rate(r.tps), bold(GREEN)),
        num(format!("{}{unit}", fmt_rate(r.rate)), rate_style),
        num(fmt_int(r.in_flight as u64), Style::new().fg(Color::White)),
        num(fmt_ms(r.p50), lat),
        num(fmt_ms(r.p90), lat),
        num(fmt_ms(r.p99), lat),
    ];
    if show_db {
        let db = Style::new().fg(Color::Cyan);
        cells.push(num(fmt_ms(r.db50), db));
        cells.push(num(fmt_ms(r.db99), db));
    }
    for c in Class::ERRORS {
        cells.push(error_cell(r.errors[c.index()], r.recent[c.index()], c));
    }
    cells
}

/// Column header and minimum width for the Regions table.
const BASE_COLS: [(&str, u16); 7] = [
    ("Region", 6),
    ("TPS", 7),
    ("rate", 9),
    ("in-flight", 9),
    ("p50 ms", 6),
    ("p90 ms", 6),
    ("p99 ms", 6),
];
const DB_COLS: [(&str, u16); 2] = [("DB p50", 6), ("DB p99", 6)];
const ERR_COLS: [(&str, u16); 5] = [
    ("4xx", 5),
    ("429", 5),
    ("5xx", 5),
    ("timeout", 7),
    ("connect", 7),
];

fn render_table(f: &mut Frame, area: Rect, v: &View) {
    let label_w = v
        .regions
        .iter()
        .map(|r| r.label.chars().count() as u16)
        .max()
        .unwrap_or(6);
    let inner_w = area.width.saturating_sub(2);
    let layout = |db: bool| -> Vec<(&'static str, u16)> {
        let mut cols: Vec<(&str, u16)> = BASE_COLS.to_vec();
        cols[0].1 = cols[0].1.max(label_w);
        if db {
            cols.extend(DB_COLS);
        }
        cols.extend(ERR_COLS);
        cols
    };
    let needed = |cols: &[(&str, u16)], spacing: u16| -> u16 {
        cols.iter().map(|c| c.1).sum::<u16>() + spacing * (cols.len() as u16 - 1)
    };
    // Prefer DB columns, then roomy spacing; drop DB time before squeezing.
    let mut show_db = v.show_db;
    let mut cols = layout(show_db);
    if show_db && needed(&cols, 1) > inner_w {
        show_db = false;
        cols = layout(false);
    }
    let spacing = if needed(&cols, 3) <= inner_w {
        3
    } else if needed(&cols, 2) <= inner_w {
        2
    } else {
        1
    };
    let header = Row::new(
        cols.iter()
            .enumerate()
            .map(|(i, (h, _))| {
                let line = Line::styled(*h, bold(Color::Cyan));
                Cell::from(if i == 0 { line } else { line.right_aligned() })
            })
            .collect::<Vec<_>>(),
    );
    let widths: Vec<Constraint> = cols.iter().map(|c| Constraint::Length(c.1)).collect();
    let mut rows: Vec<Row> = v
        .regions
        .iter()
        .map(|r| Row::new(region_cells(r, v, false, show_db)))
        .collect();
    if v.regions.len() > 1 {
        rows.push(Row::new(region_cells(&v.all, v, true, show_db)));
    }
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(DIM))
        .title(Span::styled(" Regions ", bold(Color::White)))
        .title(
            Line::from(Span::styled(
                " latency of 2xx over the last second; errors since start ",
                Style::new().fg(DIM),
            ))
            .right_aligned(),
        );
    f.render_widget(
        Table::new(rows, widths)
            .header(header)
            .column_spacing(spacing)
            .flex(Flex::SpaceBetween)
            .block(block),
        area,
    );
}

fn render_events(f: &mut Frame, area: Rect, v: &View) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(DIM))
        .title(Span::styled(" Events ", bold(Color::White)));
    let inner_h = area.height.saturating_sub(2) as usize;
    let lines: Vec<Line> = v
        .events
        .iter()
        .skip(v.events.len().saturating_sub(inner_h))
        .map(|e| Line::styled(e.clone(), Style::new().fg(Color::Gray)))
        .collect();
    f.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
}

fn render_footer(f: &mut Frame, area: Rect, v: &View) {
    let key = |k: &'static str| Span::styled(k, bold(Color::White));
    let txt = |t: String| Span::styled(t, Style::new().fg(DIM));
    let line = Line::from(vec![
        Span::raw(" "),
        key("q"),
        txt(" quit   ".into()),
        key("+/-"),
        txt(" target ±25%   ".into()),
        txt(format!("mix {}   {}   ", v.info.mix, v.info.http)),
        txt(format!("{} Region(s)", v.regions.len())),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

// Keep the per-class array size in sync with the table columns.
const _: () = assert!(NCLASS == 6);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::{SPARK_POINTS, ViewInfo};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn big_digits_have_consistent_rows() {
        let rows = big_text("12,345", 1);
        assert_eq!(rows.len(), 5);
        let w = rows[0].chars().count();
        assert!(rows.iter().all(|r| r.chars().count() == w));
        // 5 digits x 3 px + comma 1 px + 5 gaps, 2 cells per px.
        assert_eq!(w, (5 * 3 + 1 + 5) * 2);
        assert_eq!(big_text("8", 2).len(), 10);
    }

    #[test]
    fn resamples_keeping_peaks() {
        let data: Vec<u64> = (0..240).collect();
        let r = resample(&data, 60);
        assert_eq!(r.len(), 60);
        assert_eq!(*r.last().unwrap(), 239);
        let up = resample(&[1, 2], 6);
        assert_eq!(up, vec![1, 1, 1, 2, 2, 2]);
    }

    fn row(label: &str) -> RegionRow {
        RegionRow {
            label: label.into(),
            tps: 6172.0,
            rate: 6200.0,
            in_flight: 210,
            p50: Some(24.0),
            p90: Some(31.0),
            p99: Some(80.0),
            db50: Some(3.1),
            db99: Some(9.0),
            errors: [100, 0, 3, 1, 0, 0],
            recent: [10, 0, 3, 0, 0, 0],
            backing_off: false,
            skipped: 0,
        }
    }

    fn sample_view() -> View {
        View {
            info: ViewInfo {
                title: "Aurora DSQL · multi-Region blog".into(),
                duration: Some(300.0),
                mix: "90/10".into(),
                http: "HTTP/1.1".into(),
            },
            mode: Mode::Rate,
            elapsed: 83.0,
            total_tps: 12345.0,
            target: 20000.0,
            rate: 12400.0,
            ramp_left: 23.0,
            backing_off: false,
            stopping: None,
            spark: (0..SPARK_POINTS as u64).collect(),
            peak: 12400.0,
            regions: vec![row("us-east-1"), row("us-east-2")],
            all: row("all"),
            events: vec!["00:12 us-east-1: example event".into()],
            show_db: true,
            posts: 3412,
            users: 200,
        }
    }

    fn draw(view: &View, w: u16, h: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| render(f, view)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect()
    }

    /// `LOADGEN_PRINT_TUI=1 cargo test print_tui -- --nocapture` shows the layout.
    #[test]
    fn print_tui() {
        if std::env::var_os("LOADGEN_PRINT_TUI").is_some() {
            for line in draw(&sample_view(), 120, 34) {
                println!("{line}");
            }
        }
    }

    #[test]
    fn renders_without_panicking() {
        let view = sample_view();
        for (w, h) in [(120, 40), (80, 24), (200, 60), (40, 10)] {
            let text = draw(&view, w, h).concat();
            if w >= 80 {
                assert!(text.contains("us-east-1"), "{w}x{h}");
                assert!(text.contains("RAMPING"), "{w}x{h}");
            }
        }
    }
}
