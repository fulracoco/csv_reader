// csv-worker.js — Worker thread for CSV file operations
// Keeps CPU-intensive parsing off the main thread.

const { parentPort } = require('worker_threads');
const fs = require('fs');
const path = require('path');

// ─── Utilities ──────────────────────────────────────────────────────────────

const MAX_CELL_PREVIEW = 500;

function truncateCell(text) {
  if (text && text.length > MAX_CELL_PREVIEW) {
    return text.substring(0, MAX_CELL_PREVIEW);
  }
  return text;
}

function csvQuote(val) {
  if (val.includes(',') || val.includes('"') || val.includes('\n') || val.includes('\r')) {
    return '"' + val.replace(/"/g, '""') + '"';
  }
  return val;
}

function parseCSVLine(text, delimiter) {
  const result = [];
  let current = '';
  let inQuotes = false;

  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (inQuotes) {
      if (ch === '"') {
        if (i + 1 < text.length && text[i + 1] === '"') {
          current += '"';
          i++;
        } else {
          inQuotes = false;
        }
      } else {
        current += ch;
      }
    } else {
      if (ch === '"') {
        inQuotes = true;
      } else if (ch === delimiter) {
        result.push(current);
        current = '';
      } else if (ch !== '\r' && ch !== '\n') {
        current += ch;
      }
    }
  }
  result.push(current);
  return result;
}

function splitCSVRows(text) {
  const rows = [];
  let current = '';
  let inQuotes = false;

  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (inQuotes) {
      if (ch === '"') {
        if (i + 1 < text.length && text[i + 1] === '"') {
          current += '""';
          i++;
        } else {
          inQuotes = false;
        }
      }
      current += ch;
    } else {
      if (ch === '"') {
        inQuotes = true;
        current += ch;
      } else if (ch === '\n') {
        rows.push(current);
        current = '';
      } else if (ch === '\r') {
        if (i + 1 < text.length && text[i + 1] === '\n') {
          i++;
        }
        rows.push(current);
        current = '';
      } else {
        current += ch;
      }
    }
  }
  if (current.length > 0) {
    rows.push(current);
  }
  return rows;
}

function detectDelimiter(text) {
  const candidates = [',', '\t', ';', '|'];
  let best = ',';
  let bestCount = 0;

  for (const delim of candidates) {
    let count = 0;
    let inQuotes = false;
    for (let i = 0; i < text.length; i++) {
      const ch = text[i];
      if (ch === '"') {
        if (inQuotes && i + 1 < text.length && text[i + 1] === '"') {
          i++;
        } else {
          inQuotes = !inQuotes;
        }
      } else if (ch === delim && !inQuotes) {
        count++;
      }
    }
    if (count > bestCount) {
      bestCount = count;
      best = delim;
    }
  }

  return best;
}

// ─── CSV Engine ──────────────────────────────────────────────────────────────

class CsvEngine {
  constructor() {
    this.fd = null;
    this.filePath = null;
    this.fileSize = 0;
    this.offsets = [];
    this.headers = [];
    this.delimiter = ',';
    this.encoding = 'utf8';
    this.bomOffset = 0;
    this.cache = new Map();
    this.cacheKeys = [];
    this.cacheMax = 500;
  }

  sendProgress(data) {
    parentPort.postMessage({ type: 'progress', data });
  }

  async open(filePath) {
    this.filePath = filePath;
    this.fd = await fs.promises.open(filePath, 'r');
    const stat = await this.fd.stat();
    this.fileSize = stat.size;

    const bom = Buffer.allocUnsafe(4);
    await this.fd.read(bom, 0, 4, 0);
    if (bom[0] === 0xEF && bom[1] === 0xBB && bom[2] === 0xBF) {
      this.encoding = 'utf8';
      this.bomOffset = 3;
    } else if (bom[0] === 0xFF && bom[1] === 0xFE) {
      this.encoding = 'utf16le';
      this.bomOffset = 2;
    } else if (bom[0] === 0xFE && bom[1] === 0xFF) {
      this.encoding = 'utf16be';
      this.bomOffset = 2;
    } else {
      this.encoding = 'utf8';
      this.bomOffset = 0;
    }

    await this.buildIndex();

    if (this.offsets.length > 0) {
      const headerText = await this.readRowBytes(0);
      this.delimiter = detectDelimiter(headerText);
      this.headers = parseCSVLine(headerText, this.delimiter);
    }

    const rowCount = Math.max(0, this.offsets.length - 1);
    while (rowCount > 0 && this.offsets[rowCount] >= this.fileSize) {
      this.offsets.pop();
    }
    const actualRowCount = Math.max(0, this.offsets.length - 1);

    return {
      filePath: this.filePath,
      fileName: path.basename(this.filePath),
      fileSize: this.fileSize,
      rowCount: actualRowCount,
      columnCount: this.headers.length,
      headers: this.headers,
    };
  }

