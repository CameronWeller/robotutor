const $ = (s) => document.querySelector(s);
let profile = sessionStorage.profile || "teacher",
  state,
  current,
  draft;
$("#profile").value = profile;
function node(tag, text, parent) {
  const e = document.createElement(tag);
  if (text != null) e.textContent = text;
  if (parent) parent.append(e);
  return e;
}
function button(parent, label, fn) {
  const b = node("button", label, parent);
  b.onclick = async () => {
    b.disabled = true;
    try {
      await fn();
    } catch (e) {
      notice(e.message);
    } finally {
      b.disabled = false;
    }
  };
  return b;
}
let dirty = false;
function notice(text) {
  $("#notice").textContent = text;
}
async function api(action, fields = {}) {
  const r = await fetch("/api", {
    method: "POST",
    headers: { "Content-Type": "application/json", "X-Profile": profile },
    body: JSON.stringify({ action, ...fields }),
  });
  const data = await r.json();
  if (!r.ok) throw Error(data.error);
  return data;
}
async function refresh() {
  state = await api("state");
  if (!state.courses.some((c) => c.id === current))
    current = state.courses[0]?.id;
  render();
}
function proposalRequest() {
  let last, token;
  return fields => {
    const packet = JSON.stringify(fields);
    if (packet !== last) {
      token = crypto.randomUUID();
      last = packet;
    }
    return api("propose", {...fields, token});
  };
}
async function change(action, fields) {
  await api(action, fields);
  await refresh();
}
function field(parent, label, obj, key, multiline = false, type = "text") {
  const wrap = node("label", label, parent);
  wrap.className = "field";
  const input = node(multiline ? "textarea" : "input", null, wrap);
  if (!multiline) input.type = type;
  input.value = obj[key] || "";
  input.oninput = () => {
    dirty = true;
    obj[key] = input.value;
  };
  return input;
}
function move(parent, items, i, redraw) {
  for (const [label, delta] of [
    ["Move up", -1],
    ["Move down", 1],
  ]) {
    const j = i + delta;
    if (j >= 0 && j < items.length)
      button(parent, label, () => {
        [items[i], items[j]] = [items[j], items[i]];
        redraw();
      });
  }
}
function outline(parent, o, open) {
  for (const u of o.units) {
    node("h3", u.title, parent);
    for (const a of u.activities) {
      const p = node("section", null, parent);
      node("strong", a.title, p);
      if (open) button(p, u === o.units[0] && a === u.activities[0] ? "Open first activity" : "Open activity", () => open(o.units.indexOf(u), u.activities.indexOf(a)));
      node("p", a.instructions, p);
      node("p", "Objective · " + a.objective, p);
      node("p", "Submission criterion · " + a.criterion, p);
    }
  }
}
function edit(root) {
  node("h1", current ? "Edit course" : "Create course", root);
  field(root, "Course title", draft, "title");
  field(root, "Course destination", draft, "destination", true);
  draft.units.forEach((u, i) => {
    const box = node("fieldset", null, root);
    node("legend", "Unit " + (i + 1), box);
    field(box, "Unit title", u, "title");
    move(box, draft.units, i, render);
    u.activities.forEach((a, j) => {
      const sub = node("fieldset", null, box);
      node("legend", "Activity " + (j + 1), sub);
      for (const [label, key] of [
        ["Activity title", "title"],
        ["Instructions", "instructions"],
        ["Learning objective", "objective"],
        ["Submission criterion", "criterion"],
      ])
        field(sub, label, a, key, key !== "title");
      move(sub, u.activities, j, render);
    });
    button(box, "Add activity", () => {
      u.activities.push({
        key: crypto.randomUUID(), due: null,
        title: "",
        instructions: "",
        objective: "",
        criterion: "",
      });
      render();
    });
  });
  button(root, "Add unit", () => {
    draft.units.push({ title: "", activities: [] });
    render();
  });
  button(root, "Preview", () => {
    const dialog = node("dialog", null, root);
    node("h2", draft.title, dialog);
    node("p", draft.destination, dialog);
    outline(dialog, draft);
    button(dialog, "Close", () => dialog.remove());
    dialog.showModal();
  });
  const sendProposal = proposalRequest();
  const proposal = state.courses.find(c => c.id === current)?.proposal;
  button(root, proposal ? "Check and release changes" : "Save draft", async () => {
    if (proposal) notice("Checking your edits…");
    const saved = proposal
      ? await sendProposal({id: current, edited: draft, revision: proposal.revision})
      : await api("save", { id: current, outline: draft });
    current = saved.id;
    draft = null;
    await refresh();
  });
  button(root, "Cancel", () => {
    draft = null;
    refresh();
  });
}
function assignment(parent, a, teacher) {
  const box = node("section", null, parent);
  node("h3", a.activity.title, box);
  node("p", a.unit + " · Published version " + a.version, box);
  node("p", a.status, box).className = "status";
  if (teacher) {
    const due = { due: a.due };
    field(box, "Due date", due, "due", false, "date");
    button(box, "Update due date", () => change("due", { id: a.id, ...due }));
  } else {
    if (a.due) node("p", "Due " + a.due, box);
    node("p", a.activity.instructions, box);
    node("p", "Submit · " + a.activity.criterion, box);
    const open = node("a", "Open in Robotutor", box);
    open.className = "open";
    open.href = "robotutor://assignment/" + a.id;
  }
  for (const [i, s] of a.submissions.entries()) {
    node("h4", "Submission " + (i + 1), box);
    node("pre", s.work, box);
    if (s.decision)
      node("p", s.decision + (s.feedback ? " · " + s.feedback : ""), box);
  }
  if (teacher && a.status === "Awaiting review") {
    const review = { feedback: "" };
    field(box, "Teacher feedback", review, "feedback", true);
    for (const decision of ["Revision requested", "Accepted"])
      button(
        box,
        decision === "Accepted" ? "Accept work" : "Request revision",
        () =>
          change("review", {
            id: a.id,
            revision: a.submissions.at(-1).id,
            decision,
            ...review,
          }),
      );
  }
}
function render() {
  dirty = false;
  notice("");
  const root = $("#course"),
    nav = $("nav");
  root.replaceChildren();
  nav.replaceChildren();
  const teacher = profile === "teacher";
  $("#new").hidden = !teacher;
  $("#propose").hidden = !teacher;
  for (const c of state.courses)
    button(nav, c.outline.title, () => {
      proposing = false;
      draft = null;
      current = c.id;
      render();
    });
  if (proposing && teacher) {
    node("h1", "Propose a course", root);
    proposalForm(root, null);
    return;
  }
  if (draft && teacher) {
    edit(root);
    return;
  }
  const c = state.courses.find((x) => x.id === current);
  if (!c) {
    node("h1", teacher ? "Your courses" : "My courses", root);
    node(
      "p",
      teacher
        ? "Describe who you teach and what you want them to learn."
        : "Your enrolled courses appear here.",
      root,
    );
    if (!teacher) state.assignments.forEach((a) => assignment(root, a, false));
    return;
  }
  node("h1", c.outline.title, root);
  node("p", c.outline.destination, root);
  if (teacher) {
    if (!c.archived) {
      button(root, "Edit draft", () => {
        draft = structuredClone(c.outline);
        render();
      });
      if (c.proposal) {
        node("p", c.proposal.changes, root);
        if (c.proposal.problem) node("p", "Needs revision · " + c.proposal.problem, root);
        proposalForm(root, c);
      } else button(root, "Publish version", () => change("publish", { id: c.id }));
    }
    button(root, c.archived ? "Reopen course" : "Archive course", () =>
      change("archive", { id: c.id, archived: !c.archived }),
    );
  }
  node("h2", "Course outline", root);
  outline(root, c.outline, teacher || c.archived ? null : async (unit, activity) => {
    const opened = await api("open_activity", {version: c.version, unit, activity});
    location.href = "robotutor://assignment/" + opened.id;
    await refresh();
  });
  if (teacher) {
    node("h2", "Roster", root);
    node("p", c.enrolled ? "Learner · enrolled" : "No learners enrolled", root);
    if (!c.archived)
      button(root, c.enrolled ? "Remove learner" : "Enroll learner", () =>
        change(c.enrolled ? "remove" : "enroll", { id: c.id }),
      );
    if (c.published && c.enrolled && !c.archived) {
      node("h2", "Assign published activity", root);
      const pick = node("select", null, root);
      pick.setAttribute("aria-label", "Published activity");
      c.published.units.forEach((u, i) =>
        u.activities.forEach((a, j) => {
          node("option", u.title + " · " + a.title, pick).value = i + "," + j;
        }),
      );
      const due = { due: "" };
      field(root, "Assignment due date", due, "due", false, "date");
      button(root, "Assign to learner", () => {
        const [unit, activity] = pick.value.split(",").map(Number);
        return change("assign", {
          id: c.id,
          version: c.version,
          unit,
          activity,
          ...due,
        });
      });
    }
  }
  node("h2", "Assignments", root);
  state.assignments
    .filter((a) => a.course === c.id)
    .forEach((a) => assignment(root, a, teacher));
}
let proposing = false;
function proposalForm(root, course) {
  const input = {brief: ""};
  const sendProposal = proposalRequest();
  const entry = field(root, course ? "Requested change" : "Course brief", input, "brief", true);
  entry.rows = course ? 2 : 4;
  if (!course) entry.placeholder = "Teach fractions to my fifth-grade class using cooking examples.";
  button(root, course ? "Revise course" : "Generate proposal", async () => {
    if (!input.brief.trim()) {entry.focus(); return;}
    notice("Drafting and checking the course…");
    const result = await sendProposal({...input, id: course?.id, revision: course?.proposal.revision});
    proposing = false;
    current = result.id;
    await refresh();
  });
}
$("#propose").onclick = () => {
  proposing = true;
  draft = null;
  render();
};
$("#new").onclick = () => {
  proposing = false;
  current = null;
  draft = { title: "", destination: "", units: [] };
  render();
};
$("#profile").onchange = () => {
  proposing = false;
  profile = $("#profile").value;
  sessionStorage.profile = profile;
  current = null;
  draft = null;
  refresh().catch((e) => notice(e.message));
};
async function checkUpdates() {
  if (proposing || draft || dirty || document.hidden) return;
  const requested = profile;
  const updated = await api("state");
  if (
    profile === requested &&
    !draft &&
    !dirty &&
    JSON.stringify(updated) !== JSON.stringify(state)
  ) {
    state = updated;
    if (!state.courses.some((c) => c.id === current))
      current = state.courses[0]?.id;
    render();
  }
}
window.addEventListener("focus", () =>
  checkUpdates().catch((e) => notice(e.message)),
);
setInterval(() => checkUpdates().catch((e) => notice(e.message)), 5000);
refresh().catch((e) => notice(e.message));
