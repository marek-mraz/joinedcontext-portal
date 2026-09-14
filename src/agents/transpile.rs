//! The code preview's transpiler (Architecture/20 §5, SDK-12, SDK-15).
//!
//! Every TypeScript file of a code run is parsed, stripped of its types and compiled for the
//! automatic JSX runtime in this process. Its imports are read on what is left after the
//! transform, so a type-only import, which the transform removes, is allowed anywhere, and
//! whatever would run is checked against the names SDK-12 allows. A relative import becomes the
//! import-map name of the file it resolves to, because the preview holds each file as a `data:`
//! module that has no address to resolve `./x` against.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use oxc::allocator::{Allocator, FromIn};
use oxc::ast::ast::{
    ExportAllDeclaration, ExportFromDeclaration, Expression, ImportDeclaration, ImportExpression,
    Statement, StringLiteral,
};
use oxc::ast_visit::{walk_mut, VisitMut};
use oxc::codegen::Codegen;
use oxc::diagnostics::OxcDiagnostic;
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::SourceType;
use oxc::str::Str;
use oxc::transformer::{TransformOptions, Transformer};

/// The import-map prefix of a project file: `src/App.tsx` is `@app/src/App.tsx`.
pub const APP: &str = "@app/";

/// What interface code may import besides its own files (SDK-12).
pub const INTERFACE: &[&str] = &[
    "react",
    "react-dom/client",
    "react/jsx-runtime",
    "echarts",
    "recharts",
    "maplibre-gl",
    "@deck.gl/core",
    "@deck.gl/layers",
    "@deck.gl/aggregation-layers",
    "@deck.gl/mapbox",
    "@joinedcontext/sdk",
];
/// What a function may import besides the files under `functions/`.
pub const FUNCTION: &[&str] = &["@joinedcontext/sdk/server"];
/// What a test may import on top of what its folder may.
pub const TEST: &[&str] = &[
    "vitest",
    "@testing-library/react",
    "@joinedcontext/sdk/testing",
];
/// The SDK stylesheet: imported by `src/main.tsx`, inlined by the document, removed here.
const SDK_STYLE: &str = "@joinedcontext/sdk/style.css";

/// One thing wrong with one file, where it is: what goes back to the model (SDK-14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: {}",
            self.file, self.line, self.column, self.message
        )
    }
}

