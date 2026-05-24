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
let scrollRAF = null;
let isScrolling = false;

// Selection state
let selectedRows = new Set();
let selectedCols = new Set();
let lastClickedRow = -1;
let lastClickedCol = -1;

// ─── Event Listeners ───────────────────────────────────────────────────────

document.getElementById('btn-open-welcome').addEventListener('click', openFile);
document.getElementById('btn-open').addEventListener('click', openFile);
document.getElementById('btn-close-detail').addEventListener('click', closeDetail);
document.getElementById('btn-copy').addEventListener('click', copyCellContent);
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

// Menu bar "Open File" event
window.csvAPI.onMenuOpenFile(() => {
  openFile();
});

document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') {
    closeDetail();
    clearSelection();
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
  // Update status text
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

  const unsub = window.csvAPI.onProgress((progress) => {
    btn.textContent = 'Indexing... ' + progress.percent + '%';
  });

  const info = await window.csvAPI.openFile();
  unsub();

  btn.textContent = origText;
  btn.disabled = false;

  if (!info) return;

  fileInfo = info;
  elementPool = [];
  clearSelection();
  welcome.classList.add('hidden');
  mainView.classList.remove('hidden');
  closeDetail();

  fileNameEl.textContent = info.fileName;
  fileStatsEl.textContent = formatFileInfo(info);
  statusText.textContent = `Loaded ${info.rowCount.toLocaleString()} rows, ${info.columnCount} columns`;

  calcColumnWidth();
  buildHeader();
  setupVirtualScroll();
  scrollContainer.scrollTop = 0;
  requestAnimationFrame(() => scheduleRender());
}

function formatFileInfo(info) {
  const parts = [];
  parts.push(formatBytes(info.fileSize));
  parts.push(info.rowCount.toLocaleString() + ' rows');
  parts.push(info.columnCount + ' cols');
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
  const avail = containerWidth - 64; // minus row number column
  colWidth = Math.max(MIN_COL_WIDTH, Math.floor(avail / fileInfo.columnCount));
  totalWidth = 64 + colWidth * fileInfo.columnCount;
}

