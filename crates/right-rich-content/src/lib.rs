#![warn(unreachable_pub)]

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use url::Url;

/// Maximum number of UTF-16 code units Telegram accepts in one rich message.
///
/// Telegram measures message length in UTF-16 code units, not Unicode scalars:
/// an astral-plane character (most emoji) counts as 2. Every budget in this
/// crate is therefore computed in UTF-16 units so a "fits" chunk cannot be
/// rejected as too long.
pub const MAX_RICH_MESSAGE_UTF16: usize = 32_768;
/// Maximum number of UTF-16 code units Telegram accepts in one regular
/// (plain-text) message. See [`MAX_RICH_MESSAGE_UTF16`] for the unit.
pub const MAX_PLAIN_MESSAGE_UTF16: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RichContent(Content);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Content {
    Text(TextContent),
    Blocks(BlocksContent),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextContent {
    text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlocksContent {
    blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block(BlockKind);

#[derive(Debug, Clone, PartialEq, Eq)]
enum BlockKind {
    Paragraph {
        runs: Vec<Run>,
    },
    Heading {
        level: u8,
        runs: Vec<Run>,
    },
    List {
        ordered: bool,
        items: Vec<ListItem>,
    },
    Quote {
        runs: Vec<Run>,
    },
    Code {
        text: String,
        language: Option<String>,
    },
    Table {
        rows: Vec<Vec<TableCell>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    runs: Vec<Run>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableCell {
    runs: Vec<Run>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    text: String,
    marks: Option<Vec<Mark>>,
    link: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mark {
    Bold,
    Italic,
    Strikethrough,
    Code,
}

#[derive(Debug, Clone, Copy)]
pub enum RichContentRef<'a> {
    Text(&'a str),
    Blocks(&'a [Block]),
}

#[derive(Debug, Clone, Copy)]
pub enum BlockRef<'a> {
    Paragraph {
        runs: &'a [Run],
    },
    Heading {
        level: u8,
        runs: &'a [Run],
    },
    List {
        ordered: bool,
        items: &'a [ListItem],
    },
    Quote {
        runs: &'a [Run],
    },
    Code {
        text: &'a str,
        language: Option<&'a str>,
    },
    Table {
        rows: &'a [Vec<TableCell>],
    },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("rich content must contain visible text")]
    EmptyContent,
    #[error("rich content source text must not exceed {MAX_RICH_MESSAGE_UTF16} UTF-16 code units")]
    SourceTextTooLong,
    #[error("heading level must be between 1 and 3")]
    HeadingLevel,
    #[error("list must contain at least one item")]
    EmptyList,
    #[error("table must contain at least one row and one column")]
    EmptyTable,
    #[error("table rows must all have the same number of cells")]
    NonRectangularTable,
    #[error("marks on a run must be unique")]
    DuplicateMark,
    #[error("code mark cannot be combined with another mark or link")]
    InvalidCodeMark,
    #[error("link scheme must be http, https, or tg")]
    InvalidLink,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum RawRichContent {
    Text(RawTextContent),
    Blocks(RawBlocksContent),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTextContent {
    text: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBlocksContent {
    blocks: Vec<RawBlock>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum RawBlock {
    Paragraph {
        runs: Vec<RawRun>,
    },
    Heading {
        level: u8,
        runs: Vec<RawRun>,
    },
    List {
        ordered: bool,
        items: Vec<RawListItem>,
    },
    Quote {
        runs: Vec<RawRun>,
    },
    Code {
        text: String,
        language: Option<String>,
    },
    Table {
        rows: Vec<Vec<RawTableCell>>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawListItem {
    runs: Vec<RawRun>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTableCell {
    runs: Vec<RawRun>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRun {
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    marks: Option<Vec<Mark>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    link: Option<String>,
}

impl RichContent {
    pub fn literal(text: impl Into<String>) -> Result<Self, ValidationError> {
        let content = Self(Content::Text(TextContent { text: text.into() }));
        content.validate()?;
        Ok(content)
    }

    pub fn paragraph(text: impl Into<String>) -> Result<Self, ValidationError> {
        let content = Self(Content::Blocks(BlocksContent {
            blocks: vec![Block(BlockKind::Paragraph {
                runs: vec![Run::plain(text)],
            })],
        }));
        content.validate()?;
        Ok(content)
    }

    pub fn as_ref(&self) -> RichContentRef<'_> {
        match &self.0 {
            Content::Text(content) => RichContentRef::Text(&content.text),
            Content::Blocks(content) => RichContentRef::Blocks(&content.blocks),
        }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        match &self.0 {
            Content::Text(content) => validate_source_text(&content.text),
            Content::Blocks(content) => {
                if content.blocks.is_empty() {
                    return Err(ValidationError::EmptyContent);
                }
                for block in &content.blocks {
                    block.validate()?;
                }
                require_visible(&self.normalized_text())
            }
        }
    }

    pub fn normalized_text(&self) -> String {
        match &self.0 {
            Content::Text(content) => normalize(&content.text),
            Content::Blocks(content) => normalized_blocks(&content.blocks),
        }
    }

    /// Platform-owned constructor for arbitrary non-empty text of any length.
    ///
    /// Unlike [`RichContent::literal`], this accepts text beyond
    /// [`MAX_RICH_MESSAGE_UTF16`] and splits it into valid paragraph blocks
    /// without truncation. Whitespace-only input is rejected and split points
    /// never isolate a whitespace-only chunk, so every public constructor
    /// preserves the `RichContent` validity invariant.
    pub fn platform_text(text: impl Into<String>) -> Result<Self, ValidationError> {
        let normalized = normalize(&text.into());
        require_visible(&normalized)?;
        let blocks = split_visible_utf16(&normalized, MAX_RICH_MESSAGE_UTF16)
            .into_iter()
            .map(|chunk| {
                Block(BlockKind::Paragraph {
                    runs: vec![Run::plain(chunk)],
                })
            })
            .collect();
        Ok(Self(Content::Blocks(BlocksContent { blocks })))
    }

    pub fn delivery_parts(&self) -> Vec<Self> {
        match &self.0 {
            Content::Text(content) => {
                split_visible_utf16(&normalize(&content.text), MAX_RICH_MESSAGE_UTF16)
                    .into_iter()
                    .map(|text| Self(Content::Text(TextContent { text })))
                    .collect()
            }
            Content::Blocks(content) => split_blocks(&content.blocks),
        }
    }

    pub fn prepend_platform_paragraph(&mut self, text: impl Into<String>) {
        self.insert_platform_paragraph(text.into(), true);
    }

    pub fn append_platform_paragraph(&mut self, text: impl Into<String>) {
        self.insert_platform_paragraph(text.into(), false);
    }

    fn insert_platform_paragraph(&mut self, text: String, prepend: bool) {
        let blocks: Vec<_> = split_visible_utf16(&normalize(&text), MAX_RICH_MESSAGE_UTF16)
            .into_iter()
            .map(|text| {
                Block(BlockKind::Paragraph {
                    runs: vec![Run::plain(text)],
                })
            })
            .collect();
        match &mut self.0 {
            Content::Text(content) => {
                let agent = Block(BlockKind::Paragraph {
                    runs: vec![Run::plain(std::mem::take(&mut content.text))],
                });
                let mut combined = Vec::with_capacity(blocks.len() + 1);
                if prepend {
                    combined.extend(blocks);
                    combined.push(agent);
                } else {
                    combined.push(agent);
                    combined.extend(blocks);
                }
                self.0 = Content::Blocks(BlocksContent { blocks: combined });
            }
            Content::Blocks(content) if prepend => {
                content.blocks.splice(0..0, blocks).for_each(drop)
            }
            Content::Blocks(content) => content.blocks.extend(blocks),
        }
    }
}

impl Serialize for RichContent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate().map_err(serde::ser::Error::custom)?;
        RawRichContent::from(self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RichContent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let content = Self::from(RawRichContent::deserialize(deserializer)?);
        content.validate().map_err(serde::de::Error::custom)?;
        Ok(content)
    }
}

impl JsonSchema for RichContent {
    fn schema_name() -> Cow<'static, str> {
        "RichContent".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::RichContent").into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let mut value = rich_content_schema().to_value();
        if let Some(definitions) = value
            .as_object_mut()
            .and_then(|schema| schema.remove("$defs"))
            && let Some(definitions) = definitions.as_object()
        {
            generator.definitions_mut().extend(definitions.clone());
        }
        Schema::try_from(value).expect("authoritative rich content schema is an object")
    }
}

/// Authoritative JSON Schema for agent-authored rich content.
pub fn rich_content_schema() -> Schema {
    json_schema!({
        "oneOf": [
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "Non-empty text. maxLength 32768 is an upper bound in JSON-Schema code points; runtime validation enforces the same number of UTF-16 code units, which is stricter for astral-plane characters (emoji count as 2 units).",
                        "minLength": 1,
                        "maxLength": MAX_RICH_MESSAGE_UTF16
                    }
                },
                "required": ["text"]
            },
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "blocks": {
                        "type": "array",
                        "minItems": 1,
                        "items": { "$ref": "#/$defs/block" }
                    }
                },
                "required": ["blocks"]
            }
        ],
        "$defs": {
            "mark": { "enum": ["bold", "italic", "strikethrough", "code"] },
            "run": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "text": { "type": "string", "description": "Non-empty run text. maxLength 32768 is an upper bound in JSON-Schema code points; runtime validation enforces the same number of UTF-16 code units (stricter for astral-plane characters).", "minLength": 1, "maxLength": MAX_RICH_MESSAGE_UTF16 },
                    "marks": {
                        "type": ["array", "null"],
                        "uniqueItems": true,
                        "items": { "$ref": "#/$defs/mark" }
                    },
                    "link": { "type": ["string", "null"], "pattern": "^(https?|tg):" }
                },
                "required": ["text"],
                "allOf": [{
                    "if": {
                        "properties": { "marks": { "contains": { "const": "code" } } },
                        "required": ["marks"]
                    },
                    "then": {
                        "properties": { "marks": { "maxItems": 1 }, "link": { "type": "null" } }
                    }
                }]
            },
            "runs": { "type": "array", "minItems": 1, "items": { "$ref": "#/$defs/run" } },
            "cell": {
                "type": "object",
                "additionalProperties": false,
                "properties": { "runs": { "type": "array", "items": { "$ref": "#/$defs/run" } } },
                "required": ["runs"]
            },
            "block": {
                "oneOf": [
                    {
                        "type": "object", "additionalProperties": false,
                        "properties": { "type": { "const": "paragraph" }, "runs": { "$ref": "#/$defs/runs" } },
                        "required": ["type", "runs"]
                    },
                    {
                        "type": "object", "additionalProperties": false,
                        "properties": {
                            "type": { "const": "heading" },
                            "level": { "type": "integer", "minimum": 1, "maximum": 3 },
                            "runs": { "$ref": "#/$defs/runs" }
                        },
                        "required": ["type", "level", "runs"]
                    },
                    {
                        "type": "object", "additionalProperties": false,
                        "properties": {
                            "type": { "const": "list" }, "ordered": { "type": "boolean" },
                            "items": {
                                "type": "array", "minItems": 1,
                                "items": {
                                    "type": "object", "additionalProperties": false,
                                    "properties": { "runs": { "$ref": "#/$defs/runs" } },
                                    "required": ["runs"]
                                }
                            }
                        },
                        "required": ["type", "ordered", "items"]
                    },
                    {
                        "type": "object", "additionalProperties": false,
                        "properties": { "type": { "const": "quote" }, "runs": { "$ref": "#/$defs/runs" } },
                        "required": ["type", "runs"]
                    },
                    {
                        "type": "object", "additionalProperties": false,
                        "properties": {
                            "type": { "const": "code" },
                            "text": { "type": "string", "minLength": 1, "maxLength": MAX_RICH_MESSAGE_UTF16 },
                            "language": { "type": ["string", "null"] }
                        },
                        "required": ["type", "text"]
                    },
                    {
                        "type": "object", "additionalProperties": false,
                        "properties": {
                            "type": { "const": "table" },
                            "rows": {
                                "type": "array", "minItems": 1,
                                "items": { "type": "array", "minItems": 1, "items": { "$ref": "#/$defs/cell" } }
                            }
                        },
                        "required": ["type", "rows"]
                    }
                ]
            }
        }
    })
}

impl TryFrom<String> for RichContent {
    type Error = ValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::literal(value)
    }
}

impl TryFrom<&str> for RichContent {
    type Error = ValidationError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::literal(value)
    }
}

impl Block {
    pub fn as_ref(&self) -> BlockRef<'_> {
        match &self.0 {
            BlockKind::Paragraph { runs } => BlockRef::Paragraph { runs },
            BlockKind::Heading { level, runs } => BlockRef::Heading {
                level: *level,
                runs,
            },
            BlockKind::List { ordered, items } => BlockRef::List {
                ordered: *ordered,
                items,
            },
            BlockKind::Quote { runs } => BlockRef::Quote { runs },
            BlockKind::Code { text, language } => BlockRef::Code {
                text,
                language: language.as_deref(),
            },
            BlockKind::Table { rows } => BlockRef::Table { rows },
        }
    }

    fn validate(&self) -> Result<(), ValidationError> {
        match &self.0 {
            BlockKind::Paragraph { runs } | BlockKind::Quote { runs } => validate_runs(runs, false),
            BlockKind::Heading { level, runs } => {
                if !(1..=3).contains(level) {
                    return Err(ValidationError::HeadingLevel);
                }
                validate_runs(runs, false)
            }
            BlockKind::List { items, .. } => {
                if items.is_empty() {
                    return Err(ValidationError::EmptyList);
                }
                for item in items {
                    validate_runs(&item.runs, false)?;
                }
                Ok(())
            }
            BlockKind::Code { text, .. } => validate_source_text(text),
            BlockKind::Table { rows } => {
                let Some(width) = rows.first().map(Vec::len).filter(|width| *width > 0) else {
                    return Err(ValidationError::EmptyTable);
                };
                if rows.iter().any(|row| row.len() != width) {
                    return Err(ValidationError::NonRectangularTable);
                }
                for cell in rows.iter().flatten() {
                    validate_runs(&cell.runs, true)?;
                }
                Ok(())
            }
        }
    }

    fn normalized_text(&self) -> String {
        match &self.0 {
            BlockKind::Paragraph { runs }
            | BlockKind::Heading { runs, .. }
            | BlockKind::Quote { runs } => runs_text(runs),
            BlockKind::List { ordered, items } => items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let prefix = if *ordered {
                        format!("{}. ", index + 1)
                    } else {
                        "- ".to_owned()
                    };
                    format!("{prefix}{}", runs_text(&item.runs))
                })
                .collect::<Vec<_>>()
                .join("\n"),
            BlockKind::Code { text, .. } => normalize(text),
            BlockKind::Table { rows } => rows
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|cell| runs_text(&cell.runs))
                        .collect::<Vec<_>>()
                        .join("\t")
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