/// A code run, transpiled: its interface for the document and its functions for the runtime.
#[derive(Debug, Default)]
pub struct Project {
    /// Import-map name → module code, for every interface file and JSON file under `src/`.
    pub modules: BTreeMap<String, String>,
    /// Import name → module code, for every function file under `functions/` but its tests.
    pub functions: BTreeMap<String, String>,
    /// The stylesheets under `src/`, in path order.
    pub styles: Vec<String>,
    /// Everything that keeps the preview from being built, every file checked.
    pub problems: Vec<Problem>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Folder {
    Src,
    Functions,
}

/// Transpiles the interface, checks the functions and tests, and names every problem at once.
pub fn transpile(files: &BTreeMap<String, String>) -> Project {
    let mut project = Project::default();
    for (path, source) in files {
        let folder = if path.starts_with("src/") {
            Folder::Src
        } else if path.starts_with("functions/") {
            Folder::Functions
        } else {
            continue;
        };
        if path.ends_with(".css") && folder == Folder::Src {
            project.styles.push(source.clone());
        } else if path.ends_with(".json") && folder == Folder::Src {
            match serde_json::from_str::<serde_json::Value>(source) {
                Ok(value) => {
                    project
                        .modules
                        .insert(format!("{APP}{path}"), format!("export default {value};\n"));
                }
                Err(error) => project.problems.push(Problem {
                    file: path.clone(),
                    line: error.line(),
                    column: error.column(),
                    message: error.to_string(),
                }),
            }
        } else if (path.ends_with(".ts") || path.ends_with(".tsx")) && !path.ends_with(".d.ts") {
            let test = path.contains(".test.");
            let code = module(path, source, files, folder, test, &mut project.problems);
            match code {
                Some(code) if !test && folder == Folder::Src => {
                    project.modules.insert(format!("{APP}{path}"), code);
                }
                Some(code) if !test => {
                    project.functions.insert(format!("{APP}{path}"), code);
                }
                _ => {}
            }
        }
    }
    project
}

fn module(
    path: &str,
    source: &str,
    files: &BTreeMap<String, String>,
    folder: Folder,
    test: bool,
    problems: &mut Vec<Problem>,
) -> Option<String> {
    let before = problems.len();
    let report = |problems: &mut Vec<Problem>, errors: &[OxcDiagnostic]| {
        for error in errors {
            let offset = error.labels.first().map_or(0, |label| label.offset());
            problems.push(problem(path, source, offset, error.message.to_string()));
        }
    };
    let allocator = Allocator::default();
    let source_type = if path.ends_with(".tsx") {
        SourceType::tsx()
    } else {
        SourceType::ts()
    };
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if !parsed.diagnostics.is_empty() {
        report(problems, &parsed.diagnostics);
        return None;
    }
    let mut program = parsed.program;
    let semantic = SemanticBuilder::new()
        .with_check_syntax_error(true)
        .build(&program);
    if !semantic.diagnostics.is_empty() {
        report(problems, &semantic.diagnostics);
        return None;
    }
    let scoping = semantic.semantic.into_scoping();
    let transformed = Transformer::new(&allocator, Path::new(path), &TransformOptions::default())
        .build_with_scoping(scoping, &mut program);
    if !transformed.diagnostics.is_empty() {
        report(problems, &transformed.diagnostics);
        return None;
    }
    let mut imports = Imports {
        allocator: &allocator,
        path,
        source,
        files,
        folder,
        test,
        problems,
    };
    imports.visit_program(&mut program);
    // A stylesheet import is left with an empty name by the visitor: the document inlines it.
    program.body.retain(|statement| {
        !matches!(statement, Statement::ImportDeclaration(decl) if decl.source.value.is_empty())
    });
    (problems.len() == before).then(|| Codegen::new().build(&program).code)
}

fn problem(path: &str, source: &str, offset: u32, message: String) -> Problem {
    let before = &source[..(offset as usize).min(source.len())];
    let line_start = before.rfind('\n').map_or(0, |at| at + 1);
    Problem {
        file: path.to_owned(),
        line: before.matches('\n').count() + 1,
        column: before[line_start..].chars().count() + 1,
        message,
    }
}

struct Imports<'a, 'p> {
    allocator: &'a Allocator,
    path: &'p str,
    source: &'p str,
    files: &'p BTreeMap<String, String>,
    folder: Folder,
    test: bool,
    problems: &'p mut Vec<Problem>,
}

/// What becomes of one import.
enum Target {
    /// The import-map name it is written as.
    Name(String),
    /// Removed: a stylesheet the document inlines.
    Style,
}

impl<'a> Imports<'a, '_> {
    fn allowed(&self) -> Vec<&'static str> {
        let mut names = match self.folder {
            Folder::Src => INTERFACE.to_vec(),
            Folder::Functions => FUNCTION.to_vec(),
        };
        if self.test {
            names.extend_from_slice(TEST);
        }
        names
    }

    fn target(&self, specifier: &str, bare_import: bool) -> Result<Target, String> {
        if !specifier.starts_with("./") && !specifier.starts_with("../") {
            let allowed = self.allowed();
            if allowed.contains(&specifier) {
                return Ok(Target::Name(specifier.to_owned()));
            }
            if specifier == SDK_STYLE && self.folder == Folder::Src && bare_import {
                return Ok(Target::Style);
            }
            return Err(format!(
                "'{specifier}' may not be imported here; this file may import its own relative files and {}",
                allowed.join(", ")
            ));
        }
        let dir = self.path.rsplit_once('/').map_or("", |(dir, _)| dir);
        let joined = normalize(&format!("{dir}/{specifier}"))
            .ok_or_else(|| format!("'{specifier}' leaves the project"))?;
        let root = match self.folder {
            Folder::Src => "src/",
            Folder::Functions => "functions/",
        };
        if !joined.starts_with(root) {
            return Err(format!(
                "'{specifier}' leaves {root}; a file here imports only files under {root}"
            ));
        }
        let found = [
            joined.clone(),
            format!("{joined}.tsx"),
            format!("{joined}.ts"),
            format!("{joined}/index.tsx"),
            format!("{joined}/index.ts"),
        ]
        .into_iter()
        .find(|candidate| self.files.contains_key(candidate))
        .ok_or_else(|| format!("'{specifier}' names no file of the project"))?;
        if found.ends_with(".css") {
            return if bare_import {
                Ok(Target::Style)
            } else {
                Err(format!(
                    "'{specifier}' is a stylesheet: import it for its effect only, `import \"{specifier}\"`"
                ))
            };
        }
        Ok(Target::Name(format!("{APP}{found}")))
    }

    fn rewrite(&mut self, literal: &mut StringLiteral<'a>, bare_import: bool) {
        match self.target(&literal.value, bare_import) {
            Ok(Target::Name(name)) => {
                literal.value = Str::from_in(name.as_str(), self.allocator);
                literal.raw = None;
            }
            Ok(Target::Style) => {
                literal.value = Str::from_in("", self.allocator);
                literal.raw = None;
            }
            Err(message) => {
                let problem = problem(self.path, self.source, literal.span.start, message);
                self.problems.push(problem);
            }
        }
    }
}