function setupVirtualScroll() {
  calcColumnWidth();

  const totalHeight = fileInfo.rowCount * rowHeight;
  scrollInner.style.height = totalHeight + 'px';
  scrollInner.style.width = totalWidth + 'px';

  tableHeader.style.width = totalWidth + 'px';

  // Reset visible range so next render always proceeds
  visibleStart = -1;
  visibleEnd = -1;

  // Calculate how many rows fit in viewport
  const viewportHeight = scrollContainer.clientHeight;
  const visibleCount = Math.ceil(viewportHeight / rowHeight);
  const bufferedCount = visibleCount + 20;

  // Recycle old elements
  for (const el of rowElements) {
    el.remove();
  }
  rowElements = [];

  // Update existing pool elements' widths
  for (const row of elementPool) {
    row.style.width = totalWidth + 'px';
    for (let i = 1; i < row.children.length; i++) {
      row.children[i].style.width = colWidth + 'px';
      row.children[i].style.minWidth = colWidth + 'px';
    }
  }

  // Create or reuse row elements
  while (elementPool.length < bufferedCount) {
    const row = document.createElement('div');
    row.className = 'table-row';
    row.style.width = totalWidth + 'px';

    const numCell = document.createElement('div');
    numCell.className = 'row-num';
    row.appendChild(numCell);

    for (let i = 0; i < fileInfo.columnCount; i++) {
      const cell = document.createElement('div');
      cell.className = 'table-cell';
      cell.style.width = colWidth + 'px';
      cell.style.minWidth = colWidth + 'px';
      cell.addEventListener('click', (e) => onCellClick(e, cell));
      row.appendChild(cell);
    }

    elementPool.push(row);
  }

  // Place elements in DOM
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
  const end = Math.min(fileInfo.rowCount, Math.ceil((scrollTop + viewportHeight) / rowHeight) + 10);
  const count = end - start;

  if (start === visibleStart && end === visibleEnd) return;

  // Ensure we have enough elements
  while (rowElements.length < count) {
    const row = elementPool.length > rowElements.length
      ? elementPool[rowElements.length]
      : createRowElement();
    row.style.display = 'none';
    row.style.height = rowHeight + 'px';
    rowsContainer.appendChild(row);
    rowElements.push(row);
  }

  // Hide all rows first
  for (const row of rowElements) {
    row.style.display = 'none';
  }

  // Request data
  let rows;
  try {
    rows = await window.csvAPI.getRows(start, count);
  } catch (err) {
    console.error('Failed to get rows:', err);
    statusText.textContent = 'Error loading rows';
    return;
  }

  // Abort if a newer render has started
  if (seq !== renderSeq) return;

  // Render visible rows
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

    // Row number
    const numCell = rowEl.children[0];
    numCell.textContent = (rowIndex + 1).toLocaleString();

    // Data cells
    for (let j = 0; j < fileInfo.columnCount; j++) {
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

    // Clear unused cells
    for (let j = fileInfo.columnCount + 1; j < rowEl.children.length; j++) {
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

  const colCount = fileInfo ? fileInfo.columnCount : 1;
  for (let i = 0; i < colCount; i++) {
    const cell = document.createElement('div');
    cell.className = 'table-cell';
    cell.style.width = colWidth + 'px';
    cell.style.minWidth = colWidth + 'px';
    cell.addEventListener('click', (e) => onCellClick(e, cell));
    row.appendChild(cell);
  }

  return row;
}

// ─── Cell Click / Detail Panel ─────────────────────────────────────────────

function onCellClick(e, cell) {
  const rowIndex = parseInt(cell.dataset.rowIndex);
  const colIndex = parseInt(cell.dataset.colIndex);
  if (isNaN(rowIndex) || isNaN(colIndex)) return;

  // Clear multi-selection when clicking a single cell
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
    const content = await window.csvAPI.getCellContent(row, col);
    detailContent.value = content;
    statusText.textContent = 'Cell [' + (row + 1) + ', ' + (col + 1) + '] — ' + formatBytes(content.length);
  } catch (err) {
    detailContent.value = 'Error loading content: ' + err.message;
  }
}

function closeDetail() {
  detailPanel.classList.add('hidden');
  selectedCell = null;
}

// ─── Export Dialog ──────────────────────────────────────────────────────────

function openExportDialog() {
  if (!fileInfo) return;

  exportColumns.innerHTML = '';
  const selCols = selectedCols.size > 0 ? selectedCols : new Set([...Array(fileInfo.columnCount).keys()]);
  for (let i = 0; i < fileInfo.columnCount; i++) {
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

  exportRowFrom.max = fileInfo.rowCount;
  exportRowTo.max = fileInfo.rowCount;
  if (selectedRows.size > 0) {
    const sorted = [...selectedRows].sort((a, b) => a - b);
    exportRowFrom.value = sorted[0] + 1;
    exportRowTo.value = sorted[sorted.length - 1] + 1;
  } else {
    exportRowFrom.value = 1;
    exportRowTo.value = fileInfo.rowCount;
  }
  exportRowTotal.textContent = '(of ' + fileInfo.rowCount.toLocaleString() + ' rows)';
  exportStatus.textContent = '';

  document.querySelectorAll('.btn-preset').forEach(btn => {
    btn.onclick = () => {
      if (btn.dataset.preset === 'all') {
        exportRowFrom.value = 1;
        exportRowTo.value = fileInfo.rowCount;
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
  if (isNaN(fromRow) || isNaN(toRow) || fromRow < 0 || toRow >= fileInfo.rowCount || fromRow > toRow) {
    exportStatus.textContent = 'Invalid row range.'; return;
  }

  const totalRows = toRow - fromRow + 1;
  exportStatus.textContent = 'Exporting ' + totalRows.toLocaleString() + ' rows...';
  document.getElementById('btn-do-export').disabled = true;

  const unsub = window.csvAPI.onExportProgress((p) => {
    if (p.done) {
      exportStatus.textContent = 'Done.';
    } else {
      const pct = Math.round((p.current / p.total) * 100);
      exportStatus.textContent = 'Exporting... ' + pct + '%';
    }
  });

  try {
    const result = await window.csvAPI.exportCSV(colIndices, fromRow, toRow);
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

  unsub();
  document.getElementById('btn-do-export').disabled = false;

  // Clear selection after export
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
    const content = await window.csvAPI.getCellContent(selectedCell.row, selectedCell.col);
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
