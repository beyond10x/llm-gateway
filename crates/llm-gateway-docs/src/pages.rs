//! Checks over the hand-written pages under `website/docs`.
//!
//! - No admonition carries a raw title (`:::caution Planned`): the Docs System guard fails the
//!   build on that form, and Docusaurus prints it as text.
//! - No page names a planning-store story (`story:<id>`) or a home-directory path (`/home/<name>`):
//!   the site is public, and the planning store and the machine that built it are not.

use std::{fs, path::Path};

use crate::{DOCS, Result};

/// Whether `line` matches `^:::[a-z]+ +\S`: an admonition whose title is not in brackets.
fn is_raw_admonition(line: &str) -> bool {
    let Some(rest) = line.strip_prefix(":::") else {
        return false;
    };
    let kind = rest.bytes().take_while(u8::is_ascii_lowercase).count();
    let rest = &rest[kind..];
    let spaces = rest.bytes().take_while(|b| *b == b' ').count();
    kind > 0
        && spaces > 0
        && rest[spaces..]
            .chars()
            .next()
            .is_some_and(|c| !c.is_whitespace())
}

/// Whether `line` names a story id: `story:` followed by a lowercase letter or digit.
fn names_a_story(line: &str) -> bool {
    line.match_indices("story:").any(|(at, _)| {
        line[at + "story:".len()..]
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}

/// Every problem in one page: `page:line: problem`.
fn page_problems(name: &str, page: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let mut in_code = false;
    for (index, line) in page.lines().enumerate() {
        let at = index + 1;
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
        } else if !in_code && is_raw_admonition(line) {
            problems.push(format!(
                "{name}:{at}: raw admonition title (write `:::kind[Title]`)"
            ));
        }
        if names_a_story(line) {
            problems.push(format!("{name}:{at}: names a planning-store story"));
        }
        if line.contains("/home/") {
            problems.push(format!(
                "{name}:{at}: names a home-directory path (write `~/`)"
            ));
        }
    }
    if in_code {
        problems.push(format!("{name}: unclosed code block"));
    }
    problems
}

fn walk(root: &Path, dir: &Path, problems: &mut Vec<String>) -> Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .map_err(|error| format!("reading {}: {error}", dir.display()))?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            walk(root, &path, problems)?;
        } else if path
            .extension()
            .is_some_and(|ext| ext == "md" || ext == "mdx")
        {
            let page = fs::read_to_string(&path)
                .map_err(|error| format!("reading {}: {error}", path.display()))?;
            let name = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            problems.extend(page_problems(&name, &page));
        }
    }
    Ok(())
}

/// Every problem in the pages under `website/docs`.
pub(crate) fn check(root: &Path) -> Result<Vec<String>> {
    let mut problems = Vec::new();
    walk(root, &root.join(DOCS), &mut problems)?;
    Ok(problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_admonition_titles_are_found_and_bracketed_ones_pass() {
        for raw in [
            ":::caution Planned",
            ":::note  These",
            ":::info Decided design",
        ] {
            assert!(is_raw_admonition(raw), "{raw}");
        }
        for fine in [
            ":::caution[Planned]",
            ":::",
            ":::note",
            ":::note ",
            "::: note T",
            " :::note T",
        ] {
            assert!(!is_raw_admonition(fine), "{fine}");
        }
    }

    #[test]
    fn a_story_id_or_a_home_path_is_found_anywhere_and_an_admonition_outside_code() {
        let page = "# Guide\n\nSee story:docs-site.\n\n```text\n:::note Inside\n/home/me/x\n```\n:::note Outside\n";
        assert_eq!(
            page_problems("g.md", page),
            [
                "g.md:3: names a planning-store story",
                "g.md:7: names a home-directory path (write `~/`)",
                "g.md:9: raw admonition title (write `:::kind[Title]`)",
            ]
        );
        assert!(page_problems("g.md", "a user story: none\n").is_empty());
        assert_eq!(
            page_problems("g.md", "```\nopen\n"),
            ["g.md: unclosed code block"]
        );
    }
}
