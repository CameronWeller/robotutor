//! The interactive SSH composer saves edits before waiting for another key.
use crate::{practice, Journal, Visit};
use crossterm::{
    cursor::MoveToColumn,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{self, Clear, ClearType},
};
use std::{
    io::{self, Write},
    time::Duration,
};
struct Raw;
impl Drop for Raw {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}
fn draw(buffer: &[char], cursor: usize) -> io::Result<()> {
    use textwrap::core::display_width;
    let width = terminal::size()
        .map_or(80, |w| usize::from(w.0))
        .saturating_sub(8)
        .max(3);
    let printable = |s: &[char]| {
        s.iter()
            .map(|c| if c.is_control() { ' ' } else { *c })
            .collect::<String>()
    };
    let mut left = cursor;
    let mut cells = 0;
    while left > 0 {
        let n = display_width(&printable(&buffer[left - 1..left]));
        if cells + n >= width {
            break;
        }
        cells += n;
        left -= 1;
    }
    let mut visible = printable(&buffer[left..cursor]);
    let mut count = cells;
    for c in &buffer[cursor..] {
        let s = printable(std::slice::from_ref(c));
        count += display_width(&s);
        if count >= width {
            break;
        }
        visible.push_str(&s);
    }
    let mut out = io::stdout();
    execute!(out, MoveToColumn(0), Clear(ClearType::CurrentLine))?;
    write!(out, "You › {visible}")?;
    execute!(out, MoveToColumn((6 + cells) as u16))?;
    out.flush()
}
pub fn read(
    visit: &mut Visit,
    journal: &mut Option<Journal>,
    installed: &mut crate::terminal::Installed,
) -> Result<Option<String>, String> {
    terminal::enable_raw_mode().map_err(|e| e.to_string())?;
    let raw = Raw;
    let mut buffer: Vec<char> = visit.draft.text.chars().collect();
    let mut cursor = buffer.len();
    loop {
        draw(&buffer, cursor).map_err(|e| e.to_string())?;
        if !event::poll(Duration::from_secs(1)).map_err(|e| e.to_string())? {
            if installed.replaced() {
                drop(raw);
                println!("\nReopening the updated tutor with your saved draft.");
                return Err(installed.reload().to_string());
            }
            continue;
        }
        let Event::Key(key) = event::read().map_err(|e| e.to_string())? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('d' | 'c') if ctrl => {
                drop(raw);
                println!();
                return Ok(None);
            }
            KeyCode::F(6) => {
                drop(raw);
                println!();
                return Ok(Some("/tap".into()));
            }
            KeyCode::Enter => {
                let text: String = buffer.into_iter().collect();
                drop(raw);
                println!();
                if !text.starts_with('/') && practice::current(visit).is_some() {
                    return Ok(Some("/send".into()));
                }
                return Ok(Some(text));
            }
            KeyCode::Esc => {
                buffer.clear();
                cursor = 0;
                continue;
            }
            KeyCode::Left => {
                cursor = cursor.saturating_sub(1);
                continue;
            }
            KeyCode::Right => {
                cursor = (cursor + 1).min(buffer.len());
                continue;
            }
            KeyCode::Home | KeyCode::Char('a') if ctrl || key.code == KeyCode::Home => {
                cursor = 0;
                continue;
            }
            KeyCode::End | KeyCode::Char('e') if ctrl || key.code == KeyCode::End => {
                cursor = buffer.len();
                continue;
            }
            KeyCode::Char('u') if ctrl => {
                buffer.drain(..cursor);
                cursor = 0;
            }
            KeyCode::Backspace if cursor > 0 => {
                cursor -= 1;
                buffer.remove(cursor);
            }
            KeyCode::Delete if cursor < buffer.len() => {
                buffer.remove(cursor);
            }
            KeyCode::Char(c) if !ctrl && !c.is_control() && buffer.len() < 32000 => {
                buffer.insert(cursor, c);
                cursor += 1
            }
            _ => continue,
        }
        if buffer.first() != Some(&'/') && visit.pending.is_none() {
            let text: String = buffer.iter().collect();
            let draft = practice::set_fields(visit, text, visit.draft.fields.clone())?;
            practice::edit(visit, journal, draft)?;
        }
    }
}
