use crate::display;
use std::io::{self, IsTerminal};
use textwrap::wrap;

pub enum Layout {
    Plain,
    Panel { columns: usize, color: bool },
}

impl Layout {
    pub fn interactive() -> Self {
        if !io::stdout().is_terminal() {
            return Self::Plain;
        }
        let columns = crossterm::terminal::size()
            .ok()
            .map(|(w, _)| usize::from(w))
            .filter(|w| *w > 0)
            .unwrap_or(80);
        let color = std::env::var_os("TERM").is_some_and(|v| !v.is_empty() && v != "dumb")
            && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
        Self::Panel { columns, color }
    }

    pub fn render(
        &self,
        fields: &[(&str, &str)],
        decision: Option<String>,
        reply: Option<&str>,
    ) -> String {
        let Self::Panel { columns, color } = *self else {
            let mut lines: Vec<_> = fields
                .iter()
                .map(|(label, value)| format!("{label} · {value}"))
                .collect();
            if let Some(json) = decision {
                lines.push(format!("Full decision\n{json}"));
            }
            if let Some(reply) = reply {
                lines.push(format!("Delivered › {reply}"));
            }
            return display(&lines.join("\n"));
        };

        let paint = |style: &str, text: &str| {
            if color {
                format!("\x1b[{style}m{text}\x1b[0m")
            } else {
                text.to_owned()
            }
        };
        let rail = paint("36", "│");
        let mut lines = vec![String::new(), paint("1;36", "╭─ Teaching trace")];
        let column_layout = columns >= 64;
        let label_width = fields
            .iter()
            .map(|(label, _)| label.len())
            .max()
            .unwrap_or(0);
        let indent = if column_layout { label_width + 2 } else { 2 };
        let continuation = format!("{rail} {:indent$}", "");
        let width = columns.saturating_sub(2 + indent).max(1);

        for &(label, value) in fields {
            let heading = paint(if label == "Changed" { "1;32" } else { "1" }, label);
            let first = if column_layout {
                let gap = indent - label.len();
                format!("{rail} {heading}{:gap$}", "")
            } else {
                lines.push(format!("{rail} {heading}"));
                continuation.clone()
            };
            let value = display(value).replace('\t', "    ");
            for (i, line) in wrap(&value, width).into_iter().enumerate() {
                let prefix = if i == 0 { &first } else { &continuation };
                lines.push(format!("{prefix}{line}"));
            }
        }
        for (title, body) in [
            ("Full decision", decision.as_deref()),
            ("Delivered reply", reply),
        ] {
            if let Some(body) = body {
                lines.push(rail.clone());
                lines.push(paint("1;36", &format!("├─ {title}")));
                let body = display(body).replace('\t', "    ");
                for line in wrap(&body, columns.saturating_sub(2).max(1)) {
                    lines.push(format!("{rail} {line}"));
                }
            }
        }
        lines.push(paint("36", "╰─"));
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_trace_aligns_values_and_resets_color_before_body_text() {
        let trace = Layout::Panel {
            columns: 80,
            color: true,
        }
        .render(
            &[("Changed", "New task"), ("What happened next", "pending")],
            None,
            None,
        );
        assert!(trace.starts_with("\n\x1b[1;36m╭─ Teaching trace\x1b[0m"));
        assert!(trace.contains("\x1b[1;32mChanged\x1b[0m             New task"));
        assert!(trace.contains("\x1b[1mWhat happened next\x1b[0m  pending"));
        assert!(trace.ends_with("╰─\x1b[0m"));
    }

    #[test]
    fn narrow_monochrome_trace_wraps_fields_and_details_in_terminal_cells() {
        let trace = Layout::Panel {
            columns: 28,
            color: false,
        }
        .render(
            &[
                ("Observed", "引用".repeat(12).as_str()),
                ("Changed", "a new task"),
            ],
            Some("{\n  \"note\": \"a sentence longer than one line\"\n}".into()),
            Some("First line\nSecond line\x1b[31m"),
        );
        assert!(trace.contains("│ Observed\n│   引用"));
        assert!(trace.contains("│ Changed\n│   a new task"));
        assert!(trace.contains("├─ Full decision\n│ {\n│   \"note\""));
        assert!(trace.contains("├─ Delivered reply\n│ First line\n│ Second line"));
        assert!(!trace.contains('\x1b'));
        for line in trace.lines() {
            assert!(textwrap::core::display_width(line) <= 28, "{line}");
        }
    }
}