impl ListItem {
    pub fn runs(&self) -> &[Run] {
        &self.runs
    }
}

impl TableCell {
    pub fn runs(&self) -> &[Run] {
        &self.runs
    }
}

impl Run {
    fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            marks: None,
            link: None,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn marks(&self) -> Option<&[Mark]> {
        self.marks.as_deref()
    }

    pub fn link(&self) -> Option<&str> {
        self.link.as_deref()
    }

    fn validate(&self) -> Result<(), ValidationError> {
        require_visible(&self.text)?;
        let marks = self.marks.as_deref().unwrap_or_default();
        if marks
            .iter()
            .enumerate()
            .any(|(index, mark)| marks[index + 1..].contains(mark))
        {
            return Err(ValidationError::DuplicateMark);
        }
        if marks.contains(&Mark::Code) && (marks.len() != 1 || self.link.is_some()) {
            return Err(ValidationError::InvalidCodeMark);
        }
        if let Some(link) = &self.link {
            let parsed = Url::parse(link).map_err(|_| ValidationError::InvalidLink)?;
            if !matches!(parsed.scheme(), "http" | "https" | "tg") {
                return Err(ValidationError::InvalidLink);
            }
            if matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_none() {
                return Err(ValidationError::InvalidLink);
            }
        }
        Ok(())
    }
}