  async buildIndex() {
    const CHUNK_SIZE = 256 * 1024;
    this.offsets = [this.bomOffset];
    let bytePos = this.bomOffset;
    let inQuotes = false;
    let prevByteWasCR = false;
    let lastProgress = 0;

    while (bytePos < this.fileSize) {
      const toRead = Math.min(CHUNK_SIZE, this.fileSize - bytePos);
      const buffer = Buffer.allocUnsafe(toRead);
      await this.fd.read(buffer, 0, toRead, bytePos);

      for (let i = 0; i < buffer.length; i++) {
        const ch = buffer[i];

        if (i === 0 && prevByteWasCR && ch === 0x0A && !inQuotes) {
          prevByteWasCR = false;
          const offset = bytePos + 1;
          if (offset < this.fileSize) this.offsets.push(offset);
          continue;
        }

        if (ch === 0x22 && this.encoding !== 'utf16le' && this.encoding !== 'utf16be') {
          if (inQuotes && i + 1 < buffer.length && buffer[i + 1] === 0x22) {
            i++;
          } else {
            inQuotes = !inQuotes;
          }
        } else if (ch === 0x0A && !inQuotes) {
          const offset = bytePos + i + 1;
          if (offset < this.fileSize) this.offsets.push(offset);
          prevByteWasCR = false;
        } else if (ch === 0x0D && !inQuotes) {
          if (i + 1 < buffer.length && buffer[i + 1] === 0x0A) {
            i++;
            const offset = bytePos + i + 1;
            if (offset < this.fileSize) this.offsets.push(offset);
            prevByteWasCR = false;
          } else {
            prevByteWasCR = true;
            const offset = bytePos + i + 1;
            if (offset < this.fileSize) this.offsets.push(offset);
          }
        } else {
          prevByteWasCR = false;
        }
      }

      bytePos += buffer.length;

      if (bytePos - lastProgress >= CHUNK_SIZE * 20) {
        lastProgress = bytePos;
        this.sendProgress({ current: bytePos, total: this.fileSize, percent: Math.round((bytePos / this.fileSize) * 100) });
      }
    }

    while (this.offsets.length > 1 && this.offsets[this.offsets.length - 1] >= this.fileSize) {
      this.offsets.pop();
    }
  }

  async readRowBytes(rowIndex) {
    if (rowIndex >= this.offsets.length) return '';
    const start = this.offsets[rowIndex];
    const end = rowIndex + 1 < this.offsets.length ? this.offsets[rowIndex + 1] : this.fileSize;
    if (end <= start) return '';

    const length = end - start;
    const buffer = Buffer.allocUnsafe(length);
    await this.fd.read(buffer, 0, length, start);

    if (this.encoding === 'utf16le') {
      return buffer.toString('utf16le');
    }
    return buffer.toString('utf8');
  }

  getRows(startRow, count) {
    return this._getRowsInternal(startRow, count);
  }

