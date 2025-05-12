use std::rc::Rc;

use itertools::Itertools;
use ratatui::crossterm::event::Event;
use ratatui::crossterm::event::KeyCode;
use ratatui::crossterm::event::KeyEvent;
use ratatui::crossterm::event::KeyEventKind;
use ratatui::crossterm::event::KeyModifiers;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::symbols;
use ratatui::text::Line;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::HighlightSpacing;
use ratatui::widgets::List;
use ratatui::widgets::ListItem;
use ratatui::widgets::ListState;
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use regex::Regex;
use tui_input::backend::crossterm::EventHandler;
use tui_input::Input;

use crate::histfile::HistEntry;
use crate::raw_pattern_to_regex;
use crate::CaseMode;

const HEADER_FOOTER_STYLE: Style = Style::new().fg(Color::White).bg(Color::Blue);
const SELECTED_STYLE: Style = Style::new()
    .add_modifier(Modifier::REVERSED)
    .add_modifier(Modifier::BOLD);

/// Run the interactive history selector
pub fn run_interactive(
    entries: Vec<HistEntry>,
    inital_search: String,
    exclude_re: Vec<Regex>,
    case_mode: CaseMode,
) -> anyhow::Result<Option<HistEntry>> {
    App::new(entries, inital_search, exclude_re, case_mode).run()
}

/// Representation of the filtered list of HistoryEntry together
/// with the ratatui ListState needed for selection and scrolling
#[derive(Default)]
struct FilteredList<'a> {
    entries: Vec<Rc<HistEntryWrapper<'a>>>,
    state: ListState,
    pagination_num_lines: u16,
}

impl<'a> FilteredList<'a> {
    /// Create a new instance and select the last entry in the list
    fn new(entries: Vec<Rc<HistEntryWrapper<'a>>>) -> Self {
        let mut state = ListState::default();
        state.select_last();
        Self {
            entries,
            state,
            pagination_num_lines: 1,
        }
    }

    /// Return the selected HistoryEntry
    fn get_selected(&self) -> Option<HistEntry> {
        match self.state.selected() {
            Some(idx) if idx < self.entries.len() => Some(self.entries[idx].orig.clone()),
            _ => None,
        }
    }

    fn render(&mut self, area: Rect, frame: &mut Frame) {
        let block = Block::new()
            .title(Line::raw("Interactive History Search").centered())
            .borders(Borders::TOP)
            .border_set(symbols::border::EMPTY)
            .border_style(HEADER_FOOTER_STYLE);

        self.pagination_num_lines = area.height / 2;
        let items = self
            .entries
            .iter()
            .map(|entry| ListItem::from(entry.lines.clone()))
            .collect_vec();
        let list = List::new(items)
            .block(block)
            .highlight_style(SELECTED_STYLE)
            .highlight_symbol("➤")
            .highlight_spacing(HighlightSpacing::Always);

        frame.render_stateful_widget(list, area, &mut self.state);
    }

    fn scroll_up(&mut self) {
        self.state.scroll_up_by(self.pagination_num_lines);
    }

    fn scroll_down(&mut self) {
        self.state.scroll_down_by(self.pagination_num_lines);
    }

    fn select_next(&mut self) {
        self.state.select_next();
    }

    fn select_previous(&mut self) {
        self.state.select_previous();
    }
}

struct App<'a> {
    entries: Vec<Rc<HistEntryWrapper<'a>>>,
    filtered_entries: FilteredList<'a>,
    case_mode: CaseMode,
    search_input: Input,
}

/// The outcome of handling a key event.
enum HandleKeyRes {
    /// Continue processing
    Continue,
    /// We are done. Exit interactive mode. The contained
    /// Option holds the selected entry (if any)
    Return(Option<HistEntry>),
}