impl From<RawRichContent> for RichContent {
    fn from(raw: RawRichContent) -> Self {
        Self(match raw {
            RawRichContent::Text(value) => Content::Text(TextContent { text: value.text }),
            RawRichContent::Blocks(value) => Content::Blocks(BlocksContent {
                blocks: value.blocks.into_iter().map(Block::from).collect(),
            }),
        })
    }
}

impl From<RawBlock> for Block {
    fn from(raw: RawBlock) -> Self {
        Self(match raw {
            RawBlock::Paragraph { runs } => BlockKind::Paragraph {
                runs: map_runs(runs),
            },
            RawBlock::Heading { level, runs } => BlockKind::Heading {
                level,
                runs: map_runs(runs),
            },
            RawBlock::List { ordered, items } => BlockKind::List {
                ordered,
                items: items
                    .into_iter()
                    .map(|item| ListItem {
                        runs: map_runs(item.runs),
                    })
                    .collect(),
            },
            RawBlock::Quote { runs } => BlockKind::Quote {
                runs: map_runs(runs),
            },
            RawBlock::Code { text, language } => BlockKind::Code { text, language },
            RawBlock::Table { rows } => BlockKind::Table {
                rows: rows
                    .into_iter()
                    .map(|row| {
                        row.into_iter()
                            .map(|cell| TableCell {
                                runs: map_runs(cell.runs),
                            })
                            .collect()
                    })
                    .collect(),
            },
        })
    }
}

