const {app, BrowserWindow, WebContentsView, ipcMain} = require('electron');
const {execFile, spawnSync} = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const {isIP} = require('node:net');
const {pathToFileURL} = require('node:url');

const workspace = path.resolve(process.argv[2]);
const helper = process.env.ROBOTUTOR_RESEARCH_HELPER;
const assets = path.resolve(__dirname, '../course-window/src/research');
const toolbarUrl = pathToFileURL(path.join(assets, 'toolbar.html')).href;
app.setPath('userData', path.join(workspace, 'chromium'));
app.enableSandbox();

function publicPage(raw) {
  try {
    const url = new URL(raw);
    return url.protocol === 'https:' && !url.username && !url.password && !url.port &&
      url.hostname.includes('.') && !isIP(url.hostname) &&
      !/[\[\]]|\.$|\.(localhost|local|lan|internal|test)$/.test(url.hostname);
  } catch { return false; }
}

function notebook(action, url = '', metadata = null) {
  const result = spawnSync(helper, ['--notebook', workspace], {
    input: JSON.stringify({action, url, metadata}), encoding: 'utf8', timeout: 5000
  });
  if (result.status !== 0) throw new Error('Notebook unavailable');
  return JSON.parse(result.stdout);
}

async function open() {
  const win = new BrowserWindow({width: 1040, height: 750, minWidth: 520, minHeight: 400,
    title: 'Robotutor · Research browser', autoHideMenuBar: true,
    webPreferences: {sandbox: true, contextIsolation: true, nodeIntegration: false,
      preload: path.join(__dirname, 'preload.cjs')}});
  const toolbar = win.webContents;
  toolbar.on('will-navigate', event => event.preventDefault());
  toolbar.setWindowOpenHandler(() => ({action: 'deny'}));
  const content = new WebContentsView({webPreferences: {partition: 'persist:research',
    sandbox: true, contextIsolation: true, nodeIntegration: false, webSecurity: true}});
  const web = content.webContents;
  const ses = web.session;
  ses.setPermissionRequestHandler((_contents, _permission, decision) => decision(false));
  ses.setPermissionCheckHandler(() => false);
  ses.on('will-download', event => event.preventDefault());
  web.on('will-attach-webview', event => event.preventDefault());
  for (const name of ['will-frame-navigate', 'will-redirect']) {
    web.on(name, event => {
      if (!publicPage(event.url)) event.preventDefault();
    });
  }
  web.setWindowOpenHandler(({url}) => {
    if (publicPage(url)) web.loadURL(url).catch(() => {});
    return {action: 'deny'};
  });
  let recording = false, expanded = false, waiting = false, account = '', message = '', epoch = 0, height = 100;
  const metadata = fs.readFileSync(path.join(assets, 'metadata.js'), 'utf8');
  win.contentView.addChildView(content);
  function layout() {
    const [width, h] = win.getContentSize();
    const top = Math.min(height, h - 1);
    content.setVisible(!expanded);
    content.setBounds({x: 0, y: top, width, height: h-top});
  }
  function revealPage() {
    expanded = false;
    layout();
  }
  async function render() {
    if (toolbar.isDestroyed()) return;
    layout();
    const url = web.getURL();
    const state = {recording, expanded, waiting, account, message, engine: "Chromium", runtime: process.versions.chrome,
      url: publicPage(url) ? url : '', rows: expanded ? notebook('list') : []};
    await toolbar.executeJavaScript('window.render?.(' + JSON.stringify(state) + ')');
  }
  async function observe() {
    const at = epoch, url = web.getURL();
    if (!recording || expanded || !publicPage(url)) return;
    const article = notebook('source', url);
    if (article === null) return;
    const data = article ? await web.executeJavaScript(metadata) : null;
    if (recording && !expanded && epoch === at && url === web.getURL() && (!article || data?.href === url))
      notebook('record', url, data);
  }
  function fail() {
    epoch++; recording = false; message = 'Research action failed. Recording paused.';
    if (!toolbar.isDestroyed()) toolbar.executeJavaScript('window.showError?.("Research action failed. Recording paused.")').catch(() => {});
  }
  web.on('did-start-navigation', () => {epoch++;});
  web.on('did-finish-load', () => {observe().then(render).catch(fail);});
  const control = async (event, request) => {
    if (event.sender !== toolbar || !event.senderFrame || event.senderFrame !== toolbar.mainFrame ||
        event.senderFrame.url !== toolbarUrl) return;
    switch (request.action) {
      case 'height': height = Math.max(70, Math.min(320, Number(request.value) || 90)); break;
      case 'go': {
        const target = notebook('destination', request.value);
        if (target && publicPage(target)) {revealPage(); message = ''; await web.loadURL(target);}
        else message = 'Enter a search or public HTTPS address.';
        break;
      }
      case 'toggle': epoch++; recording = !recording; if (recording) await observe(); break;
      case 'notebook': epoch++; expanded = !expanded; break;
      case 'clear': epoch++; recording = false; notebook('clear'); break;
      case 'back':
        if (web.navigationHistory.canGoBack()) {
          revealPage();
          web.navigationHistory.goBack();
        }
        break;
      case 'forward':
        if (web.navigationHistory.canGoForward()) {
          revealPage();
          web.navigationHistory.goForward();
        }
        break;
      case 'reload': revealPage(); web.reload(); break;
      case 'google':
        if (waiting) break;
        if (account) {account = ''; break;}
        waiting = true;
        execFile(helper, ['--notebook', workspace], {timeout: 180000}, (err, stdout) => {
          waiting = false;
          if (win.isDestroyed()) return;
          if (err) message = 'Google sign-in unavailable. Check the desktop client configuration.';
          else account = JSON.parse(stdout);
          render().catch(fail);
        }).stdin.end(JSON.stringify({action: 'google'}));
        break;
      default: return;
    }
    await render();
  };
  const listener = (event, request) => control(event, request).catch(fail);
  ipcMain.on('research-control', listener);
  win.on('resize', layout);
  win.on('closed', () => {ipcMain.removeListener('research-control', listener); web.close();});
  await win.loadURL(toolbarUrl);
  await web.loadURL('data:text/html;charset=utf-8,' + encodeURIComponent(fs.readFileSync(path.join(assets, 'welcome.html'), 'utf8')));
  await render();
  return {win, toolbar, web};
}

module.exports = {open, publicPage};
