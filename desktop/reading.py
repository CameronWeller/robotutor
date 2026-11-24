"""Native, selectable presentation of learner-visible tutor text."""
import re

from gi.repository import Gtk, Pango


# Only separate a recognizable request in existing plain prose. Fixed prompts are
# kept verbatim in the journal and used as-is for grading and activity selection.
REQUEST = re.compile(
    r"(?:What|Which|How|Why|Where|When|Who|Can you|Would you|"
    r"Choose|Compare|Describe|Explain|Fill|Identify|Plot|Predict|Select|Show|Write)\b"
)
REQUEST_BREAK = re.compile(r"(?<=[.!?])\s+(?=" + REQUEST.pattern + r")")
INLINE = re.compile(r"(`[^`\n]+`|\*\*(?!\s).+?(?<!\s)\*\*)")
BULLET = re.compile(r"^(\s*)([-*+] |\d+[.)] )(\S.*)$")
HEADING = re.compile(r"^#{1,6}\s+(.+)$")
FENCE = re.compile(r"^\s*(`{3,}|~{3,})(.*)$")


class ReadingView(Gtk.TextView):
    def __init__(self):
        super().__init__(editable=False, cursor_visible=False, wrap_mode=Gtk.WrapMode.WORD_CHAR,
                         left_margin=16, right_margin=16, top_margin=8, bottom_margin=8,
                         pixels_inside_wrap=2)
        buf = self.get_buffer()
        buf.create_tag("speaker", weight=Pango.Weight.BOLD, scale=0.93, pixels_above_lines=9)
        buf.create_tag("heading", weight=Pango.Weight.BOLD, scale=1.15, pixels_above_lines=5)
        buf.create_tag("ask", weight=Pango.Weight.SEMIBOLD, scale=1.05, pixels_above_lines=4)
        buf.create_tag("list-item", left_margin=30, indent=-14)
        buf.create_tag("emphasis", weight=Pango.Weight.BOLD)
        buf.create_tag("code", family="monospace")
        self.question = None

    def write(self, text, *tags):
        buf = self.get_buffer()
        buf.insert_with_tags_by_name(buf.get_end_iter(), text, *tags)

    def inline(self, text, *tags):
        for part in INLINE.split(text):
            if part.startswith("`") and INLINE.fullmatch(part):
                self.write(part[1:-1], *tags, "code")
            elif part.startswith("**") and INLINE.fullmatch(part):
                self.write(part[2:-2], *tags, "emphasis")
            elif part:
                self.write(part, *tags)

    def format_tutor(self, text, prompt="", objective=""):
        # Older replies sometimes join the exact exercise to the explanation
        # with a single space. Recover that boundary using the saved prompt.
        text = text.rstrip("\n")
        prompt = prompt.strip()
        task_at = len(text)
        if prompt and text.endswith(prompt):
            before = text[:-len(prompt)]
            if not before or before.endswith("\n"):
                task_at = len(before)
            elif before[-1].isspace() and before.rstrip().endswith((".", "!", "?")):
                before = before.rstrip() + "\n\n"
                text = before + prompt
                task_at = len(before)
        offset = 0
        fence = ""
        for line in text.split("\n"):
            in_task = offset >= task_at
            offset += len(line) + 1
            marker = FENCE.fullmatch(line)
            if fence:
                if marker and marker[1].startswith(fence) and not marker[2].strip():
                    fence = ""
                else:
                    self.write(line + "\n", "code")
                continue
            if marker and marker[1][0] not in marker[2]:
                fence = marker[1]
                continue
            heading = HEADING.fullmatch(line)
            bullet = BULLET.fullmatch(line)
            if line.startswith(("    ", "\t")):
                self.write(line, "code")
            elif heading or (objective and line.strip() == objective.strip()):
                self.inline(heading[1] if heading else line, "heading")
            elif bullet:
                lead = "• " if bullet[2][0] in "-*+" else bullet[2]
                self.inline(bullet[1] + lead + bullet[3], "list-item")
            else:
                # Leave quoted material, inline code and source markup intact.
                split = REQUEST_BREAK.search(line) if in_task and not any(c in line for c in '`"“”<>') else None
                if split:
                    self.inline(line[:split.start()])
                    self.write("\n")
                    self.inline(line[split.end():], "ask")
                else:
                    tags = ("ask",) if in_task and (REQUEST.match(line) or line.endswith("?")) else ()
                    self.inline(line, *tags)
            self.write("\n")

    def set_question(self, text):
        if text == self.question:
            return
        self.question = text
        self.get_buffer().set_text("")
        if text:
            self.format_tutor(text, prompt=text)
            buf = self.get_buffer()
            # format_tutor terminates chat entries with a newline. A standalone
            # question ends on its final rendered line.
            end = buf.get_end_iter()
            last = end.copy()
            if last.backward_char() and last.get_char() == "\n":
                buf.delete(last, end)
            # A new exercise always starts at the top, even after a long prior task.
            mark = buf.get_mark("question-start")
            if mark is None:
                mark = buf.create_mark("question-start", buf.get_start_iter(), True)
            self.scroll_to_mark(mark, 0, True, 0, 0)

    def get_text(self):
        return self.question or ""