impl From<&RichContent> for RawRichContent {
    fn from(content: &RichContent) -> Self {
        match &content.0 {
            Content::Text(value) => Self::Text(RawTextContent {
                text: value.text.clone(),
            }),
            Content::Blocks(value) => Self::Blocks(RawBlocksContent {
                blocks: value.blocks.iter().map(RawBlock::from).collect(),
            }),
        }
    }
}

impl From<&Block> for RawBlock {
    fn from(block: &Block) -> Self {
        match &block.0 {
            BlockKind::Paragraph { runs } => Self::Paragraph {
                runs: raw_runs(runs),
            },
            BlockKind::Heading { level, runs } => Self::Heading {
                level: *level,
                runs: raw_runs(runs),
            },
            BlockKind::List { ordered, items } => Self::List {
                ordered: *ordered,
                items: items
                    .iter()
                    .map(|item| RawListItem {
                        runs: raw_runs(&item.runs),
                    })
                    .collect(),
            },
            BlockKind::Quote { runs } => Self::Quote {
                runs: raw_runs(runs),
            },
            BlockKind::Code { text, language } => Self::Code {
                text: text.clone(),
                language: language.clone(),
            },
            BlockKind::Table { rows } => Self::Table {
                rows: rows
                    .iter()
                    .map(|row| {
                        row.iter()
                            .map(|cell| RawTableCell {
                                runs: raw_runs(&cell.runs),
                            })
                            .collect()
                    })
                    .collect(),
            },
        }
    }
}

