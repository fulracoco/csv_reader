// csv-worker.js — Worker thread for CSV file operations
// Uses napi-rs native module (csv-native) for CPU-intensive work.

const { parentPort } = require('worker_threads');
const path = require('path');

const native = require(path.join(__dirname, 'native', 'csv-native.node'));
const { CsvEngine } = native;

let engine = null;

parentPort.on('message', (msg) => {
  const { id, action, args } = msg;

  try {
    switch (action) {
      case 'open': {
        if (engine) {
          engine.close();
          engine = null;
        }
        engine = new CsvEngine();
        const result = engine.open(args[0]);
        parentPort.postMessage({ id, result });
        break;
      }

      case 'get-rows': {
        if (!engine) {
          parentPort.postMessage({ id, error: 'No file open' });
          break;
        }
        const result = engine.getRows(args[0], args[1]);
        parentPort.postMessage({ id, result });
        break;
      }

      case 'get-rows-by-index': {
        if (!engine) {
          parentPort.postMessage({ id, error: 'No file open' });
          break;
        }
        const result = engine.getRowsByIndex(args[0]);
        parentPort.postMessage({ id, result });
        break;
      }

      case 'get-cell-content': {
        if (!engine) {
          parentPort.postMessage({ id, result: '' });
          break;
        }
        const result = engine.getCellContent(args[0], args[1]);
        parentPort.postMessage({ id, result });
        break;
      }

      case 'update-cell': {
        if (!engine) {
          parentPort.postMessage({ id, error: 'No file open' });
          break;
        }
        engine.updateCell(args[0], args[1], args[2]);
        parentPort.postMessage({ id, result: { ok: true } });
        break;
      }

      case 'export-csv': {
        if (!engine) {
          parentPort.postMessage({ id, error: 'No file open' });
          break;
        }
        engine.exportCsv(args[0], args[1], args[2], args[3]);
        parentPort.postMessage({ id, result: { ok: true } });
        break;
      }

      case 'close': {
        if (engine) {
          engine.close();
          engine = null;
        }
        parentPort.postMessage({ id, result: null });
        break;
      }

      default:
        parentPort.postMessage({ id, error: 'Unknown action: ' + action });
    }
  } catch (err) {
    parentPort.postMessage({ id, error: err.message || String(err) });
  }
});
