/**
 * The Monaco editor itself, loaded only when the source view is opened.
 *
 * Monaco is a few megabytes and reaches for browser APIs at import time, so it is not part of
 * the Portal's main bundle and not something every other view has to carry: this module is
 * imported lazily, and everything that can be decided without an editor lives in
 * `LinkmlSourceEditor` instead.
 */
import type { JSX } from "react";
import Editor from "@monaco-editor/react";
import type { OnMount } from "@monaco-editor/react";
import "./monaco-setup";

export interface MonacoSourceViewProps {
  value: string;
  onChange: (value: string) => void;
  onMount: OnMount;
  height: string;
}

export default function MonacoSourceView({
  value,
  onChange,
  onMount,
  height,
}: MonacoSourceViewProps): JSX.Element {
  return (
    <Editor
      height={height}
      language="yaml"
      value={value}
      onChange={(next) => onChange(next ?? "")}
      onMount={onMount}
      options={{
        minimap: { enabled: false },
        fontSize: 13,
        tabSize: 2,
        scrollBeyondLastLine: false,
        renderWhitespace: "boundary",
      }}
    />
  );
}
