use std::str;

use xxhash_rust::xxh32::xxh32;

use crate::FileEdit;
use crate::LineAnchor;
use crate::LineRange;
use crate::PlanError;

const LINE_HASH_WIDTH: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EditOutputLimit {
    pub max_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LineEnding {
    CrLf,
    Lf,
    Cr,
    None,
}

impl LineEnding {
    fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::CrLf => b"\r\n",
            Self::Lf => b"\n",
            Self::Cr => b"\r",
            Self::None => b"",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TextLine {
    text: String,
    ending: LineEnding,
    original_line: Option<u64>,
    inserted_after: Option<u64>,
}

struct TextDocument {
    bom: bool,
    original_lines: Vec<String>,
    lines: Vec<TextLine>,
    deleted: Vec<bool>,
    fallback_ending: LineEnding,
}

impl TextDocument {
    fn parse(path: &str, contents: &[u8]) -> Result<Self, PlanError> {
        let contents = validate_utf8(path, contents)?;
        let (bom, contents) = contents
            .strip_prefix('\u{feff}')
            .map_or((false, contents), |contents| (true, contents));
        let mut lines = Vec::new();
        let mut start = 0;
        let bytes = contents.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            let ending = match bytes[index] {
                b'\r' if bytes.get(index + 1) == Some(&b'\n') => Some((LineEnding::CrLf, 2)),
                b'\r' => Some((LineEnding::Cr, 1)),
                b'\n' => Some((LineEnding::Lf, 1)),
                _ => None,
            };
            if let Some((ending, width)) = ending {
                lines.push(TextLine {
                    text: contents[start..index].to_string(),
                    ending,
                    original_line: Some(lines.len() as u64 + 1),
                    inserted_after: None,
                });
                index += width;
                start = index;
            } else {
                index += 1;
            }
        }
        if start < contents.len() {
            lines.push(TextLine {
                text: contents[start..].to_string(),
                ending: LineEnding::None,
                original_line: Some(lines.len() as u64 + 1),
                inserted_after: None,
            });
        }
        let fallback_ending = lines
            .iter()
            .map(|line| line.ending)
            .find(|ending| *ending != LineEnding::None)
            .unwrap_or(LineEnding::Lf);
        let original_lines = lines
            .iter()
            .map(|line| line.text.clone())
            .collect::<Vec<_>>();
        let deleted = vec![false; original_lines.len()];
        Ok(Self {
            bom,
            original_lines,
            lines,
            deleted,
            fallback_ending,
        })
    }

    fn apply(&mut self, path: &str, edit: FileEdit) -> Result<(), PlanError> {
        match edit {
            FileEdit::ReplaceAll { .. } => Err(PlanError::InvalidEdits {
                path: path.to_string(),
            }),
            FileEdit::ReplaceLines { range, lines } => self.replace(path, range, lines),
            FileEdit::InsertBefore { anchor, lines } => {
                let index = self.anchor_index(path, &anchor)?;
                let inserted =
                    self.inserted_lines(path, lines, self.fallback_ending, false, None)?;
                self.lines.splice(index..index, inserted);
                Ok(())
            }
            FileEdit::InsertAfter { anchor, lines } => {
                let anchor_index = self.anchor_index(path, &anchor)?;
                if lines.is_empty() {
                    return Err(invalid_edit_text(path, "inserted lines must not be empty"));
                }
                let mut insertion_index = anchor_index + 1;
                while self
                    .lines
                    .get(insertion_index)
                    .is_some_and(|line| line.inserted_after == Some(anchor.line))
                {
                    insertion_index += 1;
                }
                let predecessor_index = insertion_index - 1;
                let final_ending = if self.lines[predecessor_index].ending == LineEnding::None {
                    self.lines[predecessor_index].ending = self.fallback_ending;
                    LineEnding::None
                } else {
                    self.fallback_ending
                };
                let inserted =
                    self.inserted_lines(path, lines, final_ending, true, Some(anchor.line))?;
                self.lines
                    .splice(insertion_index..insertion_index, inserted);
                Ok(())
            }
        }
    }

    fn replace(
        &mut self,
        path: &str,
        range: LineRange,
        lines: Vec<String>,
    ) -> Result<(), PlanError> {
        if range.start.line > range.end.line {
            return Err(invalid_anchor(
                path,
                format!(
                    "range starts at line {} after line {}",
                    range.start.line, range.end.line
                ),
            ));
        }
        let start = self.anchor_index(path, &range.start)?;
        let end = self.anchor_index(path, &range.end)?;
        for line in range.start.line..=range.end.line {
            self.require_original_line(path, line)?;
        }
        if start > end {
            return Err(invalid_anchor(
                path,
                "range order changed after earlier edits",
            ));
        }
        let final_ending = self.lines[end].ending;
        let replacement = self.inserted_lines(path, lines, final_ending, true, None)?;
        self.lines.splice(start..=end, replacement);
        for line in range.start.line..=range.end.line {
            self.deleted[line as usize - 1] = true;
        }
        Ok(())
    }

    fn inserted_lines(
        &self,
        path: &str,
        lines: Vec<String>,
        final_ending: LineEnding,
        permit_empty: bool,
        inserted_after: Option<u64>,
    ) -> Result<Vec<TextLine>, PlanError> {
        if lines.is_empty() && !permit_empty {
            return Err(invalid_edit_text(path, "inserted lines must not be empty"));
        }
        let line_count = lines.len();
        lines
            .into_iter()
            .enumerate()
            .map(|(index, text)| {
                if text.contains(['\r', '\n']) {
                    return Err(invalid_edit_text(
                        path,
                        "line values must not contain line endings",
                    ));
                }
                Ok(TextLine {
                    text,
                    ending: if index + 1 == line_count {
                        final_ending
                    } else {
                        self.fallback_ending
                    },
                    original_line: None,
                    inserted_after,
                })
            })
            .collect()
    }

    fn anchor_index(&self, path: &str, anchor: &LineAnchor) -> Result<usize, PlanError> {
        let original_index = self.require_original_line(path, anchor.line)?;
        if anchor.expected_hash.len() != LINE_HASH_WIDTH
            || !anchor
                .expected_hash
                .chars()
                .all(|ch| ch.is_ascii_hexdigit())
        {
            return Err(invalid_anchor(
                path,
                format!(
                    "line {} hash must contain exactly {LINE_HASH_WIDTH} hexadecimal characters",
                    anchor.line
                ),
            ));
        }
        let actual = line_hash(&self.original_lines[original_index]);
        if !actual.eq_ignore_ascii_case(&anchor.expected_hash) {
            return Err(invalid_anchor(
                path,
                format!(
                    "line {} hash mismatch: expected {}, found {actual}",
                    anchor.line, anchor.expected_hash
                ),
            ));
        }
        self.lines
            .iter()
            .position(|line| line.original_line == Some(anchor.line))
            .ok_or_else(|| {
                invalid_anchor(path, format!("line {} was already replaced", anchor.line))
            })
    }

    fn require_original_line(&self, path: &str, line: u64) -> Result<usize, PlanError> {
        let index = usize::try_from(line)
            .ok()
            .and_then(|line| line.checked_sub(1))
            .filter(|index| *index < self.original_lines.len())
            .ok_or_else(|| {
                invalid_anchor(
                    path,
                    format!(
                        "line {line} is outside file range 1..={}",
                        self.original_lines.len()
                    ),
                )
            })?;
        if self.deleted[index] {
            return Err(invalid_anchor(
                path,
                format!("line {line} was already replaced"),
            ));
        }
        Ok(index)
    }

    fn finish(self, limit: EditOutputLimit) -> Result<Vec<u8>, PlanError> {
        let encoded_len = self
            .lines
            .iter()
            .fold(u64::from(self.bom) * 3, |len, line| {
                len.saturating_add(line.text.len() as u64)
                    .saturating_add(line.ending.as_bytes().len() as u64)
            });
        check_output_limit(encoded_len, limit)?;
        let mut output = Vec::with_capacity(encoded_len as usize);
        if self.bom {
            output.extend_from_slice("\u{feff}".as_bytes());
        }
        for line in self.lines {
            output.extend_from_slice(line.text.as_bytes());
            output.extend_from_slice(line.ending.as_bytes());
        }
        Ok(output)
    }
}

pub(crate) fn compile_edits(
    path: &str,
    before: &[u8],
    edits: Vec<FileEdit>,
    allow_empty: bool,
    limit: EditOutputLimit,
) -> Result<Vec<u8>, PlanError> {
    validate_utf8(path, before)?;
    if edits.is_empty() && allow_empty {
        check_output_limit(before.len() as u64, limit)?;
        return Ok(before.to_vec());
    }
    if let [FileEdit::ReplaceAll { contents }] = edits.as_slice() {
        validate_utf8(path, contents)?;
        check_output_limit(contents.len() as u64, limit)?;
        return Ok(contents.clone());
    }
    if edits.is_empty()
        || edits
            .iter()
            .any(|edit| matches!(edit, FileEdit::ReplaceAll { .. }))
    {
        return Err(PlanError::InvalidEdits {
            path: path.to_string(),
        });
    }
    let mut document = TextDocument::parse(path, before)?;
    for edit in edits {
        document.apply(path, edit)?;
    }
    document.finish(limit)
}

pub(crate) fn validate_utf8<'a>(path: &str, contents: &'a [u8]) -> Result<&'a str, PlanError> {
    str::from_utf8(contents).map_err(|_| PlanError::InvalidUtf8 {
        path: path.to_string(),
    })
}

pub(crate) fn line_hash(input: &str) -> String {
    let hash = xxh32(input.as_bytes(), 0) & 0xffff;
    format!("{hash:04x}")
}

fn invalid_anchor(path: &str, reason: impl Into<String>) -> PlanError {
    PlanError::InvalidAnchor {
        path: path.to_string(),
        reason: reason.into(),
    }
}

fn invalid_edit_text(path: &str, reason: impl Into<String>) -> PlanError {
    PlanError::InvalidEditText {
        path: path.to_string(),
        reason: reason.into(),
    }
}

fn check_output_limit(observed: u64, limit: EditOutputLimit) -> Result<(), PlanError> {
    if observed > limit.max_bytes {
        Err(PlanError::Limit {
            resource: "file bytes",
            observed,
            limit: limit.max_bytes,
        })
    } else {
        Ok(())
    }
}
