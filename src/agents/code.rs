//! A code run's rules (Architecture/20 §4.1, SDK-11…SDK-14): which paths the model may write,
//! what keeps a project from being a preview, and the system prompt of the one call that
//! writes the application over the SDK template. The driver that uses them is in `oneshot`.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use super::preview;
use super::transpile;

/// The row types Model Tools renders from the endpoint's model; never the model's (SDK-10).
pub const TYPES: &str = "src/jc-types.ts";
/// Writable files a project may hold, and their bytes together (SDK-11).
pub const MAX_FILES: usize = 80;
pub const MAX_BYTES: usize = 800_000;

/// What a refused block is told (SDK-11).
pub const REFUSAL: &str = "not a path the application may write: src/**/*.tsx, src/**/*.ts, \
     src/**/*.css, src/design-tokens.json or functions/**/*.ts, never src/main.tsx or \
     src/jc-types.ts";

/// The SDK as the model reads it: its API with every signature, and the names it exports.
pub const SDK_API: &str = include_str!("../../sdk/API.md");
pub const SDK_EXPORTS: &str = include_str!("../../sdk/src/sdk/index.ts");

/// Whether the model may write `path` (SDK-11).
pub fn writable(path: &str) -> bool {
    if path
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
        || path == preview::MAIN
        || path == TYPES
    {
        return false;
    }
    let under = |folder: &str, extensions: &[&str]| {
        path.starts_with(folder) && extensions.iter().any(|ext| path.ends_with(ext))
    };
    under("src/", &[".tsx", ".ts", ".css"])
        || path == "src/design-tokens.json"
        || under("functions/", &[".ts"])
}

/// Everything that keeps `files` from being a preview, one line each with its file and line:
/// the limits of SDK-11, then every transpile error and refused import (SDK-12, SDK-14).
pub fn problems(files: &BTreeMap<String, String>) -> Vec<String> {
    let mut problems = Vec::new();
    let written: Vec<&String> = files
        .iter()
        .filter(|(path, _)| writable(path))
        .map(|(_, content)| content)
        .collect();
    if written.len() > MAX_FILES {
        problems.push(format!(
            "the application has {} writable files; at most {MAX_FILES}",
            written.len()
        ));
    }
    let bytes: usize = written.iter().map(|content| content.len()).sum();
    if bytes > MAX_BYTES {
        problems.push(format!(
            "the writable files hold {bytes} bytes; at most {MAX_BYTES}"
        ));
    }
    let mut code: BTreeMap<String, String> = files
        .iter()
        .filter(|(path, _)| path.starts_with("src/") || path.starts_with("functions/"))
        .map(|(path, content)| (path.clone(), content.clone()))
        .collect();
    // The entry is the Portal's whatever the files hold, as the preview document builds it.
    if let Some(main) = preview::Template::get(preview::MAIN) {
        code.insert(
            preview::MAIN.to_owned(),
            String::from_utf8_lossy(&main.data).into_owned(),
        );
    }
    problems.extend(
        transpile::transpile(&code)
            .problems
            .iter()
            .map(ToString::to_string),
    );
    problems
}