fn map_runs(runs: Vec<RawRun>) -> Vec<Run> {
    runs.into_iter()
        .map(|run| Run {
            text: run.text,
            marks: run.marks,
            link: run.link,
        })
        .collect()
}

fn raw_runs(runs: &[Run]) -> Vec<RawRun> {
    runs.iter()
        .map(|run| RawRun {
            text: run.text.clone(),
            marks: run.marks.clone(),
            link: run.link.clone(),
        })
        .collect()
}

fn split_blocks(blocks: &[Block]) -> Vec<RichContent> {
    let visible: Vec<_> = blocks
        .iter()
        .filter(|block| !block.normalized_text().is_empty())
        .collect();
    let mut parts = Vec::new();
    let mut batch = Vec::new();
    let mut batch_units = 0usize;

    for block in visible {
        let normalized = block.normalized_text();
        let block_units = utf16_len(&normalized);
        if block_units > MAX_RICH_MESSAGE_UTF16 {
            push_block_batch(&mut parts, &mut batch);
            batch_units = 0;
            parts.extend(
                split_visible_utf16(&normalized, MAX_RICH_MESSAGE_UTF16)
                    .into_iter()
                    .map(|text| RichContent(Content::Text(TextContent { text }))),
            );
            continue;
        }
        let separator = usize::from(!batch.is_empty()) * 2;
        if !batch.is_empty() && batch_units + separator + block_units > MAX_RICH_MESSAGE_UTF16 {
            push_block_batch(&mut parts, &mut batch);
            batch_units = 0;
        }
        if !batch.is_empty() {
            batch_units += 2;
        }
        batch_units += block_units;
        batch.push(block.clone());
    }
    push_block_batch(&mut parts, &mut batch);
    parts
}

fn push_block_batch(parts: &mut Vec<RichContent>, batch: &mut Vec<Block>) {
    if !batch.is_empty() {
        parts.push(RichContent(Content::Blocks(BlocksContent {
            blocks: std::mem::take(batch),
        })));
    }
}

