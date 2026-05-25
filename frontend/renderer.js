// ─── Tauri API wrappers ─────────────────────────────────────────────────────

const tauri = window.__TAURI__ ?? window.__TAURI_INTERNALS__;
if (!tauri) {
  document.body.innerHTML = '<div style="padding:40px;color:red;font-family:sans-serif">Error: Tauri runtime not detected. Please run this app with <code>npm run dev</code> or <code>npm run build</code>.</div>';
  throw new Error('Tauri runtime not available');
}

const invoke = tauri.core?.invoke ?? tauri.invoke;
const _listen = tauri.event?.listen ?? tauri.listen;

async function listen(event, callback) {
  if (_listen) return _listen(event, callback);
  console.warn('Event listen not available, skipping: ' + event);
  return () => {};
}

const api = {
  openFile: () => invoke('open_file'),
  getRows: (start, count) => invoke('get_rows', { start, count }),
  getRowsByIndex: (indices) => invoke('get_rows_by_index', { indices }),
  getCellContent: (row, col) => invoke('get_cell_content', { row, col }),
  updateCell: (row, col, content) => invoke('update_cell', { row, col, content }),
  exportCSV: (colIndices, startRow, endRow) =>
    invoke('export_csv', { colIndices, startRow, endRow }),
  search: (query, colFilter, caseSensitive, maxResults) =>
    invoke('search_csv', { query, colFilter, caseSensitive, maxResults }),
  onProgress: (callback) => listen('index-progress', (e) => callback(e.payload ?? e)),
  onExportProgress: (callback) => listen('export-progress', (e) => callback(e.payload ?? e)),
  onSearchProgress: (callback) => listen('search-progress', (e) => callback(e.payload ?? e)),
  onMenuOpenFile: (callback) => listen('menu-open-file', callback),
};

// ─── DOM Elements ──────────────────────────────────────────────────────────

const welcome = document.getElementById('welcome');
const mainView = document.getElementById('main-view');
const tableHeaderWrap = document.getElementById('table-header-wrap');
const tableHeader = document.getElementById('table-header');
const scrollContainer = document.getElementById('scroll-container');
const scrollInner = document.getElementById('scroll-inner');
const rowsContainer = document.getElementById('rows-container');
const detailPanel = document.getElementById('detail-panel');
const detailContent = document.getElementById('detail-content');
const detailCol = document.getElementById('detail-col');
const detailRow = document.getElementById('detail-row');
const fileNameEl = document.getElementById('file-name');
const fileStatsEl = document.getElementById('file-stats');
const statusText = document.getElementById('status-text');
const rowHeightSelect = document.getElementById('row-height-select');
const exportModal = document.getElementById('export-modal');
const exportColumns = document.getElementById('export-columns');
const exportRowFrom = document.getElementById('export-row-from');
const exportRowTo = document.getElementById('export-row-to');
const exportRowTotal = document.getElementById('export-row-total');
const exportStatus = document.getElementById('export-status');
const searchInput = document.getElementById('search-input');
const searchColumn = document.getElementById('search-column');
const searchCaseSensitive = document.getElementById('search-case-sensitive');
const searchClearBtn = document.getElementById('btn-search-clear');
const searchResultsPanel = document.getElementById('search-results-panel');
const searchResultsStatus = document.getElementById('search-results-status');
const searchResultsList = document.getElementById('search-results-list');
const searchCloseBtn = document.getElementById('btn-search-close');

// ─── State ─────────────────────────────────────────────────────────────────

const MIN_COL_WIDTH = 120;

let fileInfo = null;
let rowHeight = 40;
let colWidth = MIN_COL_WIDTH;
let totalWidth = 0;
let rowElements = [];
let elementPool = [];
let visibleStart = 0;
let visibleEnd = 0;
let selectedCell = null;
let isEditing = false;
let editingCell = null;
let originalContent = '';
let scrollRAF = null;
let isScrolling = false;

let selectedRows = new Set();
let selectedCols = new Set();
let searchInProgress = false;
let lastClickedRow = -1;
let lastClickedCol = -1;

// ─── Event Listeners ───────────────────────────────────────────────────────