pub static SYSTEM: LazyLock<String> = LazyLock::new(|| {
    let names = |list: &[&str]| {
        list.iter()
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        r#"# SYSTEM INSTRUCTION: AN APPLICATION ON THE JOINEDCONTEXT APP SDK — SEARCH/REPLACE FORMAT

You write a React 19 + TypeScript application over a template that already runs: an app shell
with pages, an overview, a page per entity type with filters, a table, a map, charts, a detail
card, a form and exports, a backend function, and a test beside each of them. The user message
holds every file of the project as it stands, the SDK's API, the row types of the endpoint, the
data needs, what the application may write, five entities per type and the request.

You answer ONCE per call; a script applies your answer mechanically. There is no tool and no
follow-up question. A run takes several calls: a small first version, then the rest of the
application, then repairs; the section THIS CALL of the user message says what this one asks for.

## WHAT TO BUILD

- Everything the request and the data call for, over the calls of the run: every page, filter, chart,
  map, table, form, export and function they need. A new page is an entry in `pages` in
  `src/App.tsx`.
- The template is scaffolding, not the design. Give this application its own: a composition
  that fits the request and the data (what opens first, a summary or a hero, which maps, charts,
  tables, cards and filters, in what order), its own accent colours, surfaces and type scale in
  `src/design-tokens.json` and `src/app.css`, its own titles and copy. Two requests over the same
  endpoint must not look alike.
- Build with the SDK hooks and the components in `src/components/`, restyled or changed when the
  design needs it. Rewrite `src/App.tsx` and the pages freely; delete a template page, component
  or function the application does not use, together with its test.
- A later instruction may change the design as freely as the first answer did.
- Unless THIS CALL asks for a first version without tests: a test beside every page, component
  and function you add or change (`*.test.tsx`,
  `*.test.ts`), written like the template's tests: vitest, @testing-library/react,
  `stubClient` or `fakeContext` from `@joinedcontext/sdk/testing`.
- Types and attribute names exactly as `src/jc-types.ts` and the samples spell them; import the
  row types with `import type`.
- A form or any save only when the user message says the application may write.
- Titles and labels in the language of the request.

## WHAT YOU MAY WRITE

`src/**/*.tsx`, `src/**/*.ts`, `src/**/*.css`, `src/design-tokens.json` and `functions/**/*.ts`,
at most {MAX_FILES} files and {MAX_BYTES} bytes together. Never `src/main.tsx` (the Portal owns
the entry), never `src/jc-types.ts` (rendered from the endpoint's model), never `package.json`
or any configuration: a block for another path is refused.

## WHAT YOU MAY IMPORT

- Under `src/`: a relative file, {interface}.
- Under `functions/`: a relative file under `functions/`, {function}.
- In a test, additionally: {test}.
- `import type` from anywhere in the project.
Any other import is refused before a preview exists. Data goes through the SDK, never `fetch`.

## THE FORMAT RULES

1. Every block is four markers in order: the file path alone on the line right before
   `<<<<<<< SEARCH`, then `=======`, then `>>>>>>> REPLACE`. A block without its path line or
   its closing marker is not read and changes nothing.
2. To CREATE a file or REWRITE it whole, leave the SEARCH block empty. Do that for every new
   file and for every file that changes in more than a few places.
3. For a small edit, SEARCH holds at least 3 consecutive lines copied exactly from the current
   file, unique in it; REPLACE holds the new lines only.
4. Before the first block, write one or two plain sentences for the person reading the chat:
   what the application does and what you changed. After the last block, nothing. Do not wrap
   the blocks in a markdown fence.
5. If something the request asks for is out of reach (a login, a file upload, another data
   source), say so in those sentences and build the nearest thing. Raw HTML or a static page
   is a page component with that markup and its own CSS file under `src/`, said in one sentence.
6. Do what the message asks and no more. A question gets its answer in the sentences, with no
   block when nothing has to change; a small request ("put a smiley on the dashboard") is a
   small edit in place, never a new page, generator or export format nobody asked for.

## THE SYNTAX

```text
src/pages/Stations.tsx
<<<<<<< SEARCH
=======
import type {{ Row }} from "@joinedcontext/sdk";
…
>>>>>>> REPLACE
src/App.tsx
<<<<<<< SEARCH
      {{ id: "overview", label: "Overview", render: () => <Overview schema={{schema}} /> }},
=======
      {{ id: "overview", label: "Overview", render: () => <Overview schema={{schema}} /> }},
      {{ id: "stations", label: "Stations", render: () => <Stations /> }},
>>>>>>> REPLACE
```
"#,
        interface = names(transpile::INTERFACE),
        function = names(transpile::FUNCTION),
        test = names(transpile::TEST),
    )
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_writes_interface_code_tokens_and_functions_but_not_the_entry_or_the_types() {
        for path in [
            "src/App.tsx",
            "src/pages/Stations.test.tsx",
            "src/lib/rows.ts",
            "src/app.css",
            "src/design-tokens.json",
            "functions/summary.ts",
            "functions/lib/stats.ts",
        ] {
            assert!(writable(path), "{path}");
        }
        for path in [
            "src/main.tsx",
            "src/jc-types.ts",
            "package.json",
            "index.html",
            "vite.config.ts",
            "src/data.json",
            "functions/summary.js",
            "src/../package.json",
            "src//App.tsx",
            "./src/App.tsx",
            "tests/App.tsx",
        ] {
            assert!(!writable(path), "{path}");
        }
    }

    #[test]
    fn the_template_builds_and_what_breaks_it_is_named_with_file_and_line() {
        let mut files = preview::template_files();
        assert!(!files.is_empty(), "the template is embedded");
        assert_eq!(problems(&files), Vec::<String>::new());

        files.insert(
            "src/pages/Broken.tsx".to_owned(),
            "import axios from \"axios\";\nexport const x = axios;\n".to_owned(),
        );
        let found = problems(&files);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].starts_with("src/pages/Broken.tsx:1:"), "{found:?}");
        assert!(found[0].contains("axios"), "{found:?}");
    }

    #[test]
    fn the_prompt_hands_the_design_to_the_model_and_keeps_the_rules() {
        let system = SYSTEM.as_str();
        assert!(system.contains("The template is scaffolding, not the design."));
        assert!(system.contains("`src/design-tokens.json`"));
        assert!(system.contains("must not look alike"));
        assert!(system.contains("Raw HTML or a static page"));
        assert!(!system.contains("delete nothing that still works"));
        // The rules that keep a project a preview are still there (SDK-11, SDK-12).
        assert!(system.contains("Never `src/main.tsx`"));
        assert!(system.contains("A test beside every page"));
    }

    #[test]
    fn a_project_over_the_limits_is_a_problem() {
        let mut files = BTreeMap::new();
        for i in 0..=MAX_FILES {
            files.insert(format!("src/x{i}.css"), "a{}".to_owned());
        }
        files.insert("src/big.css".to_owned(), "a".repeat(MAX_BYTES));
        let found = problems(&files);
        assert!(
            found.iter().any(|p| p.contains("writable files")),
            "{found:?}"
        );
        assert!(found.iter().any(|p| p.contains("bytes")), "{found:?}");
    }
}
