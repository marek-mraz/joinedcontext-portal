/// <reference types="vite/client" />
/**
 * Monaco, bundled with the Portal rather than fetched from a CDN.
 *
 * `@monaco-editor/react` loads the editor from jsDelivr by default. A municipal portal runs
 * behind its own Content Security Policy and, on some installations, with no route to the
 * public internet at all, so the loader is pointed at the copy in this bundle and the worker
 * is the one Vite emits.
 */
import { loader } from "@monaco-editor/react";
import * as monaco from "monaco-editor";
import EditorWorker from "monaco-editor/editor/editor.worker.js?worker";

(self as unknown as { MonacoEnvironment?: { getWorker: () => Worker } }).MonacoEnvironment = {
  getWorker: () => new EditorWorker(),
};

loader.config({ monaco });