document.getElementById('btn-open-welcome').addEventListener('click', openFile);
document.getElementById('btn-open').addEventListener('click', openFile);
document.getElementById('btn-close-detail').addEventListener('click', closeDetail);
document.getElementById('btn-copy').addEventListener('click', copyCellContent);
document.getElementById('btn-edit').addEventListener('click', toggleEdit);
document.getElementById('btn-export').addEventListener('click', openExportDialog);
document.getElementById('btn-close-export').addEventListener('click', closeExportDialog);
document.getElementById('btn-cancel-export').addEventListener('click', closeExportDialog);
document.getElementById('btn-do-export').addEventListener('click', doExport);
exportModal.addEventListener('click', (e) => {
  if (e.target === exportModal) closeExportDialog();
});

rowHeightSelect.addEventListener('change', () => {
  rowHeight = parseInt(rowHeightSelect.value);
  if (fileInfo) {
    setupVirtualScroll();
    scheduleRender();
  }
});

scrollContainer.addEventListener('scroll', onScroll, { passive: true });
scrollContainer.addEventListener('scroll', syncHeaderScroll, { passive: true });

window.addEventListener('resize', () => {
  if (fileInfo) {
    calcColumnWidth();
    buildHeader();
    setupVirtualScroll();
    scheduleRender();
  }
});

api.onMenuOpenFile(() => {
  openFile();
});

api.onSearchProgress((payload) => {
  if (searchInProgress && payload && payload.total > 0) {
    const pct = Math.round((payload.done / payload.total) * 100);
    searchResultsStatus.textContent = 'Searching... ' + pct + '%';
  }
});

document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') {
    if (isEditing) {
      exitEditMode();
    } else {
      closeDetail();
      clearSelection();
    }
  }
  if ((e.ctrlKey || e.metaKey) && e.key === 'f') {
    e.preventDefault();
    searchInput.focus();
    searchInput.select();
  }
  if ((e.ctrlKey || e.metaKey) && e.key === 'c') {
    if (selectedCell && !detailPanel.classList.contains('hidden')) {
      copyCellContent();
    } else if (selectedCell) {
      copySelectedCell();
    }
  }
});

detailPanel.addEventListener('click', (e) => {
  if (e.target === detailPanel) closeDetail();
});

// ─── Search Events ────────────────────────────────────────────────────────

searchInput.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') {
    hideSearchResults();
    searchInput.blur();
  }
  if (e.key === 'Enter') {
    performSearch();
  }
});

searchClearBtn.addEventListener('click', () => {
  searchInput.value = '';
  hideSearchResults();
  searchInput.focus();
});

searchCloseBtn.addEventListener('click', hideSearchResults);

document.addEventListener('click', (e) => {
  if (!searchResultsPanel.classList.contains('hidden') &&
      !e.target.closest('.search-container') &&
      !e.target.closest('.search-results-panel')) {
    hideSearchResults();
  }
});

// ─── Row/Column Selection ──────────────────────────────────────────────────

rowsContainer.addEventListener('click', (e) => {
  const rowNum = e.target.closest('.row-num');
  if (!rowNum) return;
  const row = rowNum.closest('.table-row');
  if (!row) return;
  const rowIndex = parseInt(row.dataset.rowIndex);
  if (isNaN(rowIndex)) return;
  handleRowClick(rowIndex, e.ctrlKey || e.metaKey, e.shiftKey);
});

tableHeader.addEventListener('click', (e) => {
  const headerCell = e.target.closest('.header-cell:not(.row-num)');
  if (!headerCell) return;
  const colIndex = Array.from(tableHeader.children).indexOf(headerCell) - 1;
  if (colIndex < 0) return;
  handleColClick(colIndex, e.ctrlKey || e.metaKey, e.shiftKey);
});

function handleRowClick(rowIndex, ctrl, shift) {
  if (shift && lastClickedRow >= 0) {
    const from = Math.min(lastClickedRow, rowIndex);
    const to = Math.max(lastClickedRow, rowIndex);
    selectedRows.clear();
    for (let i = from; i <= to; i++) selectedRows.add(i);
  } else if (ctrl) {
    if (selectedRows.has(rowIndex)) selectedRows.delete(rowIndex);
    else selectedRows.add(rowIndex);
    lastClickedRow = rowIndex;
  } else {
    selectedRows.clear();
    selectedRows.add(rowIndex);
    lastClickedRow = rowIndex;
  }
  selectedCols.clear();
  lastClickedCol = -1;
  updateSelectionUI();
}