impl<'a> VisitMut<'a> for Imports<'a, '_> {
    fn visit_import_declaration(&mut self, decl: &mut ImportDeclaration<'a>) {
        let bare = decl.specifiers.as_ref().is_none_or(|s| s.is_empty());
        self.rewrite(&mut decl.source, bare);
        // A JSON file is a JavaScript module in the preview; an attribute would ask for JSON.
        if decl.source.value.ends_with(".json") {
            decl.with_clause = None;
        }
    }

    fn visit_export_from_declaration(&mut self, decl: &mut ExportFromDeclaration<'a>) {
        self.rewrite(&mut decl.source, false);
    }

    fn visit_export_all_declaration(&mut self, decl: &mut ExportAllDeclaration<'a>) {
        self.rewrite(&mut decl.source, false);
    }

    fn visit_import_expression(&mut self, expr: &mut ImportExpression<'a>) {
        match &mut expr.source {
            Expression::StringLiteral(literal) => self.rewrite(literal, false),
            _ => {
                let problem = problem(
                    self.path,
                    self.source,
                    expr.span.start,
                    "import() takes a string literal, so the import can be checked".to_owned(),
                );
                self.problems.push(problem);
            }
        }
        walk_mut::walk_import_expression(self, expr);
    }
}

/// `src/pages/../App` → `src/App`; `None` when `..` climbs above the project.
fn normalize(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(path, text)| ((*path).to_owned(), (*text).to_owned()))
            .collect()
    }

    #[test]
    fn types_go_jsx_compiles_and_relative_imports_become_import_map_names() {
        let project = transpile(&files(&[
            (
                "src/App.tsx",
                "import { useState } from \"react\";\nimport type { Row } from \"@joinedcontext/sdk\";\n\
                 import { Card } from \"./pages/Card\";\nimport tokens from \"./design-tokens.json\";\n\
                 import \"./app.css\";\nimport \"@joinedcontext/sdk/style.css\";\n\
                 export default function App({ rows }: { rows: Row[] }) {\n  const [n] = useState<number>(tokens.n);\n  return <Card n={n} rows={rows} />;\n}\n",
            ),
            (
                "src/pages/Card.tsx",
                "export const Card = ({ n }: { n: number }) => <p>{n}</p>;\n",
            ),
            ("src/design-tokens.json", "{\"n\": 3}"),
            ("src/app.css", ".a { color: red }"),
        ]));

        assert!(project.problems.is_empty(), "{:?}", project.problems);
        let app = &project.modules["@app/src/App.tsx"];
        assert!(app.contains("from \"@app/src/pages/Card.tsx\""), "{app}");
        assert!(
            app.contains("from \"@app/src/design-tokens.json\""),
            "{app}"
        );
        assert!(app.contains("from \"react/jsx-runtime\""), "{app}");
        assert!(!app.contains("Row"), "the type import is gone: {app}");
        assert!(
            !app.contains(".css"),
            "stylesheets are inlined, not imported: {app}"
        );
        assert!(!app.contains(": number"), "{app}");
        assert_eq!(
            project.modules["@app/src/design-tokens.json"],
            "export default {\"n\":3};\n"
        );
        assert_eq!(project.styles, [".a { color: red }"]);
    }

    #[test]
    fn a_refused_import_is_named_with_its_file_and_line() {
        let project = transpile(&files(&[
            ("src/App.tsx", "import React from \"react\";\n\nimport _ from \"lodash\";\nexport default () => _.x(React);\n"),
            ("src/lazy.ts", "export const load = (name: string) => import(name);\n"),
            ("src/up.ts", "export * from \"../functions/summary\";\n"),
            ("functions/summary.ts", "import { useState } from \"react\";\nexport default () => useState;\n"),
            ("functions/summary.test.ts", "import { it } from \"vitest\";\nimport s from \"./summary\";\nit(\"runs\", () => void s);\n"),
            ("src/App.test.tsx", "import { render } from \"@testing-library/react\";\nimport { fakeContext } from \"@joinedcontext/sdk/server\";\nvoid render; void fakeContext;\n"),
        ]));

        let named: Vec<String> = project.problems.iter().map(ToString::to_string).collect();
        assert_eq!(named.len(), 5, "{named:#?}");
        assert!(
            named[0].starts_with("functions/summary.ts:1:26: 'react' may not be imported here"),
            "{named:#?}"
        );
        assert!(
            named[1].starts_with("src/App.test.tsx:2:29: '@joinedcontext/sdk/server' may not"),
            "{named:#?}"
        );
        assert!(
            named[2].starts_with("src/App.tsx:3:15: 'lodash' may not be imported here"),
            "{named:#?}"
        );
        assert!(
            named[3].starts_with("src/lazy.ts:1:39: import() takes a string literal"),
            "{named:#?}"
        );
        assert!(
            named[4].starts_with("src/up.ts:1:15: '../functions/summary' leaves src/"),
            "{named:#?}"
        );
        assert!(
            named[2].contains("@joinedcontext/sdk"),
            "the refusal lists what is allowed"
        );
        assert!(!project.modules.contains_key("@app/src/App.tsx"));
    }

    #[test]
    fn a_syntax_error_is_named_and_a_missing_file_too() {
        let project = transpile(&files(&[
            ("src/App.tsx", "export default () => <div>;\n"),
            (
                "src/Other.tsx",
                "import { X } from \"./Nope\";\nexport const Y = X;\n",
            ),
            ("src/bad.json", "{\"a\": }"),
        ]));

        let named: Vec<String> = project.problems.iter().map(ToString::to_string).collect();
        assert_eq!(named.len(), 3, "{named:#?}");
        assert!(named[0].starts_with("src/App.tsx:1:"), "{named:#?}");
        assert!(
            named[1].starts_with("src/Other.tsx:1:19: './Nope' names no file"),
            "{named:#?}"
        );
        assert!(named[2].starts_with("src/bad.json:1:"), "{named:#?}");
    }

    #[test]
    fn sixty_files_transpile_in_under_a_second() {
        let mut entries = BTreeMap::new();
        for i in 0..60 {
            entries.insert(
                format!("src/pages/Page{i}.tsx"),
                format!(
                    "import {{ useMemo, useState }} from \"react\";\nimport {{ EntityTable, useEntities }} from \"@joinedcontext/sdk\";\n\
                     import type {{ Row }} from \"@joinedcontext/sdk\";\nimport {{ Page{} }} from \"./Page{}\";\n\
                     interface Props {{ type: string; limit?: number }}\n\
                     export function Page{i}({{ type, limit = 50 }}: Props) {{\n  const [q, setQ] = useState<string>(\"\");\n  const {{ rows }} = useEntities(type, {{ limit }});\n  const shown = useMemo(() => rows.filter((r: Row) => String(r.name ?? \"\").includes(q)), [rows, q]);\n  return (\n    <section aria-label={{type}}>\n      <input value={{q}} onChange={{(e) => setQ(e.target.value)}} />\n      <EntityTable rows={{shown}} />\n      {{shown.length === 0 ? <p>Nothing</p> : <Page{} type={{type}} />}}\n    </section>\n  );\n}}\n",
                    (i + 1) % 60,
                    (i + 1) % 60,
                    (i + 1) % 60
                ),
            );
        }
        let started = std::time::Instant::now();
        let project = transpile(&entries);
        let took = started.elapsed();

        assert!(project.problems.is_empty(), "{:?}", project.problems);
        assert_eq!(project.modules.len(), 60);
        assert!(took.as_millis() < 1000, "60 files took {took:?}");
    }

    #[test]
    fn normalize_stays_inside_the_project() {
        assert_eq!(normalize("src/pages/../App").as_deref(), Some("src/App"));
        assert_eq!(normalize("src/./a//b").as_deref(), Some("src/a/b"));
        assert_eq!(normalize("src/../../etc"), None);
    }
}