  async _getRowsInternal(startRow, count) {
    const results = [];
    const uncachedRanges = [];

    for (let i = 0; i < count; i++) {
      const rowIndex = startRow + i + 1;
      if (rowIndex >= this.offsets.length) break;

      if (this.cache.has(rowIndex)) {
        const cached = this.cache.get(rowIndex);
        const idx = this.cacheKeys.indexOf(rowIndex);
        if (idx !== -1) this.cacheKeys.splice(idx, 1);
        this.cacheKeys.push(rowIndex);
        results.push({ index: i, data: cached });
        continue;
      }

      if (uncachedRanges.length > 0 && uncachedRanges[uncachedRanges.length - 1].endRow === rowIndex - 1) {
        uncachedRanges[uncachedRanges.length - 1].endRow = rowIndex;
        uncachedRanges[uncachedRanges.length - 1].indices.push(i);
      } else {
        uncachedRanges.push({ startRow: rowIndex, endRow: rowIndex, indices: [i] });
      }
    }

    const MAX_BATCH = 4 * 1024 * 1024;

    for (const range of uncachedRanges) {
      const startByte = this.offsets[range.startRow];
      const endByte = range.endRow + 1 < this.offsets.length
        ? this.offsets[range.endRow + 1]
        : this.fileSize;
      const length = endByte - startByte;

      if (length > MAX_BATCH || range.startRow === range.endRow) {
        for (let r = range.startRow; r <= range.endRow; r++) {
          const rowText = await this.readRowBytes(r);
          const parsed = parseCSVLine(rowText, this.delimiter);
          results.push({ index: range.indices[r - range.startRow], data: parsed });
          this.addToCache(r, parsed.map(c => truncateCell(c)));
        }
      } else {
        const buffer = Buffer.allocUnsafe(length);
        await this.fd.read(buffer, 0, length, startByte);
        const text = this.encoding === 'utf16le'
          ? buffer.toString('utf16le')
          : buffer.toString('utf8');
        const lines = splitCSVRows(text);
        for (let j = 0; j < lines.length && j < range.indices.length; j++) {
          const parsed = parseCSVLine(lines[j], this.delimiter);
          results.push({ index: range.indices[j], data: parsed });
          this.addToCache(range.startRow + j, parsed.map(c => truncateCell(c)));
        }
      }
    }

    results.sort((a, b) => a.index - b.index);
    return results.map(r => ({
      cells: r.data.map(cell => truncateCell(cell)),
      lengths: r.data.map(cell => cell.length),
    }));
  }

  async getRowsByIndex(rowIndices) {
    if (rowIndices.length === 0) return [];
    const sorted = [...rowIndices].sort((a, b) => a - b);
    const results = [];

    let rangeStart = sorted[0];
    let rangeEnd = sorted[0];

    for (let i = 1; i <= sorted.length; i++) {
      if (i < sorted.length && sorted[i] === rangeEnd + 1) {
        rangeEnd = sorted[i];
      } else {
        for (let r = rangeStart; r <= rangeEnd; r++) {
          const offsetIdx = r + 1;
          if (offsetIdx >= this.offsets.length) continue;
          if (this.cache.has(offsetIdx)) {
            results.push({ index: r, data: this.cache.get(offsetIdx) });
          } else {
            const rowText = await this.readRowBytes(offsetIdx);
            const parsed = parseCSVLine(rowText, this.delimiter);
            this.addToCache(offsetIdx, parsed);
            results.push({ index: r, data: parsed });
          }
        }
        if (i < sorted.length) {
          rangeStart = sorted[i];
          rangeEnd = sorted[i];
        }
      }
    }

    results.sort((a, b) => a.index - b.index);
    return results.map(r => r.data);
  }

  async exportCSV(outputPath, colIndices, startRow, endRow) {
    const fd = await fs.promises.open(outputPath, 'w');
    try {
      await fd.write(Buffer.from([0xEF, 0xBB, 0xBF]));
      const headerLine = colIndices.map(i => csvQuote(this.headers[i] || '')).join(',');
      await fd.write(Buffer.from(headerLine + '\n', 'utf8'));

      for (let r = startRow; r <= endRow; r++) {
        if (r % 100 === 0) {
          this.sendProgress({ export: true, current: r - startRow, total: endRow - startRow + 1 });
        }
        const rowText = await this.readRowBytes(r + 1);
        const parsed = parseCSVLine(rowText, this.delimiter);
        const line = colIndices.map(i => csvQuote(parsed[i] || '')).join(',');
        await fd.write(Buffer.from(line + '\n', 'utf8'));
      }
      this.sendProgress({ export: true, done: true });
    } finally {
      await fd.close();
    }
  }

  addToCache(rowIndex, data) {
    this.cache.set(rowIndex, data);
    const idx = this.cacheKeys.indexOf(rowIndex);
    if (idx !== -1) this.cacheKeys.splice(idx, 1);
    this.cacheKeys.push(rowIndex);
    if (this.cacheKeys.length > this.cacheMax) {
      const oldest = this.cacheKeys.shift();
      this.cache.delete(oldest);
    }
  }