impl App<'_> {
    fn new(
        entries: Vec<HistEntry>,
        inital_search: String,
        exclude_re: Vec<Regex>,
        case_mode: CaseMode,
    ) -> Self {
        let mut app = App {
            filtered_entries: FilteredList::default(),
            entries: entries
                .into_iter()
                .filter(|e| e.matches(&[], &exclude_re))
                .map(|e| Rc::new(e.into()))
                .collect_vec(),
            case_mode,
            search_input: Input::new(inital_search),
        };
        app.do_filter();
        app
    }

    fn run(mut self) -> anyhow::Result<Option<HistEntry>> {
        struct DropGuard;
        impl Drop for DropGuard {
            fn drop(&mut self) {
                ratatui::restore();
            }
        }
        let mut terminal = ratatui::init();
        let _guard = DropGuard;
        loop {
            terminal.draw(|frame| self.render(frame))?;
            match ratatui::crossterm::event::read()? {
                Event::Key(ev) if ev.kind == KeyEventKind::Press => match self.handle_key(ev) {
                    HandleKeyRes::Continue => (),
                    HandleKeyRes::Return(maybe_entry) => return Ok(maybe_entry),
                },
                Event::Paste(_ev) => {}
                _ => {}
            }
        }
    }

    /// Conver the current value of the search_input widget into the Vec
    /// of include regexes to filter on
    fn get_include_regexes(&self) -> Vec<Regex> {
        self.search_input
            .value()
            .split_ascii_whitespace()
            .map(|word| {
                // unwrap() is safe, since we are searching for the literal `word`
                raw_pattern_to_regex(&regex::escape(word), self.case_mode).unwrap()
            })
            .collect_vec()
    }

    /// Perform filtering: convert search input to regexes, filter history entries,
    /// create a new `FilteredList` instance for rendering
    fn do_filter(&mut self) {
        let include_re = self.get_include_regexes();
        self.filtered_entries = FilteredList::new(
            self.entries
                .iter()
                .filter(|e| e.matches(&include_re, &[]))
                .cloned()
                .collect_vec(),
        )
    }

    /// Handle a key event
    fn handle_key(&mut self, key: KeyEvent) -> HandleKeyRes {
        match key.code {
            KeyCode::Esc => {
                return HandleKeyRes::Return(None);
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return HandleKeyRes::Return(None)
            }
            KeyCode::Enter => {
                return HandleKeyRes::Return(self.filtered_entries.get_selected());
            }
            KeyCode::Up => self.filtered_entries.select_previous(),
            KeyCode::Down => self.filtered_entries.select_next(),
            KeyCode::PageUp => self.filtered_entries.scroll_up(),
            KeyCode::PageDown => self.filtered_entries.scroll_down(),
            _ => {
                let prev = self.search_input.value().to_owned();
                self.search_input.handle_event(&Event::Key(key));
                if prev != self.search_input.value() {
                    self.do_filter();
                }
            }
        }
        HandleKeyRes::Continue
    }

    /// Render the app screen
    fn render(&mut self, frame: &mut Frame) {
        let [list_area, footer_area] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
        self.filtered_entries.render(list_area, frame);
        self.render_footer(footer_area, frame);
    }

    // Render the footer area with prompt and search input
    fn render_footer(&mut self, area: Rect, frame: &mut Frame) {
        let [prompt_area, input_area] =
            Layout::horizontal([Constraint::Length(1), Constraint::Min(1)]).areas(area);

        frame.render_widget(Paragraph::new(">").style(HEADER_FOOTER_STYLE), prompt_area);
        self.render_search_input(input_area, frame);
    }

    fn render_search_input(&mut self, area: Rect, frame: &mut Frame) {
        // allow one character for the cursor
        let width = area.width.saturating_sub(1);
        let scroll = self.search_input.visual_scroll(width as usize);
        frame.render_widget(
            Paragraph::new(self.search_input.value())
                .style(HEADER_FOOTER_STYLE)
                .scroll((0, scroll as u16)),
            area,
        );
        // Ratatui hides the cursor unless it's explicitly set. Position the  cursor past the
        // end of the input text
        let x = self.search_input.visual_cursor().saturating_sub(scroll);
        frame.set_cursor_position((area.x + x as u16, area.y))
    }
}

