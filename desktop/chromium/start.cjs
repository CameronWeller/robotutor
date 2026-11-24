const {app} = require('electron');
const {open} = require('./main.cjs');
app.whenReady().then(open).catch(err => {console.error(err.message); app.exit(1);});
app.on('window-all-closed', () => app.quit());
if (process.argv.includes('--parent-pipe')) {
  process.stdin.on('end', () => app.quit()); process.stdin.resume();
}
