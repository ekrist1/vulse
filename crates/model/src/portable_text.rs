use std::collections::{BTreeSet, HashSet};

use serde_json::{Map, Value, json};

use crate::identity::is_uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mark {
    Code,
    Em,
    Strong,
    Underline,
    Strike,
}

impl Mark {
    fn as_str(self) -> &'static str {
        match self {
            Mark::Code => "code",
            Mark::Em => "em",
            Mark::Strong => "strong",
            Mark::Underline => "underline",
            Mark::Strike => "strike",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "code" => Some(Mark::Code),
            "em" => Some(Mark::Em),
            "strong" => Some(Mark::Strong),
            "underline" => Some(Mark::Underline),
            "strike" => Some(Mark::Strike),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockStyle {
    Normal,
    H1,
    H2,
    H3,
    Blockquote,
}

impl BlockStyle {
    fn as_str(self) -> &'static str {
        match self {
            BlockStyle::Normal => "normal",
            BlockStyle::H1 => "h1",
            BlockStyle::H2 => "h2",
            BlockStyle::H3 => "h3",
            BlockStyle::Blockquote => "blockquote",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "normal" => Some(BlockStyle::Normal),
            "h1" => Some(BlockStyle::H1),
            "h2" => Some(BlockStyle::H2),
            "h3" => Some(BlockStyle::H3),
            "blockquote" => Some(BlockStyle::Blockquote),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListItem {
    Bullet,
    Number,
}

impl ListItem {
    fn as_str(self) -> &'static str {
        match self {
            ListItem::Bullet => "bullet",
            ListItem::Number => "number",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "bullet" => Some(ListItem::Bullet),
            "number" => Some(ListItem::Number),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkRange {
    pub start: usize,
    pub end: usize,
    pub mark: Mark,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EditorBlock {
    Text {
        key: String,
        style: BlockStyle,
        list_item: Option<ListItem>,
        level: u8,
        text: String,
        marks: Vec<MarkRange>,
    },
    Image {
        key: String,
        asset_id: String,
    },
    /// Structured blocks are retained losslessly by the legacy editor adapter.
    Custom(Value),
}

pub fn validate_portable_text(value: &Value) -> Result<Vec<String>, String> {
    let items = value.as_array().ok_or("portable text must be an array")?;
    let mut keys = HashSet::new();
    let mut assets = Vec::new();
    validate_items(items, 0, &mut keys, &mut assets)?;
    Ok(assets)
}

fn validate_items(
    items: &[Value],
    depth: usize,
    keys: &mut HashSet<String>,
    assets: &mut Vec<String>,
) -> Result<(), String> {
    if depth > 6 {
        return Err("custom blocks may be nested at most six levels".into());
    }
    for (index, item) in items.iter().enumerate() {
        let obj = item
            .as_object()
            .ok_or_else(|| format!("block {index} must be an object"))?;
        let key = require_str(obj, "_key").map_err(|err| format!("block {index}: {err}"))?;
        if !is_block_key(key) {
            return Err(format!("block {index} has an invalid _key"));
        }
        if !keys.insert(key.to_string()) {
            return Err(format!("duplicate block _key '{key}'"));
        }
        match require_str(obj, "_type")? {
            "block" => validate_text_block(obj, index)?,
            "image" => assets.push(validate_image_block(obj, index)?),
            "iframe" => {
                reject_unknown(obj, &["_key", "_type", "url", "title", "height"], "iframe")?;
                let source = require_str(obj, "url")?;
                validate_embed_url(source)?;
                validate_label(obj, "title", true)?;
                if !obj
                    .get("height")
                    .and_then(Value::as_u64)
                    .is_some_and(|height| (160..=1200).contains(&height))
                {
                    return Err("iframe height must be between 160 and 1200 pixels".into());
                }
            }
            "accordion" => {
                reject_unknown(
                    obj,
                    &["_key", "_type", "title", "collapsed", "content"],
                    "accordion",
                )?;
                validate_label(obj, "title", true)?;
                if obj.get("collapsed").and_then(Value::as_bool).is_none() {
                    return Err("accordion collapsed must be a boolean".into());
                }
                validate_content(obj, depth, keys, assets)?;
            }
            "repeatable" => {
                reject_unknown(
                    obj,
                    &["_key", "_type", "title", "items"],
                    "repeatable group",
                )?;
                validate_label(obj, "title", true)?;
                let entries = obj
                    .get("items")
                    .and_then(Value::as_array)
                    .ok_or("repeatable items must be an array")?;
                if entries.is_empty() || entries.len() > 100 {
                    return Err("repeatable groups must contain between 1 and 100 entries".into());
                }
                for entry in entries {
                    let entry = entry
                        .as_object()
                        .ok_or("repeatable entry must be an object")?;
                    reject_unknown(entry, &["_key", "label", "content"], "repeatable entry")?;
                    let key = require_str(entry, "_key")?;
                    if !is_block_key(key) || !keys.insert(key.to_string()) {
                        return Err("repeatable entry has an invalid or duplicate _key".into());
                    }
                    validate_label(entry, "label", false)?;
                    validate_content(entry, depth, keys, assets)?;
                }
            }
            other => return Err(format!("block {index} has unsupported _type '{other}'")),
        }
    }
    Ok(())
}

fn validate_content(
    obj: &Map<String, Value>,
    depth: usize,
    keys: &mut HashSet<String>,
    assets: &mut Vec<String>,
) -> Result<(), String> {
    let content = obj
        .get("content")
        .and_then(Value::as_array)
        .ok_or("custom block content must be an array")?;
    validate_items(content, depth + 1, keys, assets)
}

fn validate_label(obj: &Map<String, Value>, field: &str, required: bool) -> Result<(), String> {
    let label = require_str(obj, field)?;
    if label.chars().count() > 200 || (required && label.trim().is_empty()) {
        return Err(format!(
            "{field} must {}contain at most 200 characters",
            if required { "be nonempty and " } else { "" }
        ));
    }
    Ok(())
}

pub fn validate_embed_url(source: &str) -> Result<(), String> {
    if source.len() > 2048
        || source
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
    {
        return Err("embed URL must be an absolute HTTP or HTTPS URL without credentials".into());
    }
    let url = url::Url::parse(source)
        .map_err(|_| "embed URL must be an absolute HTTP or HTTPS URL without credentials")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || !source
            .to_ascii_lowercase()
            .starts_with(&format!("{}://", url.scheme()))
    {
        return Err("embed URL must be an absolute HTTP or HTTPS URL without credentials".into());
    }
    Ok(())
}

pub fn editor_blocks_to_json(blocks: &[EditorBlock]) -> Value {
    Value::Array(blocks.iter().map(block_to_json).collect())
}

pub fn editor_blocks_from_json(value: &Value) -> Result<Vec<EditorBlock>, String> {
    validate_portable_text(value)?;
    let mut blocks = Vec::new();
    for item in value.as_array().expect("validated array") {
        let obj = item.as_object().expect("validated object");
        let key = require_str(obj, "_key")?.to_string();
        match require_str(obj, "_type")? {
            "image" => {
                let asset = obj.get("asset").and_then(Value::as_object).expect("asset");
                blocks.push(EditorBlock::Image {
                    key,
                    asset_id: require_str(asset, "_ref")?.to_string(),
                });
            }
            "block" => {
                let style = BlockStyle::parse(require_str(obj, "style")?).expect("style");
                let list_item = obj
                    .get("listItem")
                    .and_then(Value::as_str)
                    .map(|value| ListItem::parse(value).expect("list item"));
                let mut text = String::new();
                let mut marks = Vec::new();
                let mut offset = 0usize;
                for child in obj
                    .get("children")
                    .and_then(Value::as_array)
                    .expect("children")
                {
                    let span = child.as_object().expect("span");
                    let piece = require_str(span, "text")?;
                    let len = piece.chars().count();
                    if let Some(span_marks) = span.get("marks").and_then(Value::as_array) {
                        for mark in span_marks {
                            let mark = Mark::parse(mark.as_str().expect("mark")).expect("mark");
                            marks.push(MarkRange {
                                start: offset,
                                end: offset + len,
                                mark,
                            });
                        }
                    }
                    text.push_str(piece);
                    offset += len;
                }
                normalize_marks(&mut marks);
                blocks.push(EditorBlock::Text {
                    key,
                    style,
                    list_item,
                    level: obj.get("level").and_then(Value::as_u64).unwrap_or(1) as u8,
                    text,
                    marks,
                });
            }
            _ => blocks.push(EditorBlock::Custom(item.clone())),
        }
    }
    Ok(blocks)
}

pub fn toggle_mark(marks: &mut Vec<MarkRange>, mark: Mark, start: usize, end: usize) {
    if start >= end {
        return;
    }
    if range_covered(marks, mark, start, end) {
        let mut next = Vec::new();
        for existing in marks.drain(..) {
            if existing.mark != mark || existing.end <= start || existing.start >= end {
                next.push(existing);
                continue;
            }
            if existing.start < start {
                next.push(MarkRange {
                    start: existing.start,
                    end: start,
                    mark,
                });
            }
            if existing.end > end {
                next.push(MarkRange {
                    start: end,
                    end: existing.end,
                    mark,
                });
            }
        }
        *marks = next;
    } else {
        marks.push(MarkRange { start, end, mark });
    }
    normalize_marks(marks);
}

pub fn normalize_marks(marks: &mut Vec<MarkRange>) {
    marks.retain(|mark| mark.start < mark.end);
    marks.sort_by(|left, right| {
        (left.mark, left.start, left.end).cmp(&(right.mark, right.start, right.end))
    });
    let mut merged: Vec<MarkRange> = Vec::new();
    for mark in marks.drain(..) {
        if let Some(last) = merged.last_mut()
            && last.mark == mark.mark && mark.start <= last.end {
                last.end = last.end.max(mark.end);
                continue;
            }
        merged.push(mark);
    }
    *marks = merged;
}

pub fn clamp_marks(marks: &mut Vec<MarkRange>, char_len: usize) {
    for mark in marks.iter_mut() {
        mark.start = mark.start.min(char_len);
        mark.end = mark.end.min(char_len);
    }
    normalize_marks(marks);
}

pub fn utf16_to_char_index(text: &str, utf16_index: usize) -> usize {
    let mut seen = 0usize;
    for (index, ch) in text.chars().enumerate() {
        if seen >= utf16_index {
            return index;
        }
        seen += ch.len_utf16();
    }
    text.chars().count()
}

fn block_to_json(block: &EditorBlock) -> Value {
    match block {
        EditorBlock::Custom(value) => value.clone(),
        EditorBlock::Image { key, asset_id } => json!({
            "_key": key,
            "_type": "image",
            "asset": { "_type": "reference", "_ref": asset_id }
        }),
        EditorBlock::Text {
            key,
            style,
            list_item,
            level,
            text,
            marks,
        } => {
            let mut stored = marks.clone();
            clamp_marks(&mut stored, text.chars().count());
            let mut value = Map::new();
            value.insert("_key".into(), json!(key));
            value.insert("_type".into(), json!("block"));
            value.insert("style".into(), json!(style.as_str()));
            value.insert("children".into(), spans_json(key, text, &stored));
            value.insert("markDefs".into(), json!([]));
            if let Some(list_item) = list_item {
                value.insert("listItem".into(), json!(list_item.as_str()));
                value.insert("level".into(), json!(level));
            }
            Value::Object(value)
        }
    }
}

fn spans_json(key: &str, text: &str, marks: &[MarkRange]) -> Value {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return json!([{
            "_key": format!("{key}-0"),
            "_type": "span",
            "text": "",
            "marks": []
        }]);
    }
    let mut bounds = BTreeSet::new();
    bounds.insert(0);
    bounds.insert(chars.len());
    for mark in marks {
        bounds.insert(mark.start.min(chars.len()));
        bounds.insert(mark.end.min(chars.len()));
    }
    let bounds: Vec<usize> = bounds.into_iter().collect();
    let mut spans = Vec::new();
    for (index, pair) in bounds.windows(2).enumerate() {
        let (start, end) = (pair[0], pair[1]);
        if start == end {
            continue;
        }
        let piece: String = chars[start..end].iter().collect();
        let mut active: Vec<&str> = marks
            .iter()
            .filter(|mark| mark.start <= start && mark.end >= end)
            .map(|mark| mark.mark.as_str())
            .collect();
        active.sort_unstable();
        active.dedup();
        spans.push(json!({
            "_key": format!("{key}-{index}"),
            "_type": "span",
            "text": piece,
            "marks": active
        }));
    }
    Value::Array(spans)
}

fn validate_text_block(obj: &Map<String, Value>, index: usize) -> Result<(), String> {
    reject_unknown(
        obj,
        &[
            "_key", "_type", "style", "children", "markDefs", "listItem", "level",
        ],
        &format!("block {index}"),
    )?;
    let style = require_str(obj, "style")?;
    if BlockStyle::parse(style).is_none() {
        return Err(format!("block {index} has unsupported style '{style}'"));
    }
    if let Some(list_item) = obj.get("listItem") {
        let list_item = list_item
            .as_str()
            .ok_or_else(|| format!("block {index} listItem must be a string"))?;
        if ListItem::parse(list_item).is_none() {
            return Err(format!(
                "block {index} has unsupported listItem '{list_item}'"
            ));
        }
    }
    if let Some(level) = obj.get("level") {
        if obj.get("listItem").is_none() {
            return Err(format!("block {index} level requires listItem"));
        }
        if !level.as_u64().is_some_and(|level| (1..=6).contains(&level)) {
            return Err(format!("block {index} level must be between 1 and 6"));
        }
    }
    if let Some(mark_defs) = obj.get("markDefs")
        && mark_defs.as_array().is_none_or(|items| !items.is_empty()) {
            return Err(format!("block {index} markDefs must be an empty array"));
        }
    let children = obj
        .get("children")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("block {index} children must be an array"))?;
    if children.is_empty() {
        return Err(format!("block {index} must contain a span"));
    }
    for (span_index, child) in children.iter().enumerate() {
        let span = child
            .as_object()
            .ok_or_else(|| format!("block {index} span {span_index} must be an object"))?;
        validate_span(span, index, span_index)?;
    }
    Ok(())
}

fn validate_span(span: &Map<String, Value>, block: usize, index: usize) -> Result<(), String> {
    reject_unknown(
        span,
        &["_key", "_type", "text", "marks"],
        &format!("block {block} span {index}"),
    )?;
    if require_str(span, "_type")? != "span" {
        return Err(format!("block {block} span {index} must have _type span"));
    }
    let key = require_str(span, "_key")?;
    if !is_block_key(key) {
        return Err(format!("block {block} span {index} has an invalid _key"));
    }
    if span.get("text").and_then(Value::as_str).is_none() {
        return Err(format!("block {block} span {index} text must be a string"));
    }
    let marks = span
        .get("marks")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("block {block} span {index} marks must be an array"))?;
    let mut seen = HashSet::new();
    for mark in marks {
        let mark = mark
            .as_str()
            .ok_or_else(|| format!("block {block} span {index} marks must be strings"))?;
        if Mark::parse(mark).is_none() {
            return Err(format!(
                "block {block} span {index} has unsupported mark '{mark}'"
            ));
        }
        if !seen.insert(mark) {
            return Err(format!("block {block} span {index} repeats mark '{mark}'"));
        }
    }
    Ok(())
}

fn validate_image_block(obj: &Map<String, Value>, index: usize) -> Result<String, String> {
    reject_unknown(obj, &["_key", "_type", "asset"], &format!("block {index}"))?;
    let asset = obj
        .get("asset")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("block {index} asset must be an object"))?;
    reject_unknown(asset, &["_type", "_ref"], &format!("block {index} asset"))?;
    if let Some(kind) = asset.get("_type")
        && kind.as_str() != Some("reference") {
            return Err(format!("block {index} asset _type must be reference"));
        }
    let asset_id = require_str(asset, "_ref")?;
    if is_uuid(asset_id) {
        Ok(asset_id.to_string())
    } else {
        Err(format!("block {index} asset _ref must be an id"))
    }
}

fn reject_unknown(obj: &Map<String, Value>, allowed: &[&str], label: &str) -> Result<(), String> {
    for key in obj.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("{label} has unknown field '{key}'"));
        }
    }
    Ok(())
}