function handleColClick(colIndex, ctrl, shift) {
  if (shift && lastClickedCol >= 0) {
    const from = Math.min(lastClickedCol, colIndex);
    const to = Math.max(lastClickedCol, colIndex);
    selectedCols.clear();
    for (let i = from; i <= to; i++) selectedCols.add(i);
  } else if (ctrl) {
    if (selectedCols.has(colIndex)) selectedCols.delete(colIndex);
    else selectedCols.add(colIndex);
    lastClickedCol = colIndex;
  } else {
    selectedCols.clear();
    selectedCols.add(colIndex);
    lastClickedCol = colIndex;
  }
  selectedRows.clear();
  lastClickedRow = -1;
  updateSelectionUI();
}

function clearSelection() {
  selectedRows.clear();
  selectedCols.clear();
  lastClickedRow = -1;
  lastClickedCol = -1;
  updateSelectionUI();
}

function updateSelectionUI() {
  for (const rowEl of rowElements) {
    const ri = parseInt(rowEl.dataset.rowIndex);
    if (isNaN(ri)) continue;
    rowEl.classList.toggle('selected', selectedRows.has(ri));
    for (let j = 1; j < rowEl.children.length; j++) {
      rowEl.children[j].classList.toggle('col-selected', selectedCols.has(j - 1));
    }
  }
  for (let j = 1; j < tableHeader.children.length; j++) {
    tableHeader.children[j].classList.toggle('selected', selectedCols.has(j - 1));
  }
  if (selectedRows.size > 0 || selectedCols.size > 0) {
    const parts = [];
    if (selectedRows.size > 0) parts.push(selectedRows.size + ' row' + (selectedRows.size > 1 ? 's' : ''));
    if (selectedCols.size > 0) parts.push(selectedCols.size + ' col' + (selectedCols.size > 1 ? 's' : ''));
    statusText.textContent = parts.join(', ') + ' selected — use Export to save';
  }
}

scrollContainer.addEventListener('scroll', () => {
  tableHeader.scrollLeft = scrollContainer.scrollLeft;
}, { passive: true });

// ─── File Open ─────────────────────────────────────────────────────────────

async function openFile() {
  const btn = document.getElementById('btn-open-welcome');
  const origText = btn.textContent;
  btn.textContent = 'Indexing...';
  btn.disabled = true;

  const info = await api.openFile();

  btn.textContent = origText;
  btn.disabled = false;

  if (!info) return;

  fileInfo = info;
  elementPool = [];
  clearSelection();
  welcome.classList.add('hidden');
  mainView.classList.remove('hidden');
  closeDetail();

  fileNameEl.textContent = info.file_name;
  fileStatsEl.textContent = formatFileInfo(info);
  statusText.textContent = `Loaded ${info.row_count.toLocaleString()} rows, ${info.column_count} columns`;

  calcColumnWidth();
  buildHeader();
  populateSearchColumns();
  setupVirtualScroll();
  scrollContainer.scrollTop = 0;
  requestAnimationFrame(() => scheduleRender());
}

function formatFileInfo(info) {
  const parts = [];
  parts.push(formatBytes(info.file_size));
  parts.push(info.row_count.toLocaleString() + ' rows');
  parts.push(info.column_count + ' cols');
  return parts.join('  ·  ');
}