/// Wraps a HistoryEntry so it's easier to use as a ratatui `ListItem`
struct HistEntryWrapper<'a> {
    orig: HistEntry,
    /// The ratatui `Line`s that we will use to render this entry in the list
    lines: Vec<Line<'a>>,
}

impl HistEntryWrapper<'_> {
    pub fn matches(&self, include_re: &[Regex], exclude_re: &[Regex]) -> bool {
        self.orig.matches(include_re, exclude_re)
    }
}

impl From<HistEntry> for HistEntryWrapper<'_> {
    fn from(entry: HistEntry) -> Self {
        // We render each entry as the timestamp, followed by the command. For
        // multiline entries, we indent sub-sequent lines so that line is aligned
        // with the command from the first line.
        let ts_str = entry.ts_as_string();
        let indent = ts_str.len() + 1;
        let indent_spaces = std::iter::repeat_n(' ', indent).collect::<String>();
        let mut lines = Vec::new();
        for line in entry.command.lines() {
            let mut s = String::with_capacity(indent + line.len());
            if lines.is_empty() {
                // first line
                s += &ts_str;
                s += " ";
                s += line;
            } else {
                // subsequent lines
                s += &indent_spaces;
                s += line;
            }
            lines.push(Line::raw(s))
        }
        Self { orig: entry, lines }
    }
}

#[cfg(test)]
mod test {

    use chrono::DateTime;
    use chrono::Duration;
    use chrono::Utc;

    use super::*;

    fn newentry(ts: DateTime<Utc>, command: &str) -> HistEntry {
        HistEntry {
            ts,
            command: command.to_owned(),
        }
    }

    fn mk_entries() -> Vec<HistEntry> {
        let t0 = crate::default_ts() + Duration::hours(28);
        let mk_ts = |mins| t0 + Duration::minutes(mins);
        vec![
            newentry(t0, "Lorem Ipsum"),
            newentry(mk_ts(5), "is simply a dummy"),
            newentry(mk_ts(10), "text of the"),
            newentry(mk_ts(23), "printing and typesetting"),
            newentry(mk_ts(42), "industry"),
        ]
    }

    #[test]
    fn test_app_new() {
        fn to_orig(wrapped: Vec<Rc<HistEntryWrapper<'_>>>) -> Vec<HistEntry> {
            wrapped.iter().map(|e| e.orig.clone()).collect_vec()
        }

        let app = App::new(mk_entries(), String::new(), Vec::new(), CaseMode::Sensitive);
        assert_eq!(app.filtered_entries.entries.len(), 5);
        assert_eq!(to_orig(app.filtered_entries.entries), mk_entries());

        // include filter
        let app = App::new(
            mk_entries(),
            "Lorem".to_owned(),
            Vec::new(),
            CaseMode::Sensitive,
        );
        assert_eq!(app.filtered_entries.entries.len(), 1);
        assert_eq!(to_orig(app.entries), mk_entries());
        assert_eq!(
            to_orig(app.filtered_entries.entries),
            vec![mk_entries()[0].clone()]
        );

        // case sensitivity
        let app = App::new(
            mk_entries(),
            "lorem".to_owned(),
            Vec::new(),
            CaseMode::Sensitive,
        );
        assert_eq!(app.filtered_entries.entries.len(), 0);

        // exclude filter
        let app = App::new(
            mk_entries(),
            String::new(),
            vec![Regex::new("simply").unwrap()],
            CaseMode::Sensitive,
        );
        let orig = mk_entries();
        let expected = vec![
            orig[0].clone(),
            orig[2].clone(),
            orig[3].clone(),
            orig[4].clone(),
        ];
        assert_eq!(app.filtered_entries.entries.len(), 4);
        // exclude filter is pre-applied
        assert_eq!(to_orig(app.entries), expected);
        assert_eq!(to_orig(app.filtered_entries.entries), expected);
    }
}
