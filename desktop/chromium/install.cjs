// Keep Chromium sandboxed on Linux hosts that require a system-owned helper.
const fs = require('node:fs');
const path = require('node:path');
const electron = require('electron');
if (process.platform === 'linux') {
  const bundled = path.join(path.dirname(electron), 'chrome-sandbox');
  for (const candidate of ['/usr/local/libexec/robotutor/chrome-sandbox', '/opt/google/chrome/chrome-sandbox', '/opt/google/chrome/chrome_sandbox', '/usr/lib/chromium/chrome-sandbox']) {
    if (!fs.existsSync(candidate)) continue;
    const stat = fs.statSync(candidate);
    if (stat.uid !== 0 || (stat.mode & 0o4777) !== 0o4755) continue;
    if (fs.realpathSync(bundled) !== fs.realpathSync(candidate)) {
      fs.unlinkSync(bundled);
      fs.symlinkSync(candidate, bundled);
    }
    break;
  }
}
