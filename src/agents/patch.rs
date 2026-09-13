//! SEARCH/REPLACE blocks, applied the way the platform's one-shot code generation applies them
//! (Architecture/19 §1.2, AP-58, AG-54).
//!
//! The format is the one `bigshot/apply_patch.js` reads and its rules are kept whole: a path
//! line, `<<<<<<< SEARCH`, the lines to find, `=======`, the replacement, `>>>>>>> REPLACE`.
//! An empty SEARCH creates or rewrites the file; a `@@QZXJK@@` line splits a long SEARCH into a
//! start anchor and an end anchor; a match is exact first and whitespace-fuzzy second; an
//! ambiguous match is refused rather than guessed. What is not in the script: the files live in
//! a map, not on a disk, and a block is refused unless its path is one the run may write. A
//! model's answer is text until this module has said which of its blocks landed.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

/// The line that splits an anchored SEARCH block into its start and its end (rule 5).
pub const GAP: &str = "@@QZXJK@@";

static BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^(\S[^\n]*)\n<{7} SEARCH\n([\s\S]*?)={7}\n([\s\S]*?)>{7} REPLACE")
        .expect("valid regex")
});
static FENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)^\s*```[a-z]*\n(.*)\n```\s*$").expect("valid regex"));

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub path: String,
    pub search: String,
    pub replace: String,
}

/// One block that landed, and how it matched.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Applied {
    pub path: String,
    pub how: String,
}

/// One block that did not land, and why. The reason is what goes back to the model.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Refused {
    pub path: String,
    pub reason: String,
}