/// Split `text` into chunks of at most `max_units` UTF-16 code units, cutting
/// only at Unicode scalar boundaries (never inside an astral surrogate pair).
///
/// A chunk therefore also has at most `max_units` Unicode scalars, which is
/// what callers validating in scalars can rely on; the converse does not hold
/// for astral text, which is exactly why the budget is in UTF-16 units.
///
/// No chunk is whitespace-only: a cut that would strand a whitespace run
/// carries the run's trailing separator into the next chunk, and a run longer
/// than the whole budget collapses to that separator instead of ever forming
/// an invalid chunk. Whitespace between visible characters is therefore never
/// lost entirely, and non-whitespace characters are never dropped or
/// reordered. `max_units` must cover one scalar plus one separator (4 units);
/// every call site uses a Telegram budget of 4,096 or 32,768.
pub fn split_visible_utf16(text: &str, max_units: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut units = 0usize;
    let mut visible = false;
    for character in text.chars() {
        let character_units = character.len_utf16();
        if units + character_units > max_units {
            if visible {
                chunks.push(std::mem::take(&mut current));
                units = 0;
                visible = false;
            } else {
                // `current` holds only whitespace and cannot grow. Keep just
                // its final separator so the boundary between the surrounding
                // visible runs survives into the next chunk.
                if let Some(separator) = current.chars().next_back() {
                    current.drain(..current.len() - separator.len_utf8());
                    units = separator.len_utf16();
                }
            }
        }
        current.push(character);
        units += character_units;
        visible |= !character.is_whitespace();
    }
    // A trailing whitespace-only remainder is dropped rather than emitted as
    // an invalid chunk; every caller passes text whose visible content is
    // already known to be non-empty.
    if visible {
        chunks.push(current);
    }
    chunks
}

fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

fn validate_runs(runs: &[Run], allow_empty: bool) -> Result<(), ValidationError> {
    if !allow_empty && runs.is_empty() {
        return Err(ValidationError::EmptyContent);
    }
    for run in runs {
        run.validate()?;
    }
    if !allow_empty {
        require_visible(&runs_text(runs))?;
    }
    Ok(())
}

fn runs_text(runs: &[Run]) -> String {
    normalize(&runs.iter().map(|run| run.text.as_str()).collect::<String>())
}

