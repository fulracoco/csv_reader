const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('csvAPI', {
  openFile: () => ipcRenderer.invoke('open-file'),
  getRows: (start, count) => ipcRenderer.invoke('get-rows', start, count),
  getRowsByIndex: (indices) => ipcRenderer.invoke('get-rows-by-index', indices),
  getCellContent: (row, col) => ipcRenderer.invoke('get-cell-content', row, col),
  updateCell: (row, col, content) => ipcRenderer.invoke('update-cell', row, col, content),
  exportCSV: (colIndices, startRow, endRow) =>
    ipcRenderer.invoke('export-csv', colIndices, startRow, endRow),
  onProgress: (callback) => {
    const handler = (_, progress) => callback(progress);
    ipcRenderer.on('index-progress', handler);
    return () => ipcRenderer.removeListener('index-progress', handler);
  },
  onExportProgress: (callback) => {
    const handler = (_, progress) => callback(progress);
    ipcRenderer.on('export-progress', handler);
    return () => ipcRenderer.removeListener('export-progress', handler);
  },
  onMenuOpenFile: (callback) => {
    const handler = () => callback();
    ipcRenderer.on('menu-open-file', handler);
    return () => ipcRenderer.removeListener('menu-open-file', handler);
  },
});
