// Run in a disposable lesson using only synthetic content.
const assert = require('node:assert/strict');
const {execFileSync} = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const {app, session} = require('electron');
const {open} = require('./main.cjs');
const workspace = process.argv[2];
const helper = process.env.ROBOTUTOR_RESEARCH_HELPER;
function rows() {
  return JSON.parse(execFileSync(helper, ['--notebook', workspace], {
    input: '{"action":"list"}', encoding:'utf8'}));
}
async function until(check, label) {
  for (let n = 0; n < 100; n++) {
    if (await check()) return;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  throw Error('Timed out waiting for ' + label);
}
app.whenReady().then(async () => {
  session.fromPartition('persist:research').protocol.handle('https', () =>
    new Response('<main><h1>Fictional astronomy resource</h1></main>', {headers:{'content-type':'text/html'}}));
  const {win, toolbar, web} = await open();
  const click = id => toolbar.executeJavaScript('document.getElementById(' + JSON.stringify(id) + ').click()');
  const go = async url => {
    await toolbar.executeJavaScript('document.getElementById("address").value=' + JSON.stringify(url) +
      '; document.getElementById("nav").requestSubmit()');
    await until(() => web.getURL() === url && !web.isLoading(), 'page delivery');
  };
  const article = 'https://en.wikipedia.org/wiki/Fictional_Astronomy?example=private';
  await go(article);
  assert.equal(await web.executeJavaScript('typeof process'), 'undefined');
  assert.deepEqual(rows(), []);
  await click('toggle');
  await until(() => rows().length === 1, 'opted-in article record');
  assert.equal(rows()[0].resource,'https://en.wikipedia.org/wiki/Fictional_Astronomy');
  assert.deepEqual(rows()[0].available_headings, ['Fictional astronomy resource']);
  await go('https://accounts.example.org/settings');
  assert.equal(rows().length, 1);
  await click('toggle');
  await go('https://science.nasa.gov/earth/sample');
  assert.equal(rows().length, 1);
  await click('notebook');
  await until(() => toolbar.executeJavaScript('document.querySelectorAll("#rows li").length === 1'), 'review');
  for (const [width,height] of [[1040,750],[560,440]]) {
    win.setContentSize(width,height);
    await until(() => toolbar.executeJavaScript('innerWidth === '+width), 'resizing');
    assert.ok(await toolbar.executeJavaScript(
      'document.getElementById("clear").getBoundingClientRect().right <= innerWidth'));
    if (process.env.ROBOTUTOR_CAPTURE_RESEARCH === '1') {
      const capture = await toolbar.capturePage();
      fs.writeFileSync(path.join(workspace, 'notebook-' + width + '.png'), capture.toPNG());
    }
  }
  await click('toggle');
  await until(() => toolbar.executeJavaScript('document.getElementById("toggle").getAttribute("aria-pressed") === "true"'), 'recording enabled');
  assert.equal(rows().length, 1, 'starting recording in the notebook must not record the hidden page');
  for (const [action, target] of [
    ['back', 'https://accounts.example.org/settings'],
    ['forward', 'https://science.nasa.gov/earth/sample'],
    ['reload', 'https://science.nasa.gov/earth/sample'],
  ]) {
    let loaded = false;
    web.once('did-finish-load', () => { loaded = true; });
    await toolbar.executeJavaScript('document.querySelector("[data-action=' + action + ']").click()');
    await until(() => loaded, action + ' navigation');
    assert.equal(web.getURL(), target);
    assert.ok(win.contentView.children[0].getVisible(), action + ' should reveal the resource');
    await until(() => toolbar.executeJavaScript('document.getElementById("book").hidden'), 'notebook dismissed');
    if (action === 'back') assert.equal(rows().length, 1);
    else await until(() => rows().length === 2, 'visible resource recorded');
    await click('notebook');
    await until(() => !win.contentView.children[0].getVisible(), 'notebook opened');
  }
  await click('clear');
  await toolbar.executeJavaScript('document.querySelector("[data-action=clear]").click()');
  await until(() => rows().length === 0, 'confirmed deletion');
  console.log('Chromium '+process.versions.chrome+' verified navigation, review, pause, privacy and clearing.');
  win.close(); app.quit();
}).catch(err => { console.error(err); app.exit(1); });