  async getCellContent(rowIndex, colIndex) {
    const dataRowIndex = rowIndex + 1;
    if (dataRowIndex >= this.offsets.length) return '';
    const rowText = await this.readRowBytes(dataRowIndex);
    const parsed = parseCSVLine(rowText, this.delimiter);
    return parsed[colIndex] || '';
  }

  async updateCell(rowIndex, colIndex, newContent) {
    const dataRowIndex = rowIndex + 1;
    if (dataRowIndex >= this.offsets.length) throw new Error('Row out of range');

    const rowCount = this.offsets.length - 1;
    const lines = [];
    for (let i = 0; i <= rowCount; i++) {
      let text = await this.readRowBytes(i);
      text = text.replace(/[\r\n]+$/, '');
      lines.push(text);
    }

    const targetLine = lines[dataRowIndex];
    const parsed = parseCSVLine(targetLine, this.delimiter);
    if (colIndex >= parsed.length) {
      while (parsed.length <= colIndex) parsed.push('');
    }
    parsed[colIndex] = newContent;
    lines[dataRowIndex] = parsed.map(c => csvQuote(c)).join(this.delimiter);

    const fd = await fs.promises.open(this.filePath, 'w');
    try {
      if (this.encoding === 'utf16le') {
        await fd.write(Buffer.from([0xFF, 0xFE]));
      } else {
        await fd.write(Buffer.from([0xEF, 0xBB, 0xBF]));
      }
      for (let i = 0; i < lines.length; i++) {
        const line = lines[i];
        if (i === this.offsets.length - 1 && line === '') continue;
        await fd.write(Buffer.from(line + '\n', this.encoding === 'utf16le' ? 'utf16le' : 'utf8'));
      }
    } finally {
      await fd.close();
    }

    this.cache.set(dataRowIndex, parsed);
    this.cache.clear();
    this.cacheKeys = [];
    this.offsets = [];
    await this.buildIndex();
  }

  async close() {
    if (this.fd) {
      await this.fd.close();
      this.fd = null;
    }
    this.offsets = [];
    this.headers = [];
    this.cache.clear();
    this.cacheKeys = [];
  }
}

// ─── Worker Message Loop ────────────────────────────────────────────────────

let engine = null;

parentPort.on('message', async (msg) => {
  const { id, action, args } = msg;

  try {
    switch (action) {
      case 'open': {
        if (engine) await engine.close();
        engine = new CsvEngine();
        const result = await engine.open(args[0]);
        parentPort.postMessage({ id, result });
        break;
      }

      case 'get-rows': {
        if (!engine) { parentPort.postMessage({ id, error: 'No file open' }); break; }
        const result = await engine.getRows(args[0], args[1]);
        parentPort.postMessage({ id, result });
        break;
      }

      case 'get-rows-by-index': {
        if (!engine) { parentPort.postMessage({ id, error: 'No file open' }); break; }
        const result = await engine.getRowsByIndex(args[0]);
        parentPort.postMessage({ id, result });
        break;
      }

      case 'get-cell-content': {
        if (!engine) { parentPort.postMessage({ id, result: '' }); break; }
        const result = await engine.getCellContent(args[0], args[1]);
        parentPort.postMessage({ id, result });
        break;
      }

      case 'export-csv': {
        if (!engine) { parentPort.postMessage({ id, error: 'No file open' }); break; }
        await engine.exportCSV(args[0], args[1], args[2], args[3]);
        parentPort.postMessage({ id, result: { ok: true } });
        break;
      }

      case 'update-cell': {
        if (!engine) { parentPort.postMessage({ id, error: 'No file open' }); break; }
        await engine.updateCell(args[0], args[1], args[2]);
        parentPort.postMessage({ id, result: { ok: true } });
        break;
      }

      case 'close': {
        if (engine) { await engine.close(); engine = null; }
        parentPort.postMessage({ id, result: null });
        break;
      }

      default:
        parentPort.postMessage({ id, error: 'Unknown action: ' + action });
    }
  } catch (err) {
    parentPort.postMessage({ id, error: err.message });
  }
});
