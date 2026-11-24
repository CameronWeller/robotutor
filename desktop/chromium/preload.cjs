const {contextBridge, ipcRenderer} = require('electron');
contextBridge.exposeInMainWorld('ipc', {
  postMessage(input) { ipcRenderer.send('research-control', JSON.parse(input)); }
});