function formatBytes(bytes) {
  if (bytes < 1024) return bytes + ' B';
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
  if (bytes < 1024 * 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
  return (bytes / (1024 * 1024 * 1024)).toFixed(2) + ' GB';
}

// ─── Header ────────────────────────────────────────────────────────────────

function buildHeader() {
  tableHeader.innerHTML = '<div class="header-cell row-num">#</div>';
  for (const h of fileInfo.headers) {
    const cell = document.createElement('div');
    cell.className = 'header-cell';
    cell.style.width = colWidth + 'px';
    cell.style.minWidth = colWidth + 'px';
    cell.textContent = h || '(empty)';
    cell.title = h || '(empty)';
    tableHeader.appendChild(cell);
  }
}

// ─── Virtual Scrolling Setup ───────────────────────────────────────────────

function calcColumnWidth() {
  const containerWidth = scrollContainer.clientWidth || window.innerWidth - 32;
  const avail = containerWidth - 64;
  colWidth = Math.max(MIN_COL_WIDTH, Math.floor(avail / fileInfo.column_count));
  totalWidth = 64 + colWidth * fileInfo.column_count;
}

function setupVirtualScroll() {
  calcColumnWidth();

  const totalHeight = fileInfo.row_count * rowHeight;
  scrollInner.style.height = totalHeight + 'px';
  scrollInner.style.width = totalWidth + 'px';

  tableHeader.style.width = totalWidth + 'px';

  visibleStart = -1;
  visibleEnd = -1;

  const viewportHeight = scrollContainer.clientHeight;
  const visibleCount = Math.ceil(viewportHeight / rowHeight);
  const bufferedCount = visibleCount + 20;

  for (const el of rowElements) {
    el.remove();
  }
  rowElements = [];

  for (const row of elementPool) {
    row.style.width = totalWidth + 'px';
    for (let i = 1; i < row.children.length; i++) {
      row.children[i].style.width = colWidth + 'px';
      row.children[i].style.minWidth = colWidth + 'px';
    }
  }

  while (elementPool.length < bufferedCount) {
    const row = document.createElement('div');
    row.className = 'table-row';
    row.style.width = totalWidth + 'px';

    const numCell = document.createElement('div');
    numCell.className = 'row-num';
    row.appendChild(numCell);

    for (let i = 0; i < fileInfo.column_count; i++) {
      const cell = document.createElement('div');
      cell.className = 'table-cell';
      cell.style.width = colWidth + 'px';
      cell.style.minWidth = colWidth + 'px';
      cell.addEventListener('click', (e) => onCellClick(e, cell));
      cell.addEventListener('dblclick', (e) => onCellDoubleClick(e, cell));
      row.appendChild(cell);
    }

    elementPool.push(row);
  }

  for (let i = 0; i < bufferedCount; i++) {
    const row = elementPool[i];
    row.style.display = 'none';
    row.style.height = rowHeight + 'px';
    rowsContainer.appendChild(row);
    rowElements.push(row);
  }
}

function syncHeaderScroll() {
  tableHeaderWrap.scrollLeft = scrollContainer.scrollLeft;
}

// ─── Scroll Handler ────────────────────────────────────────────────────────

let renderSeq = 0;
let renderActive = false;
let needsRender = false;

function onScroll() {
  if (renderActive) {
    needsRender = true;
    return;
  }
  if (scrollRAF) return;
  scrollRAF = requestAnimationFrame(() => {
    scrollRAF = null;
    scheduleRender();
  });
}

function scheduleRender() {
  if (renderActive) {
    needsRender = true;
    return;
  }
  renderActive = true;
  needsRender = false;
  const seq = ++renderSeq;
  renderVisibleRows(seq).finally(() => {
    renderActive = false;
    if (needsRender) {
      needsRender = false;
      scheduleRender();
    }
  });
}

// ─── Row Rendering ─────────────────────────────────────────────────────────

async function renderVisibleRows(seq) {
  if (!fileInfo) return;

  const scrollTop = scrollContainer.scrollTop;
  const viewportHeight = scrollContainer.clientHeight;

  const start = Math.max(0, Math.floor(scrollTop / rowHeight) - 10);
  const end = Math.min(fileInfo.row_count, Math.ceil((scrollTop + viewportHeight) / rowHeight) + 10);
  const count = end - start;

  if (start === visibleStart && end === visibleEnd) return;

  while (rowElements.length < count) {
    const row = elementPool.length > rowElements.length
      ? elementPool[rowElements.length]
      : createRowElement();
    row.style.display = 'none';
    row.style.height = rowHeight + 'px';
    rowsContainer.appendChild(row);
    rowElements.push(row);
  }

  for (const row of rowElements) {
    row.style.display = 'none';
  }

  let rows;
  try {
    rows = await api.getRows(start, count);
  } catch (err) {
    console.error('Failed to get rows:', err);
    statusText.textContent = 'Error loading rows';
    return;
  }

  if (seq !== renderSeq) return;

  for (let i = 0; i < count; i++) {
    const rowEl = rowElements[i];
    const rowIndex = start + i;
    const rowData = rows[i] || { cells: [], lengths: [] };
    const cells = rowData.cells;
    const lengths = rowData.lengths;

    rowEl.style.display = 'flex';
    rowEl.style.top = (rowIndex * rowHeight) + 'px';
    rowEl.style.height = rowHeight + 'px';
    rowEl.style.width = totalWidth + 'px';
    rowEl.dataset.rowIndex = rowIndex;
    rowEl.classList.toggle('selected', selectedRows.has(rowIndex));

    const numCell = rowEl.children[0];
    numCell.textContent = (rowIndex + 1).toLocaleString();

    for (let j = 0; j < fileInfo.column_count; j++) {
      const cell = rowEl.children[j + 1];
      if (!cell) break;
      cell.style.width = colWidth + 'px';
      cell.style.minWidth = colWidth + 'px';
      const text = cells[j] || '';
      const origLen = lengths[j] || text.length;
      cell.textContent = text;
      cell.dataset.fullLength = origLen;
      cell.dataset.colIndex = j;
      cell.dataset.rowIndex = rowIndex;
      cell.classList.toggle('col-selected', selectedCols.has(j));
      cell.title = origLen > 500 ? 'Click to view full content (' + formatBytes(origLen) + ')' : '';

      if (origLen > 500) {
        cell.classList.add('has-more');
      } else {
        cell.classList.remove('has-more');
      }
    }

    for (let j = fileInfo.column_count + 1; j < rowEl.children.length; j++) {
      rowEl.children[j].textContent = '';
    }
  }

  visibleStart = start;
  visibleEnd = end;

  if (rows.length === 0 && count > 0) {
    statusText.textContent = 'No data returned for rows ' + (start + 1) + '-' + (end);
  }
}

function createRowElement() {
  const row = document.createElement('div');
  row.className = 'table-row';
  row.style.width = totalWidth + 'px';

  const numCell = document.createElement('div');
  numCell.className = 'row-num';
  row.appendChild(numCell);

  const colCount = fileInfo ? fileInfo.column_count : 1;
  for (let i = 0; i < colCount; i++) {
    const cell = document.createElement('div');
    cell.className = 'table-cell';
    cell.style.width = colWidth + 'px';
    cell.style.minWidth = colWidth + 'px';
    cell.addEventListener('click', (e) => onCellClick(e, cell));
    cell.addEventListener('dblclick', (e) => onCellDoubleClick(e, cell));
    row.appendChild(cell);
  }

  return row;
}

// ─── Cell Click / Detail Panel ─────────────────────────────────────────────

function onCellClick(e, cell) {
  const rowIndex = parseInt(cell.dataset.rowIndex);
  const colIndex = parseInt(cell.dataset.colIndex);
  if (isNaN(rowIndex) || isNaN(colIndex)) return;

  if (isEditing && editingCell && (editingCell.row !== rowIndex || editingCell.col !== colIndex)) {
    saveEdit();
  }

  clearSelection();

  selectedCell = { row: rowIndex, col: colIndex };
  openDetail(rowIndex, colIndex);
}

async function openDetail(row, col) {
  const colName = fileInfo.headers[col] || '(Column ' + (col + 1) + ')';
  detailCol.textContent = 'Column: ' + colName + ' (#' + (col + 1) + ')';
  detailRow.textContent = 'Row: ' + (row + 1).toLocaleString();
  detailContent.value = 'Loading...';
  detailPanel.classList.remove('hidden');

  try {
    const content = await api.getCellContent(row, col);
    detailContent.value = content;
    statusText.textContent = 'Cell [' + (row + 1) + ', ' + (col + 1) + '] — ' + formatBytes(content.length);
  } catch (err) {
    detailContent.value = 'Error loading content: ' + err;
  }
}

function closeDetail() {
  detailPanel.classList.add('hidden');
  selectedCell = null;
  exitEditMode();
}

// ─── Cell Editing ───────────────────────────────────────────────────────────

function toggleEdit() {
  if (!selectedCell) return;
  if (isEditing) {
    saveEdit();
  } else {
    enterEditMode(selectedCell.row, selectedCell.col);
  }
}

function enterEditMode(row, col) {
  if (isEditing) saveEdit();
  isEditing = true;
  editingCell = { row, col };
  originalContent = detailContent.value;
  detailContent.readOnly = false;
  detailContent.focus();
  updateEditButton();
}

function exitEditMode() {
  isEditing = false;
  editingCell = null;
  originalContent = '';
  detailContent.readOnly = true;
  updateEditButton();
}

function updateEditButton() {
  const btn = document.getElementById('btn-edit');
  if (isEditing) {
    btn.innerHTML = `
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="14" height="14">
        <polyline points="20 6 9 17 4 12"/>
      </svg>
      Save`;
    btn.classList.add('btn-save');
  } else {
    btn.innerHTML = `
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="14" height="14">
        <path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/>
        <path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/>
      </svg>
      Edit`;
    btn.classList.remove('btn-save');
  }
}

async function saveEdit() {
  if (!editingCell || !isEditing) return;
  const newContent = detailContent.value;
  if (newContent === originalContent) {
    exitEditMode();
    return;
  }

  const { row, col } = editingCell;
  try {
    await api.updateCell(row, col, newContent);
    showToast('Saved ' + formatBytes(newContent.length));
    statusText.textContent = 'Cell [' + (row + 1) + ', ' + (col + 1) + '] — ' + formatBytes(newContent.length);
    originalContent = newContent;
    visibleStart = -1;
    visibleEnd = -1;
    scheduleRender();
  } catch (err) {
    showToast('Save failed: ' + err);
  }
  exitEditMode();
}

async function onCellDoubleClick(e, cell) {
  e.stopPropagation();
  const rowIndex = parseInt(cell.dataset.rowIndex);
  const colIndex = parseInt(cell.dataset.colIndex);
  if (isNaN(rowIndex) || isNaN(colIndex)) return;

  if (isEditing) await saveEdit();
  openDetail(rowIndex, colIndex);
  setTimeout(() => enterEditMode(rowIndex, colIndex), 100);
}

// ─── Export Dialog ──────────────────────────────────────────────────────────

function openExportDialog() {
  if (!fileInfo) return;

  exportColumns.innerHTML = '';
  const selCols = selectedCols.size > 0 ? selectedCols : new Set([...Array(fileInfo.column_count).keys()]);
  for (let i = 0; i < fileInfo.column_count; i++) {
    const label = document.createElement('label');
    label.className = 'export-col-checkbox' + (selCols.has(i) ? ' checked' : '');
    const cb = document.createElement('input');
    cb.type = 'checkbox';
    cb.checked = selCols.has(i);
    cb.addEventListener('change', () => label.classList.toggle('checked', cb.checked));
    label.appendChild(cb);
    label.appendChild(document.createTextNode(fileInfo.headers[i] || '(col ' + (i + 1) + ')'));
    exportColumns.appendChild(label);
  }

  exportRowFrom.max = fileInfo.row_count;
  exportRowTo.max = fileInfo.row_count;
  if (selectedRows.size > 0) {
    const sorted = [...selectedRows].sort((a, b) => a - b);
    exportRowFrom.value = sorted[0] + 1;
    exportRowTo.value = sorted[sorted.length - 1] + 1;
  } else {
    exportRowFrom.value = 1;
    exportRowTo.value = fileInfo.row_count;
  }
  exportRowTotal.textContent = '(of ' + fileInfo.row_count.toLocaleString() + ' rows)';
  exportStatus.textContent = '';

  document.querySelectorAll('.btn-preset').forEach(btn => {
    btn.onclick = () => {
      if (btn.dataset.preset === 'all') {
        exportRowFrom.value = 1;
        exportRowTo.value = fileInfo.row_count;
      } else if (btn.dataset.preset === 'selected' && selectedRows.size > 0) {
        const sorted = [...selectedRows].sort((a, b) => a - b);
        exportRowFrom.value = sorted[0] + 1;
        exportRowTo.value = sorted[sorted.length - 1] + 1;
      }
    };
  });

  exportModal.classList.remove('hidden');
}

function closeExportDialog() {
  exportModal.classList.add('hidden');
}

async function doExport() {
  const colIndices = [];
  exportColumns.querySelectorAll('input[type="checkbox"]').forEach((cb, i) => {
    if (cb.checked) colIndices.push(i);
  });
  if (colIndices.length === 0) { exportStatus.textContent = 'Select at least one column.'; return; }

  const fromRow = parseInt(exportRowFrom.value) - 1;
  const toRow = parseInt(exportRowTo.value) - 1;
  if (isNaN(fromRow) || isNaN(toRow) || fromRow < 0 || toRow >= fileInfo.row_count || fromRow > toRow) {
    exportStatus.textContent = 'Invalid row range.'; return;
  }

  const totalRows = toRow - fromRow + 1;
  exportStatus.textContent = 'Exporting ' + totalRows.toLocaleString() + ' rows...';
  document.getElementById('btn-do-export').disabled = true;

  try {
    const result = await api.exportCSV(colIndices, fromRow, toRow);
    if (result.canceled) {
      exportStatus.textContent = '';
    } else if (result.error) {
      exportStatus.textContent = 'Error: ' + result.error;
    } else if (result.ok) {
      exportStatus.textContent = 'Saved to ' + result.path;
      setTimeout(closeExportDialog, 1200);
    }
  } catch (err) {
    exportStatus.textContent = 'Export failed';
  }

  document.getElementById('btn-do-export').disabled = false;
  clearSelection();
}


async function copyCellContent() {
  const text = detailContent.value;
  if (!text) return;

  try {
    await navigator.clipboard.writeText(text);
    showToast('Copied ' + formatBytes(text.length));
  } catch {
    detailContent.select();
    document.execCommand('copy');
    showToast('Copied');
  }
}

async function copySelectedCell() {
  if (!selectedCell) return;
  try {
    const content = await api.getCellContent(selectedCell.row, selectedCell.col);
    await navigator.clipboard.writeText(content);
    showToast('Copied ' + formatBytes(content.length));
  } catch {
    showToast('Copy failed');
  }
}

// ─── Toast ─────────────────────────────────────────────────────────────────

let toastTimer = null;

function showToast(message) {
  let toast = document.querySelector('.toast');
  if (!toast) {
    toast = document.createElement('div');
    toast.className = 'toast';
    document.body.appendChild(toast);
  }
  toast.textContent = message;
  toast.classList.add('show');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    toast.classList.remove('show');
  }, 1800);
}