/// The blocks of a model answer, in order, and the prose around them.
///
/// The prose is the assistant's turn of the conversation: what it built and why. A fence
/// around the whole answer is unwrapped first, as the script does; fences that wrap single
/// blocks are dropped from the prose so the person reads sentences, not markup.
pub fn parse(answer: &str) -> (Vec<Block>, String) {
    let text = match FENCE.captures(answer) {
        Some(fence) => fence[1].to_owned(),
        None => answer.to_owned(),
    };
    let mut blocks = Vec::new();
    let mut prose = String::new();
    let mut cursor = 0;
    for found in BLOCK.captures_iter(&text) {
        let whole = found.get(0).expect("the match");
        prose.push_str(&text[cursor..whole.start()]);
        cursor = whole.end();
        blocks.push(Block {
            // A path in backticks would be a directory named "`": the script strips them too.
            path: found[1].replace(['`', '\'', '"'], "").trim().to_owned(),
            search: found[2].strip_suffix('\n').unwrap_or(&found[2]).to_owned(),
            replace: found[3].strip_suffix('\n').unwrap_or(&found[3]).to_owned(),
        });
    }
    prose.push_str(&text[cursor..]);
    let prose = prose
        .lines()
        .filter(|line| !line.trim_start().starts_with("```"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();
    (blocks, prose)
}

/// Applies every block to `files`, in order. `allowed` is the whole of what a block may name;
/// everything else is refused before the file is looked at (AP-58).
///
/// A refused block leaves the others untouched: the model gets the reasons back and the files
/// that did change are what the person sees. That is the script's exit code 1 with the file
/// system it left behind, and it is deliberate: a foreign path is one wrong line, not a reason
/// to throw away the edit beside it.
pub fn apply(
    files: &mut BTreeMap<String, String>,
    blocks: &[Block],
    allowed: &[&str],
) -> (Vec<Applied>, Vec<Refused>) {
    let mut applied = Vec::new();
    let mut refused = Vec::new();
    for block in blocks {
        let path = block.path.as_str();
        if path.is_empty()
            || path.starts_with('/')
            || path.split('/').any(|part| part == "..")
            || !allowed.contains(&path)
        {
            refused.push(Refused {
                path: block.path.clone(),
                reason: format!(
                    "not a file this run may write; the files are: {}",
                    allowed.join(", ")
                ),
            });
            continue;
        }
        if block.search.trim().is_empty() {
            let how = if files.contains_key(path) {
                "Replaced"
            } else {
                "Created"
            };
            files.insert(path.to_owned(), format!("{}\n", block.replace));
            applied.push(Applied {
                path: path.to_owned(),
                how: how.to_owned(),
            });
            continue;
        }
        let Some(content) = files.get(path).cloned() else {
            refused.push(Refused {
                path: path.to_owned(),
                reason: "file not found (non-empty SEARCH)".to_owned(),
            });
            continue;
        };
        match locate(&content, &block.search) {
            Ok((start, end, how)) => {
                let mut end = end;
                // Deletion: eat the line break so no blank line is left behind.
                if block.replace.is_empty() && content[end..].starts_with('\n') {
                    end += 1;
                }
                let next = format!("{}{}{}", &content[..start], block.replace, &content[end..]);
                files.insert(path.to_owned(), next);
                applied.push(Applied {
                    path: path.to_owned(),
                    how: if block.replace.is_empty() {
                        "Deleted".to_owned()
                    } else {
                        how
                    },
                });
            }
            Err(reason) if reason == "No match" && nearly_whole(&block.search, &content) => {
                // The model pasted (nearly) the whole old file as SEARCH and one character was
                // off: it meant a rewrite, so that is what it gets.
                files.insert(path.to_owned(), format!("{}\n", block.replace));
                applied.push(Applied {
                    path: path.to_owned(),
                    how: "Rewrote (SEARCH ~ whole file, no exact match)".to_owned(),
                });
            }
            Err(reason) => refused.push(Refused {
                path: path.to_owned(),
                reason,
            }),
        }
    }
    (applied, refused)
}

fn nearly_whole(search: &str, content: &str) -> bool {
    let (s, c) = (search.len() as f64, content.len() as f64);
    s >= 0.9 * c && s <= 1.1 * c
}

/// Every `[start, end)` of `block` in `content` on whole lines: exact first, then with every
/// run of whitespace matching any run of whitespace.
fn find_all(content: &str, block: &str) -> (&'static str, Vec<(usize, usize)>) {
    let escaped = regex::escape(block);
    let fuzzy = format!(
        "[ \\t]*{}",
        Regex::new(r"\s+")
            .expect("valid")
            .replace_all(&escaped, "\\s+")
    );
    for (how, pattern) in [("Exact", escaped), ("Fuzzy", fuzzy)] {
        let Ok(re) = Regex::new(&format!("(?m)^{pattern}(?:\n|$)")) else {
            continue;
        };
        let hits: Vec<(usize, usize)> = re
            .find_iter(content)
            .map(|m| {
                (
                    m.start(),
                    m.end() - usize::from(content[..m.end()].ends_with('\n')),
                )
            })
            .collect();
        if !hits.is_empty() {
            return (how, hits);
        }
    }
    ("none", Vec::new())
}

/// The region a SEARCH block names, plain or anchored.
fn locate(content: &str, block: &str) -> Result<(usize, usize, String), String> {
    let lines: Vec<&str> = block.split('\n').collect();
    let Some(gap) = lines.iter().position(|line| line.trim() == GAP) else {
        let (how, hits) = find_all(content, block);
        return match hits.as_slice() {
            [] => Err("No match".to_owned()),
            [(start, end)] => Ok((*start, *end, how.to_owned())),
            many => Err(format!(
                "Ambiguous - {} {} matches",
                many.len(),
                how.to_lowercase()
            )),
        };
    };
    let head = lines[..gap].join("\n");
    let tail = lines[gap + 1..].join("\n");
    if head.trim().is_empty() || tail.trim().is_empty() {
        return Err(format!(
            "Anchor missing - need lines before and after {GAP}"
        ));
    }
    let (how, starts) = find_all(content, &head);
    let (start, after_head) = match starts.as_slice() {
        [] => return Err("Start anchor not found".to_owned()),
        [one] => *one,
        many => return Err(format!("Ambiguous start anchor - {} matches", many.len())),
    };
    let (_, ends) = find_all(content, &tail);
    let Some((_, end)) = ends.into_iter().find(|(begin, _)| *begin >= after_head) else {
        return Err("End anchor not found after start anchor".to_owned());
    };
    Ok((start, end, format!("Anchored/{how}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(content: &str) -> BTreeMap<String, String> {
        BTreeMap::from([("spec.json".to_owned(), content.to_owned())])
    }

    fn block(path: &str, search: &str, replace: &str) -> Block {
        Block {
            path: path.into(),
            search: search.into(),
            replace: replace.into(),
        }
    }

    #[test]
    fn parses_blocks_out_of_prose_and_a_surrounding_fence() {
        let answer = "I made the tiles bigger.\n\n```text\n`spec.json`\n<<<<<<< SEARCH\na\nb\nc\n=======\na\nB\nc\n>>>>>>> REPLACE\n\nerrors.md\n<<<<<<< SEARCH\n=======\n- nothing missing\n>>>>>>> REPLACE\n```\n";
        let (blocks, prose) = parse(answer);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0], block("spec.json", "a\nb\nc", "a\nB\nc"));
        assert_eq!(blocks[1], block("errors.md", "", "- nothing missing"));
        assert_eq!(prose, "I made the tiles bigger.");
    }

    #[test]
    fn an_empty_search_creates_and_a_later_one_replaces() {
        let mut map = BTreeMap::new();
        let (ok, bad) = apply(
            &mut map,
            &[block("spec.json", "", "{\"a\":1}")],
            &["spec.json"],
        );
        assert_eq!(ok[0].how, "Created");
        assert!(bad.is_empty());
        assert_eq!(map["spec.json"], "{\"a\":1}\n");
        let (ok, _) = apply(
            &mut map,
            &[block("spec.json", "", "{\"a\":2}")],
            &["spec.json"],
        );
        assert_eq!(ok[0].how, "Replaced");
        assert_eq!(map["spec.json"], "{\"a\":2}\n");
    }

    #[test]
    fn exact_then_fuzzy_then_ambiguous() {
        let mut map = files("{\n  \"title\": \"A\",\n  \"views\": []\n}\n");
        let (ok, _) = apply(
            &mut map,
            &[block(
                "spec.json",
                "  \"title\": \"A\",",
                "  \"title\": \"B\",",
            )],
            &["spec.json"],
        );
        assert_eq!(ok[0].how, "Exact");
        assert!(map["spec.json"].contains("\"title\": \"B\""));
        // Different indentation still lands, and says so.
        let (ok, _) = apply(
            &mut map,
            &[block(
                "spec.json",
                "\"title\":   \"B\",",
                "\"title\": \"C\",",
            )],
            &["spec.json"],
        );
        assert_eq!(ok[0].how, "Fuzzy");
        // The replacement lands as written: the fuzzy pattern swallows the file's indentation
        // and the model's REPLACE lines are what stand there, as in the original script.
        assert!(map["spec.json"].contains("\n\"title\": \"C\",\n"));
        // Two identical lines: refused, not guessed.
        let mut twice = files("x\ny\nx\n");
        let (ok, bad) = apply(&mut twice, &[block("spec.json", "x", "z")], &["spec.json"]);
        assert!(ok.is_empty());
        assert_eq!(bad[0].reason, "Ambiguous - 2 exact matches");
        assert_eq!(twice["spec.json"], "x\ny\nx\n");
    }

    #[test]
    fn an_anchored_range_replaces_from_start_to_the_first_end_after_it() {
        let mut map =
            files("{\n  \"filters\": [\n    1,\n    2\n  ],\n  \"views\": [\n    3\n  ]\n}\n");
        let search = format!("  \"filters\": [\n{GAP}\n  ],");
        let (ok, bad) = apply(
            &mut map,
            &[block("spec.json", &search, "  \"filters\": [],")],
            &["spec.json"],
        );
        assert!(bad.is_empty(), "{bad:?}");
        assert_eq!(ok[0].how, "Anchored/Exact");
        assert_eq!(
            map["spec.json"],
            "{\n  \"filters\": [],\n  \"views\": [\n    3\n  ]\n}\n"
        );
        let (_, bad) = apply(
            &mut map,
            &[block("spec.json", &format!("nope\n{GAP}\n  ],"), "")],
            &["spec.json"],
        );
        assert_eq!(bad[0].reason, "Start anchor not found");
    }

    #[test]
    fn a_deletion_takes_its_line_break_with_it() {
        let mut map = files("a\nb\nc\n");
        let (ok, _) = apply(&mut map, &[block("spec.json", "b", "")], &["spec.json"]);
        assert_eq!(ok[0].how, "Deleted");
        assert_eq!(map["spec.json"], "a\nc\n");
    }

    #[test]
    fn a_path_the_run_may_not_write_is_refused_and_the_rest_still_lands() {
        let mut map = files("a\n");
        let blocks = [
            block("../etc/passwd", "", "x"),
            block("/spec.json", "", "x"),
            block("kit/src/App.tsx", "", "x"),
            block("spec.json", "a", "b"),
            block("missing.json", "a", "b"),
        ];
        let (ok, bad) = apply(&mut map, &blocks, &["spec.json", "missing.json"]);
        assert_eq!(ok.len(), 1);
        assert_eq!(bad.len(), 4);
        assert!(bad
            .iter()
            .take(3)
            .all(|r| r.reason.starts_with("not a file this run may write")));
        assert_eq!(bad[3].reason, "file not found (non-empty SEARCH)");
        assert_eq!(map.len(), 1);
        assert_eq!(map["spec.json"], "b\n");
    }

    #[test]
    fn a_search_that_is_nearly_the_whole_file_is_a_rewrite() {
        let mut map = files("{\n  \"title\": \"Helsinki bikes\",\n  \"views\": [1, 2, 3]\n}\n");
        let drifted = "{\n  \"title\": \"Helsinki bikes\",\n  \"views\": [1, 2, 4]\n}";
        let (ok, bad) = apply(
            &mut map,
            &[block("spec.json", drifted, "{}")],
            &["spec.json"],
        );
        assert!(bad.is_empty());
        assert!(ok[0].how.starts_with("Rewrote"));
        assert_eq!(map["spec.json"], "{}\n");
    }
}
