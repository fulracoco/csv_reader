const { app, BrowserWindow, ipcMain, dialog, Menu, shell } = require('electron');
const path = require('path');
const { Worker } = require('worker_threads');

let mainWindow;
let csvWorker = null;
let requestId = 0;
let currentFilePath = null;
const pendingRequests = new Map();

// ─── Worker Communication ───────────────────────────────────────────────────

function getWorker() {
  if (!csvWorker) {
    csvWorker = new Worker(path.join(__dirname, 'csv-worker.js'));
    csvWorker.on('message', (msg) => {
      if (msg.type === 'progress') {
        // Forward progress to renderer
        if (mainWindow && !mainWindow.isDestroyed()) {
          if (msg.data.export) {
            mainWindow.webContents.send('export-progress', msg.data);
          } else {
            mainWindow.webContents.send('index-progress', msg.data);
          }
        }
        return;
      }

      const { id, result, error } = msg;
      const handler = pendingRequests.get(id);
      if (handler) {
        pendingRequests.delete(id);
        if (error) {
          handler.reject(new Error(error));
        } else {
          handler.resolve(result);
        }
      }
    });

    csvWorker.on('error', (err) => {
      console.error('Worker error:', err);
    });
  }
  return csvWorker;
}

function sendToWorker(action, ...args) {
  return new Promise((resolve, reject) => {
    const id = ++requestId;
    pendingRequests.set(id, { resolve, reject });
    getWorker().postMessage({ id, action, args });
  });
}

function terminateWorker() {
  if (csvWorker) {
    const w = csvWorker;
    csvWorker = null;
    // Reject all pending requests
    for (const [id, handler] of pendingRequests) {
      handler.reject(new Error('Worker terminated'));
      pendingRequests.delete(id);
    }
    w.terminate();
  }
}

// ─── i18n ───────────────────────────────────────────────────────────────────

let locale = (app.getLocale() || 'en').startsWith('zh') ? 'zh' : 'en';

const messages = {
  en: {
    file: 'File',
    openFile: 'Open File...',
    edit: 'Edit',
    view: 'View',
    help: 'Help',
    issues: 'Issues',
    language: 'Language',
    english: 'English',
    chinese: '中文',
  },
  zh: {
    file: '文件',
    openFile: '打开文件...',
    edit: '编辑',
    view: '视图',
    help: '帮助',
    issues: '问题反馈',
    language: '语言',
    english: 'English',
    chinese: '中文',
  },
};

function t(key) {
  return messages[locale][key] || key;
}

function buildAppMenu() {
  const template = [
    {
      label: t('file'),
      submenu: [
        {
          label: t('openFile'),
          accelerator: 'CmdOrCtrl+O',
          click: () => {
            if (mainWindow && !mainWindow.isDestroyed()) {
              mainWindow.webContents.send('menu-open-file');
            }
          },
        },
        { type: 'separator' },
        { role: 'quit' },
      ],
    },
    {
      label: t('edit'),
      submenu: [
        { role: 'copy' },
        { role: 'selectAll' },
      ],
    },
    {
      label: t('view'),
      submenu: [
        { role: 'reload' },
        { role: 'forceReload' },
        { role: 'toggleDevTools' },
        { type: 'separator' },
        { role: 'zoomIn' },
        { role: 'zoomOut' },
        { role: 'resetZoom' },
      ],
    },
    {
      label: t('help'),
      submenu: [
        {
          label: t('issues'),
          click: () => {
            shell.openExternal('https://github.com/fulracoco/csv_reader/issues');
          },
        },
        { type: 'separator' },
        {
          label: t('language'),
          submenu: [
            {
              label: t('english'),
              type: 'radio',
              checked: locale === 'en',
              click: () => { locale = 'en'; buildAppMenu(); },
            },
            {
              label: t('chinese'),
              type: 'radio',
              checked: locale === 'zh',
              click: () => { locale = 'zh'; buildAppMenu(); },
            },
          ],
        },
      ],
    },
  ];

  if (process.platform === 'darwin') {
    template.unshift({
      label: app.name,
      submenu: [
        { role: 'about' },
        { type: 'separator' },
        { role: 'services' },
        { type: 'separator' },
        { role: 'hide' },
        { role: 'hideOthers' },
        { role: 'unhide' },
        { type: 'separator' },
        { role: 'quit' },
      ],
    });
  }

  const menu = Menu.buildFromTemplate(template);
  Menu.setApplicationMenu(menu);
}

// ─── Electron Window ───────────────────────────────────────────────────────

function createWindow() {
  mainWindow = new BrowserWindow({
    width: 1400,
    height: 900,
    minWidth: 900,
    minHeight: 500,
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      nodeIntegration: false,
      contextIsolation: true,
    },
    title: 'CSV Reader',
    backgroundColor: '#1a1a2e',
  });

  mainWindow.loadFile('index.html');
}

app.whenReady().then(() => {
  createWindow();
  buildAppMenu();
});

app.on('window-all-closed', async () => {
  terminateWorker();
  if (process.platform !== 'darwin') app.quit();
});

app.on('will-quit', () => {
  terminateWorker();
});

app.on('activate', () => {
  if (BrowserWindow.getAllWindows().length === 0) createWindow();
});

// ─── IPC Handlers ──────────────────────────────────────────────────────────

ipcMain.handle('open-file', async () => {
  const result = await dialog.showOpenDialog(mainWindow, {
    title: 'Open CSV File',
    filters: [
      { name: 'CSV & TSV Files', extensions: ['csv', 'tsv', 'txt'] },
      { name: 'All Files', extensions: ['*'] },
    ],
    properties: ['openFile'],
  });

  if (result.canceled || result.filePaths.length === 0) return null;

  try {
    currentFilePath = result.filePaths[0];
    const info = await sendToWorker('open', currentFilePath);
    return info;
  } catch (err) {
    console.error('Error opening file:', err);
    return null;
  }
});

ipcMain.handle('get-rows', async (_, start, count) => {
  try {
    return await sendToWorker('get-rows', start, count);
  } catch (err) {
    console.error('Error reading rows:', err);
    return [];
  }
});

ipcMain.handle('get-rows-by-index', async (_, indices) => {
  try {
    return await sendToWorker('get-rows-by-index', indices);
  } catch (err) {
    console.error('Error reading rows by index:', err);
    return [];
  }
});

ipcMain.handle('export-csv', async (_, colIndices, startRow, endRow) => {
  const defaultPath = currentFilePath
    ? currentFilePath.replace(/\.\w+$/, '_export.csv')
    : 'export.csv';

  const result = await dialog.showSaveDialog(mainWindow, {
    title: 'Export CSV',
    defaultPath,
    filters: [{ name: 'CSV Files', extensions: ['csv'] }],
  });

  if (result.canceled) return { canceled: true };

  try {
    await sendToWorker('export-csv', result.filePath, colIndices, startRow, endRow);
    return { ok: true, path: result.filePath };
  } catch (err) {
    console.error('Export error:', err);
    return { error: err.message };
  }
});

ipcMain.handle('get-cell-content', async (_, row, col) => {
  try {
    return await sendToWorker('get-cell-content', row, col);
  } catch (err) {
    console.error('Error reading cell:', err);
    return '';
  }
});

ipcMain.handle('update-cell', async (_, row, col, content) => {
  try {
    await sendToWorker('update-cell', row, col, content);
    return { ok: true };
  } catch (err) {
    console.error('Error updating cell:', err);
    return { error: err.message };
  }
});
