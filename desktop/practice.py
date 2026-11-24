"""Keyboard-accessible GTK projection of the Rust activity fields."""
from gi.repository import Gtk


class ActivityEditor(Gtk.Box):
    def __init__(self, changed, command, import_document):
        super().__init__(orientation=Gtk.Orientation.VERTICAL, spacing=3)
        self.changed = changed
        self.command = command
        self.import_document = import_document
        self.widgets = {}
        self.graph = None
        self.version = None

    def values(self):
        values = {}
        for key, widget in self.widgets.items():
            if isinstance(widget, Gtk.ComboBoxText):
                values[key] = widget.get_active_text() or ""
            elif isinstance(widget, Gtk.TextView):
                buf = widget.get_buffer()
                values[key] = buf.get_text(buf.get_start_iter(), buf.get_end_iter(), True)
            else:
                values[key] = widget.get_text()
        return values


    def restore(self, form, draft):
        version = form["version"] if form else None
        if version != self.version:
            self.rebuild(form)
            self.version = version
        for key, widget in self.widgets.items():
            value = draft["fields"].get(key, "")
            if isinstance(widget, Gtk.ComboBoxText):
                if widget.get_active_text() != value:
                    widget.set_active_id(value) if value else widget.set_active(-1)
            elif isinstance(widget, Gtk.TextView):
                if self.values()[key] != value:
                    widget.get_buffer().set_text(value)
            elif widget.get_text() != value:
                widget.set_text(value)
        if self.graph:
            self.graph.queue_draw()
        if form and form["id"] == "perform":
            self.result.set_text("Intervals (ms)  " + " · ".join(map(str, draft["intervals"])))
        if form and form["id"] == "measure":
            self.result.set_text("   ".join(f"{x} cm → {y:g} m" for x, y in draft["measurements"] or []))

    def rebuild(self, form):
        while self.get_first_child():
            self.remove(self.get_first_child())
        self.widgets = {}
        self.graph = None
        if not form:
            return
        grid = Gtk.Grid(column_spacing=8, row_spacing=3)
        self.append(grid)
        for i, field in enumerate(form["fields"]):
            if field["kind"] == "timing":
                button = Gtk.Button(label="Tap beat")
                button.connect("clicked", lambda *_: self.command("/tap"))
                grid.attach(button, 0, i, 2, 1)
                continue
            label = Gtk.Label(label=field["label"], xalign=0, wrap=True)
            grid.attach(label, 0, i, 1, 1)
            if field["choices"]:
                widget = Gtk.ComboBoxText(hexpand=True)
                for value in field["choices"]:
                    widget.append(value, value)
            elif field["kind"] in {"rubric", "document"}:
                widget = Gtk.TextView(hexpand=True, wrap_mode=Gtk.WrapMode.WORD_CHAR)
                widget.set_size_request(-1, 55)
            else:
                widget = Gtk.Entry(hexpand=True)
                if field["kind"] == "point":
                    widget.set_placeholder_text("x,y")
            label.set_mnemonic_widget(widget)
            widget.set_tooltip_text(field["label"])
            if isinstance(widget, Gtk.TextView):
                widget.get_buffer().connect("changed", self.on_change)
            else:
                widget.connect("changed", self.on_change)
            grid.attach(widget, 1, i, 1, 1)
            self.widgets[field["id"]] = widget
        self.points = [f["id"] for f in form["fields"] if f["kind"] == "point"]
        if self.points:
            self.graph = Gtk.DrawingArea(content_width=220, content_height=145)
            self.graph.set_draw_func(self.draw)
            click = Gtk.GestureClick()
            click.connect("pressed", self.plot)
            self.graph.add_controller(click)
            self.point_picker = Gtk.ComboBoxText()
            for field in form["fields"]:
                if field["id"] in self.points:
                    self.point_picker.append(field["id"], "Plot · " + field["label"])
            self.point_picker.set_active(0)
            self.append(self.point_picker)
            self.append(self.graph)
        if form["id"] in {"annotate", "revise", "revision"}:
            load = Gtk.Button(label="Open a text document", halign=Gtk.Align.START)
            load.connect("clicked", lambda *_: self.import_document())
            self.append(load)
        if form["id"] == "measure":
            row = Gtk.Box(spacing=5)
            lengths = [Gtk.SpinButton.new_with_range(1, 100, 1) for _ in range(2)]
            lengths[1].set_value(2)
            for n in lengths:
                row.append(n)
            run = Gtk.Button(label="Run simulation (cm)")
            run.connect("clicked", lambda *_: self.command(
                f"/run {lengths[0].get_value_as_int()} {lengths[1].get_value_as_int()}"))
            row.append(run)
            self.append(row)
        if form["id"] in {"measure", "perform"}:
            self.result = Gtk.Label(xalign=0, selectable=True)
            self.append(self.result)
        hint = Gtk.Button(label="Ask for a hint", halign=Gtk.Align.START)
        hint.connect("clicked", lambda *_: self.command("/hint"))
        self.append(hint)

    def on_change(self, *_):
        if self.graph:
            self.graph.queue_draw()
        self.changed()

    def plot(self, _gesture, _count, x, y):
        w, h = self.graph.get_width(), self.graph.get_height()
        px = round((x - 32) * 10 / max(1, w - 46))
        py = round((h - 21 - y) * 10 / max(1, h - 35))
        if 0 <= px <= 10 and 0 <= py <= 10:
            self.widgets[self.point_picker.get_active_id()].set_text(f"{px},{py}")
            self.point_picker.set_active((self.point_picker.get_active() + 1) % len(self.points))

    def draw(self, _area, cr, w, h):
        theme = _area.get_style_context()
        def rgb(name):
            found, color = theme.lookup_color("tutor_" + name)
            if not found:
                color = theme.get_color()
            return color.red, color.green, color.blue
        primary, secondary, highlight = (rgb(role) for role in ("primary", "secondary", "highlight"))
        cr.set_source_rgb(*secondary)
        cr.paint()
        def coord(x, y):
            return 32 + x * (w - 46) / 10, h - 21 - y * (h - 35) / 10
        for i in range(11):
            cr.set_source_rgba(*primary, .55)
            cr.move_to(*coord(i, 0))
            cr.line_to(*coord(i, 10))
            cr.move_to(*coord(0, i))
            cr.line_to(*coord(10, i))
            cr.stroke()
            if i % 2 == 0:
                x, y = coord(i, 0)
                cr.set_source_rgb(*primary)
                cr.move_to(x - 3, y + 15)
                cr.show_text(str(i))
                x, y = coord(0, i)
                cr.move_to(x - 18, y + 3)
                cr.show_text(str(i))
        for key in self.points:
            try:
                x, y = map(int, self.widgets[key].get_text().strip("()").split(","))
                if 0 <= x <= 10 and 0 <= y <= 10:
                    cr.arc(*coord(x, y), 4, 0, 6.284)
                    cr.set_source_rgb(*highlight)
                    cr.fill_preserve()
                    cr.set_source_rgb(*primary)
                    cr.stroke()
            except ValueError:
                pass
