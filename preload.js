const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('csvAPI', {
  openFile: () => ipcRenderer.invoke('open-file'),
  getRows: (start, count) => ipcRenderer.invoke('get-rows', start, count),
  getRowsByIndex: (indices) => ipcRenderer.invoke('get-rows-by-index', indices),
  getCellContent: (row, col) => ipcRenderer.invoke('get-cell-content', row, col),
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
});