// ─── Search ─────────────────────────────────────────────────────────────────

function populateSearchColumns() {
  searchColumn.innerHTML = '<option value="">All Columns</option>';
  for (let i = 0; i < fileInfo.headers.length; i++) {
    const opt = document.createElement('option');
    opt.value = i;
    opt.textContent = fileInfo.headers[i] || '(col ' + (i + 1) + ')';
    searchColumn.appendChild(opt);
  }
}

async function performSearch() {
  const query = searchInput.value.trim();
  if (query.length < 2) return;
  if (!fileInfo) return;

  searchInProgress = true;
  searchResultsStatus.textContent = 'Searching...';

  const colFilter = searchColumn.value !== '' ? parseInt(searchColumn.value) : null;
  const caseSensitive = searchCaseSensitive.checked;
  const maxResults = 500;

  try {
    const results = await api.search(query, colFilter, caseSensitive, maxResults);
    renderSearchResults(results, query, maxResults);
  } catch (err) {
    console.error('Search failed:', err);
    searchResultsStatus.textContent = 'Search error: ' + err;
  } finally {
    searchInProgress = false;
  }
}

function renderSearchResults(results, query, maxResults) {
  if (results.length === 0) {
    searchResultsList.innerHTML = '<div class="search-no-results">No results for "' + escapeHtml(query) + '"</div>';
    searchResultsStatus.textContent = '0 results';
  } else {
    let html = '';
    for (const result of results) {
      html += '<div class="search-result-item" data-row="' + result.row_index + '">';
      html += '<div class="search-result-row">Row ' + (result.row_index + 1).toLocaleString() + '</div>';
      html += '<div class="search-result-matches">';
      for (const m of result.matches) {
        html += '<div class="search-result-match">';
        html += '<span class="col-name">' + escapeHtml(m.col_name) + ':</span> ';
        html += '<span>' + escapeHtml(m.cell_text) + '</span>';
        html += '</div>';
      }
      html += '</div></div>';
    }
    searchResultsList.innerHTML = html;

    if (results.length >= maxResults) {
      searchResultsStatus.textContent = maxResults + '+ results (capped, refine search for more)';
    } else {
      searchResultsStatus.textContent = results.length.toLocaleString() + ' result' +
        (results.length !== 1 ? 's' : '');
    }
  }

  searchResultsList.querySelectorAll('.search-result-item').forEach(item => {
    item.addEventListener('click', () => {
      const rowIndex = parseInt(item.dataset.row);
      if (!isNaN(rowIndex)) {
        navigateToRow(rowIndex);
      }
    });
  });

  searchResultsPanel.classList.remove('hidden');
}

function hideSearchResults() {
  searchResultsPanel.classList.add('hidden');
}

function navigateToRow(rowIndex) {
  if (!fileInfo) return;
  const targetScrollTop = rowIndex * rowHeight;
  scrollContainer.scrollTop = targetScrollTop;
  hideSearchResults();
}

function escapeHtml(str) {
  const div = document.createElement('div');
  div.textContent = str;
  return div.innerHTML;
}