fn normalized_blocks(blocks: &[Block]) -> String {
    blocks
        .iter()
        .map(Block::normalized_text)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn normalize(text: &str) -> String {
    text.lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

fn validate_source_text(text: &str) -> Result<(), ValidationError> {
    require_visible(text)?;
    if utf16_len(text) > MAX_RICH_MESSAGE_UTF16 {
        return Err(ValidationError::SourceTextTooLong);
    }
    Ok(())
}

fn require_visible(text: &str) -> Result<(), ValidationError> {
    if text.trim().is_empty() {
        Err(ValidationError::EmptyContent)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_content_unknown_fields_and_source_overflow() {
        assert!(RichContent::literal("  ").is_err());
        assert!(RichContent::literal("x".repeat(MAX_RICH_MESSAGE_UTF16 + 1)).is_err());
        // Astral text: 16,385 emoji = 32,770 UTF-16 units, far over the limit
        // while only half the scalar bound.
        assert!(RichContent::literal("🦀".repeat(MAX_RICH_MESSAGE_UTF16 / 2 + 1)).is_err());
        assert!(serde_json::from_str::<RichContent>(r#"{"text":"ok","extra":1}"#).is_err());
        assert!(
            serde_json::from_str::<RichContent>(
                r#"{"blocks":[{"type":"heading","level":4,"runs":[{"text":"x"}]}]}"#
            )
            .is_err()
        );
    }

    #[test]
    fn validates_marks_links_and_tables() {
        for json in [
            r#"{"blocks":[{"type":"paragraph","runs":[{"text":"x","marks":["bold","bold"]}]}]}"#,
            r#"{"blocks":[{"type":"paragraph","runs":[{"text":"x","marks":["code","bold"]}]}]}"#,
            r#"{"blocks":[{"type":"paragraph","runs":[{"text":"x","link":"javascript:alert(1)"}]}]}"#,
            r#"{"blocks":[{"type":"paragraph","runs":[{"text":"x","link":"https://"}]}]}"#,
            r#"{"blocks":[{"type":"table","rows":[[{"runs":[]}],[{"runs":[]},{"runs":[]}]]}]}"#,
        ] {
            assert!(serde_json::from_str::<RichContent>(json).is_err(), "{json}");
        }
    }

    #[test]
    fn serialization_revalidates_the_public_value() {
        let mut content = RichContent::literal("valid").unwrap();
        if let Content::Text(text) = &mut content.0 {
            text.text.clear();
        }
        assert!(serde_json::to_value(content).is_err());
    }

    #[test]
    fn normalizes_and_appends_platform_owned_blocks() {
        let mut content: RichContent = serde_json::from_str(r#"{"blocks":[{"type":"heading","level":2,"runs":[{"text":"Title"}]},{"type":"list","ordered":true,"items":[{"runs":[{"text":"One"}]},{"runs":[{"text":"Two"}]}]}]}"#).unwrap();
        assert_eq!(content.normalized_text(), "Title\n\n1. One\n2. Two");
        content.append_platform_paragraph("Receipt");
        assert_eq!(
            content.normalized_text(),
            "Title\n\n1. One\n2. Two\n\nReceipt"
        );
        content.validate().unwrap();
    }

    #[test]
    fn batches_blocks_and_splits_oversized_multibyte_block_without_loss() {
        // Astral emoji = 2 UTF-16 units each: two runs of 8,192 emoji total
        // exactly 32,768 units, so the block itself fits one part and only the
        // trailing paragraph forces a second.
        let half = "🦀".repeat(MAX_RICH_MESSAGE_UTF16 / 4);
        let json = serde_json::json!({
            "blocks": [
                {"type":"paragraph", "runs":[{"text": half}, {"text": half}]},
                {"type":"paragraph", "runs":[{"text":"tail"}]}
            ]
        });
        let content: RichContent = serde_json::from_value(json).unwrap();
        let parts = content.delivery_parts();
        assert_eq!(parts.len(), 2);
        assert!(
            parts
                .iter()
                .all(|part| utf16_len(&part.normalized_text()) <= MAX_RICH_MESSAGE_UTF16)
        );
        assert_eq!(
            parts
                .iter()
                .map(RichContent::normalized_text)
                .collect::<Vec<_>>()
                .join("\n\n"),
            content.normalized_text()
        );
    }

    #[test]
    fn oversized_astral_block_degrades_to_utf16_bounded_literal_parts() {
        // A single source text is capped at validation, but a block's
        // normalized text sums its runs: three 16,384-unit emoji runs make a
        // 49,152-unit paragraph block — over the rich limit as a block — so it
        // degrades to literal parts, each within the UTF-16 budget and cut
        // only at scalar boundaries.
        let run = "🦀".repeat(MAX_RICH_MESSAGE_UTF16 / 4);
        let content: RichContent = serde_json::from_value(serde_json::json!({
            "blocks": [
                {"type":"paragraph", "runs":[{"text": run}, {"text": run}, {"text": run}]}
            ]
        }))
        .unwrap();
        let parts = content.delivery_parts();
        assert_eq!(parts.len(), 2);
        for part in &parts {
            let normalized = part.normalized_text();
            assert!(utf16_len(&normalized) <= MAX_RICH_MESSAGE_UTF16);
            assert!(!normalized.is_empty());
            assert!(normalized.chars().all(|character| character == '🦀'));
        }
        // Nothing is lost: the parts reassemble the block's text exactly.
        assert_eq!(
            parts
                .iter()
                .map(RichContent::normalized_text)
                .collect::<String>(),
            run.repeat(3)
        );
    }

    #[test]
    fn plain_splitter_is_utf8_safe_at_boundary() {
        let text = "界".repeat(MAX_RICH_MESSAGE_UTF16 + 1);
        let content = RichContent(Content::Text(TextContent { text: text.clone() }));
        let parts = content.delivery_parts();
        assert_eq!(parts.len(), 2);
        assert_eq!(
            utf16_len(&parts[0].normalized_text()),
            MAX_RICH_MESSAGE_UTF16
        );
        assert_eq!(parts[1].normalized_text(), "界");
        assert_eq!(
            parts
                .iter()
                .map(RichContent::normalized_text)
                .collect::<String>(),
            text
        );
    }

    #[test]
    fn platform_text_never_truncates_and_rejects_empty() {
        // Far beyond the rich limit, astral-heavy, and newline-padded: the
        // constructor must still produce valid, persistable content. Split
        // points become paragraph breaks ("\n\n"), so exact-text equality
        // cannot hold across chunks; no-truncation is asserted on the
        // non-whitespace content, which the split must preserve verbatim.
        let detail = format!("🦀 {}\n🦀", "boom".repeat(50_000));

        let content = RichContent::platform_text(detail.clone()).unwrap();
        content.validate().unwrap();
        let non_whitespace = |text: &str| {
            text.chars()
                .filter(|character| !character.is_whitespace())
                .collect::<String>()
        };
        assert_eq!(
            non_whitespace(&content.normalized_text()),
            non_whitespace(&detail)
        );
        // Persistence serialization must not panic or reject the value.
        let serialized = serde_json::to_value(&content).unwrap();
        let round: RichContent = serde_json::from_value(serialized).unwrap();
        assert_eq!(round.normalized_text(), content.normalized_text());
        // Delivery parts each respect the UTF-16 budget.
        for part in content.delivery_parts() {
            assert!(utf16_len(&part.normalized_text()) <= MAX_RICH_MESSAGE_UTF16);
        }
        assert_eq!(
            RichContent::platform_text("   ").unwrap_err(),
            ValidationError::EmptyContent
        );
    }

    #[test]
    fn schema_carries_runtime_constraints() {
        let schema = serde_json::to_value(rich_content_schema()).unwrap();
        let encoded = schema.to_string();
        for constraint in [
            "maxLength",
            "minItems",
            "uniqueItems",
            "maximum",
            "pattern",
            "contains",
        ] {
            assert!(
                encoded.contains(constraint),
                "missing {constraint}: {encoded}"
            );
        }
        assert!(encoded.contains(&MAX_RICH_MESSAGE_UTF16.to_string()));
    }

    #[test]
    fn split_visible_utf16_never_yields_a_whitespace_only_chunk() {
        // A whitespace run longer than the whole budget, plus visible text on
        // both sides: no cut may isolate whitespace, yet the boundary between
        // the two visible runs must survive.
        let text = format!("head{}tail", " ".repeat(MAX_RICH_MESSAGE_UTF16 + 5));
        let chunks = split_visible_utf16(&text, MAX_RICH_MESSAGE_UTF16);
        assert!(chunks.len() >= 2, "must split: {}", chunks.len());
        for chunk in &chunks {
            assert!(
                chunk.chars().any(|c| !c.is_whitespace()),
                "whitespace-only chunk: {chunk:?}"
            );
            assert!(utf16_len(chunk) <= MAX_RICH_MESSAGE_UTF16);
            assert!(utf16_len(chunk) > 0);
        }
        let non_whitespace = |text: &str| {
            text.chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
        };
        assert_eq!(
            non_whitespace(&chunks.concat()),
            "headtail",
            "no visible character may be lost"
        );
        assert!(chunks[0].starts_with("head"));
        assert!(chunks.last().unwrap().ends_with("tail"));
    }

    #[test]
    fn platform_text_survives_a_whitespace_run_at_the_split_boundary() {
        // The only >32,768-unit region is a mid-line whitespace run, so
        // `normalize` keeps it and a naive unit-counting cut lands inside it,
        // isolating whitespace. No block may end up whitespace-only.
        let pad = MAX_RICH_MESSAGE_UTF16 / 2;
        let text = format!(
            "a{}{}b",
            "x".repeat(pad),
            " ".repeat(MAX_RICH_MESSAGE_UTF16)
        );
        let content = RichContent::platform_text(text).unwrap();
        // Serialize re-validates every block, so this is the exact path that
        // used to fail `notify_delivery_json`.
        content.validate().unwrap();
        let serialized = serde_json::to_value(&content).unwrap();
        let round: RichContent = serde_json::from_value(serialized).unwrap();
        for content in [&content, &round] {
            for block in match content.as_ref() {
                RichContentRef::Text(_) => panic!("platform text must be blocks"),
                RichContentRef::Blocks(blocks) => blocks,
            } {
                let text = block.normalized_text();
                assert!(!text.is_empty(), "empty paragraph block");
                assert!(
                    text.chars().any(|c| !c.is_whitespace()),
                    "whitespace-only block: {text:?}"
                );
            }
        }
    }

    #[test]
    fn oversized_block_degradation_never_emits_a_whitespace_only_part() {
        // Same hole in `split_blocks`: a block whose normalized text is
        // oversized AND contains a whitespace run spanning the split boundary
        // must degrade to parts that are all individually deliverable. The
        // block ends visible so `normalize` keeps the interior run intact.
        let head = "y".repeat(MAX_RICH_MESSAGE_UTF16 - 1);
        let run = format!("{}   {}z", head, " ".repeat(MAX_RICH_MESSAGE_UTF16));
        let content: RichContent = serde_json::from_value(serde_json::json!({
            "blocks": [ {"type":"paragraph", "runs":[{"text": run}]} ]
        }))
        .unwrap();
        let parts = content.delivery_parts();
        assert!(parts.len() >= 2);
        for part in &parts {
            let normalized = part.normalized_text();
            assert!(utf16_len(&normalized) <= MAX_RICH_MESSAGE_UTF16);
            assert!(
                normalized.chars().any(|c| !c.is_whitespace()),
                "whitespace-only part: {normalized:?}"
            );
            part.validate().unwrap();
        }
    }
}
