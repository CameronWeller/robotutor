// Run against an empty disposable pilot database. Optional --review or --accept uses its existing assignment.
const { chromium } = require("playwright");
const assert = require("node:assert/strict");
(async () => {
  const browser = await chromium.launch({
    headless: true,
    executablePath: process.env.CHROMIUM_BIN,
  });
  try {
    const page = await browser.newPage();
    await page.goto(process.env.PILOT_URL || "http://127.0.0.1:47322");
    const click = (text) =>
      page.getByRole("button", { name: text, exact: true }).click();
    const fill = (name, value) =>
      page.getByLabel(name, { exact: true }).fill(value);
    if (process.argv[2]) {
      const returning = process.argv[2] === "--review";
      await fill(
        "Teacher feedback",
        returning ? "Explain the second beat." : "Good revision.",
      );
      await click(returning ? "Request revision" : "Accept work");
      await page
        .getByText(returning ? "Revision requested" : "Accepted", {
          exact: true,
        })
        .first()
        .waitFor();
      await page.getByLabel("Local profile").selectOption("learner");
      await page
        .getByText(returning ? "Revision requested" : "Accepted", {
          exact: true,
        })
        .first()
        .waitFor();
      assert.ok(
        await page
          .getByText(
            returning ? "Explain the second beat." : "Good revision.",
            { exact: false },
          )
          .count(),
      );
      if (!returning) {
        await page.getByLabel("Local profile").selectOption("teacher");
        await click("Archive course");
        await page.getByRole("button", { name: "Reopen course" }).waitFor();
        await page.getByText("Good revision.", { exact: false }).waitFor();
        await click("Reopen course");
        await click("Remove learner");
        await page.getByText("No learners enrolled", { exact: true }).waitFor();
        await click("Enroll learner");
      }
      return;
    }
    await click("Create manually");
    await fill("Course title", "Rhythm workshop");
    await fill("Course destination", "Read a rhythm aloud");
    await click("Add unit");
    await fill("Unit title", "Pulse");
    await click("Add activity");
    for (const [k, v] of Object.entries({
      "Activity title": "Count a measure",
      Instructions: "Mark four beats.",
      "Learning objective": "Identify the pulse",
      "Submission criterion": "Share a four-beat count",
    }))
      await fill(k, v);
    await click("Preview");
    await page.getByRole("dialog").getByText("Mark four beats.").waitFor();
    await click("Close");
    await click("Save draft");
    await click("Publish version");
    await click("Enroll learner");
    await page.getByLabel("Assignment due date").fill("2027-01-05");
    await click("Assign to learner");
    await page.getByText("Assigned", { exact: true }).waitFor();
    await click("Edit draft");
    await fill("Submission criterion", "Publish a six-beat count");
    await click("Save draft");
    await click("Publish version");
    await page.getByLabel("Local profile").selectOption("learner");
    await page
      .getByText("Submit · Share a four-beat count", { exact: true })
      .waitFor();
    assert.equal(
      await page
        .getByRole("link", { name: "Open in Robotutor" })
        .getAttribute("href"),
      "robotutor://assignment/1",
    );
  } finally {
    await browser.close();
  }
})().catch((e) => {
  console.error(e);
  process.exitCode = 1;
});