fn require_str<'a>(obj: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    obj.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing {key}"))
}

fn is_block_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn range_covered(marks: &[MarkRange], mark: Mark, start: usize, end: usize) -> bool {
    let mut covered: Vec<(usize, usize)> = marks
        .iter()
        .filter(|item| item.mark == mark)
        .map(|item| (item.start, item.end))
        .collect();
    covered.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start_at, end_at) in covered {
        if let Some(last) = merged.last_mut()
            && start_at <= last.1 {
                last.1 = last.1.max(end_at);
                continue;
            }
        merged.push((start_at, end_at));
    }
    merged
        .iter()
        .any(|(covered_start, covered_end)| *covered_start <= start && *covered_end >= end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn custom_blocks_round_trip_and_validate_nested_assets() {
        let asset = "550e8400-e29b-41d4-a716-446655440000";
        let blocks = json!([
            {"_key":"accordion", "_type":"accordion", "title":"Details", "collapsed":true, "content":[
                {"_key":"repeat", "_type":"repeatable", "title":"Examples", "items":[
                    {"_key":"entry", "label":"First", "content":[
                        {"_key":"picture", "_type":"image", "asset":{"_ref":asset}},
                        {"_key":"embed", "_type":"iframe", "title":"Video", "url":"https://example.com/embed", "height":360}
                    ]}
                ]}
            ]}
        ]);
        assert_eq!(validate_portable_text(&blocks).unwrap(), vec![asset]);
        assert_eq!(
            editor_blocks_to_json(&editor_blocks_from_json(&blocks).unwrap()),
            blocks
        );
        let mut duplicate = blocks.clone();
        duplicate[0]["content"][0]["items"][0]["_key"] = json!("accordion");
        assert!(validate_portable_text(&duplicate).is_err());
        let mut invalid = blocks.clone();
        invalid[0]["content"][0]["items"][0]["content"][1]["url"] = json!("javascript:alert(1)");
        assert!(validate_portable_text(&invalid).is_err());
    }

    #[test]
    fn custom_blocks_reject_unsafe_urls_and_invalid_shapes() {
        for url in [
            "javascript:alert(1)",
            "data:text/html,test",
            "//example.com/embed",
            "https:example.com",
            "https://user:password@example.com/",
            "https://example.com/\n",
            "https://example.com\\@other.com/",
        ] {
            assert!(validate_embed_url(url).is_err(), "accepted {url}");
        }
        for url in [
            "https://www.youtube-nocookie.com/embed/abcdef12345",
            "http://localhost:3000/embed",
            "https://example.com/?a=b%20c",
        ] {
            assert!(validate_embed_url(url).is_ok());
        }
        for block in [
            json!({"_key":"a", "_type":"iframe", "url":"https://example.com", "title":"", "height":360}),
            json!({"_key":"a", "_type":"iframe", "url":"https://example.com", "title":"Embed", "height":150}),
            json!({"_key":"a", "_type":"iframe", "url":"https://example.com", "title":"Embed", "height":360, "srcdoc":"bad"}),
            json!({"_key":"a", "_type":"repeatable", "title":"Entries", "items":[]}),
            json!({"_key":"a", "_type":"accordion", "title":"Details", "collapsed":"true", "content":[]}),
        ] {
            assert!(validate_portable_text(&json!([block])).is_err());
        }
        let mut nested = json!([]);
        for index in 0..6 {
            nested = json!([{"_key":format!("a{index}"), "_type":"accordion", "title":"Details", "collapsed":false, "content":nested}]);
        }
        assert!(validate_portable_text(&nested).is_ok());
        nested = json!([{"_key":"outer", "_type":"accordion", "title":"Details", "collapsed":false, "content":nested}]);
        assert!(validate_portable_text(&nested).is_err());
    }

    #[test]
    fn round_trips_marks_and_images() {
        let asset = "550e8400-e29b-41d4-a716-446655440000";
        let blocks = vec![
            EditorBlock::Text {
                key: "k1".into(),
                style: BlockStyle::H1,
                list_item: Some(ListItem::Bullet),
                level: 2,
                text: "Hello".into(),
                marks: vec![
                    MarkRange {
                        start: 1,
                        end: 3,
                        mark: Mark::Underline,
                    },
                    MarkRange {
                        start: 0,
                        end: 5,
                        mark: Mark::Strike,
                    },
                ],
            },
            EditorBlock::Image {
                key: "k2".into(),
                asset_id: asset.into(),
            },
        ];
        let value = editor_blocks_to_json(&blocks);
        let assets = validate_portable_text(&value).unwrap();
        assert_eq!(assets, vec![asset]);
        assert_eq!(editor_blocks_from_json(&value).unwrap(), blocks);
    }

    #[test]
    fn rejects_unknown_marks() {
        let err = validate_portable_text(&json!([{
            "_key": "k",
            "_type": "block",
            "style": "normal",
            "markDefs": [],
            "children": [{
                "_key": "s",
                "_type": "span",
                "text": "Hi",
                "marks": ["rainbow"]
            }]
        }]))
        .unwrap_err();
        assert!(err.contains("unsupported mark"));
    }

    #[test]
    fn rejects_list_depth_outside_editor_limits() {
        let mut block = json!([{"_key":"key","_type":"block","style":"normal","listItem":"bullet","level":6,"children":[{"_key":"span","_type":"span","text":"Nested","marks":["underline","strike"]}]}]);
        assert!(validate_portable_text(&block).is_ok());
        for level in [0, 7] {
            block[0]["level"] = json!(level);
            assert!(validate_portable_text(&block).is_err());
        }
    }

    #[test]
    fn toggles_mark_ranges() {
        let mut marks = Vec::new();
        toggle_mark(&mut marks, Mark::Strong, 1, 4);
        assert_eq!(marks.len(), 1);
        toggle_mark(&mut marks, Mark::Strong, 2, 3);
        assert_eq!(
            marks,
            vec![
                MarkRange {
                    start: 1,
                    end: 2,
                    mark: Mark::Strong
                },
                MarkRange {
                    start: 3,
                    end: 4,
                    mark: Mark::Strong
                }
            ]
        );
    }

    #[test]
    fn utf16_offsets_count_emoji_as_one_character() {
        assert_eq!(utf16_to_char_index("a😀b", 1), 1);
        assert_eq!(utf16_to_char_index("a😀b", 3), 2);
    }
}
