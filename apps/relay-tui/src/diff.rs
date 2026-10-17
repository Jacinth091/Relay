use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffDocument {
    pub files: Vec<DiffFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFile {
    pub old_path: Option<PathBuf>,
    pub new_path: Option<PathBuf>,
    pub additions: usize,
    pub deletions: usize,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    pub kind: DiffLineKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
    Hunk,
    Metadata,
}

pub fn parse(input: &str) -> DiffDocument {
    let mut files = Vec::new();
    let mut current: Option<DiffFile> = None;
    let mut old_line = 0usize;
    let mut new_line = 0usize;
    for raw in input.lines() {
        if raw.starts_with("diff --git ") {
            if let Some(file) = current.take() {
                files.push(file);
            }
            let mut parts = raw.split_whitespace();
            let _ = parts.next();
            let _ = parts.next();
            let old_path = parts.next().map(strip_git_prefix).map(PathBuf::from);
            let new_path = parts.next().map(strip_git_prefix).map(PathBuf::from);
            current = Some(DiffFile {
                old_path,
                new_path,
                additions: 0,
                deletions: 0,
                lines: vec![DiffLine {
                    old_line: None,
                    new_line: None,
                    kind: DiffLineKind::Metadata,
                    text: raw.to_owned(),
                }],
            });
            continue;
        }
        let Some(file) = current.as_mut() else {
            continue;
        };
        if raw.starts_with("@@") {
            if let Some((old_start, new_start)) = parse_hunk_header(raw) {
                old_line = old_start;
                new_line = new_start;
            }
            file.lines.push(DiffLine {
                old_line: None,
                new_line: None,
                kind: DiffLineKind::Hunk,
                text: raw.to_owned(),
            });
        } else if raw.starts_with("+++")
            || raw.starts_with("---")
            || raw.starts_with("index ")
            || raw.starts_with("new file")
            || raw.starts_with("deleted file")
        {
            file.lines.push(DiffLine {
                old_line: None,
                new_line: None,
                kind: DiffLineKind::Metadata,
                text: raw.to_owned(),
            });
        } else if let Some(text) = raw.strip_prefix('+') {
            file.additions += 1;
            file.lines.push(DiffLine {
                old_line: None,
                new_line: Some(new_line),
                kind: DiffLineKind::Added,
                text: text.to_owned(),
            });
            new_line += 1;
        } else if let Some(text) = raw.strip_prefix('-') {
            file.deletions += 1;
            file.lines.push(DiffLine {
                old_line: Some(old_line),
                new_line: None,
                kind: DiffLineKind::Removed,
                text: text.to_owned(),
            });
            old_line += 1;
        } else if let Some(text) = raw.strip_prefix(' ') {
            file.lines.push(DiffLine {
                old_line: Some(old_line),
                new_line: Some(new_line),
                kind: DiffLineKind::Context,
                text: text.to_owned(),
            });
            old_line += 1;
            new_line += 1;
        } else if !raw.is_empty() {
            file.lines.push(DiffLine {
                old_line: None,
                new_line: None,
                kind: DiffLineKind::Metadata,
                text: raw.to_owned(),
            });
        }
    }
    if let Some(file) = current {
        files.push(file);
    }
    DiffDocument { files }
}

pub fn display_lines(document: &DiffDocument) -> Vec<String> {
    document
        .files
        .iter()
        .flat_map(|file| {
            let name = file
                .new_path
                .as_ref()
                .or(file.old_path.as_ref())
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "unknown".to_owned());
            let mut lines = vec![format!("{}  +{} -{}", name, file.additions, file.deletions)];
            lines.extend(file.lines.iter().map(|line| match line.kind {
                DiffLineKind::Hunk | DiffLineKind::Metadata => line.text.clone(),
                DiffLineKind::Added => format!("+{}", line.text),
                DiffLineKind::Removed => format!("-{}", line.text),
                DiffLineKind::Context => format!(" {}", line.text),
            }));
            lines.push(String::new());
            lines
        })
        .collect()
}

fn strip_git_prefix(path: &str) -> &str {
    path.strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path)
}

fn parse_hunk_header(header: &str) -> Option<(usize, usize)> {
    let mut parts = header.split_whitespace();
    let _ = parts.next();
    let old = parts
        .next()?
        .strip_prefix('-')?
        .split(',')
        .next()?
        .parse()
        .ok()?;
    let new = parts
        .next()?
        .strip_prefix('+')?
        .split(',')
        .next()?
        .parse()
        .ok()?;
    Some((old, new))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_files_and_line_numbers() {
        let document =
            parse("diff --git a/src/lib.rs b/src/lib.rs\n@@ -1,2 +1,3 @@\n old\n-new\n+new\n+more");
        assert_eq!(document.files.len(), 1);
        assert_eq!(document.files[0].additions, 2);
        assert_eq!(document.files[0].deletions, 1);
        assert_eq!(document.files[0].lines[3].old_line, Some(2));
    }
}
