//! A Vulse SQL text box with syntax highlighting and autocomplete for
//! keywords, collections, fields and functions. Ctrl/Cmd + Enter runs.

use vulse_model::{CollectionView, SQL_AGGREGATES, SQL_KEYWORDS, SQL_RECORD_COLUMNS};
use web_sys::{HtmlElement, HtmlTextAreaElement};
use yew::prelude::*;

/// Highlighting classes for the text, covering every character.
pub(crate) fn highlight(sql: &str) -> Vec<(&'static str, String)> {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let start = i;
        let c = chars[i];
        let class = if c == '-' && chars.get(i + 1) == Some(&'-') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            "sql-comment"
        } else if c == '\'' || c == '"' || c == '`' {
            i += 1;
            while i < chars.len() {
                if chars[i] == c {
                    if chars.get(i + 1) == Some(&c) {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            if c == '\'' { "sql-string" } else { "sql-name" }
        } else if c.is_ascii_digit() {
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            "sql-number"
        } else if c.is_alphabetic() || c == '_' {
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let upper = word.to_ascii_uppercase();
            if SQL_KEYWORDS.contains(&upper.as_str()) {
                "sql-keyword"
            } else if chars.get(i) == Some(&'(') {
                "sql-function"
            } else {
                "sql-name"
            }
        } else if c.is_whitespace() {
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            ""
        } else {
            i += 1;
            "sql-operator"
        };
        out.push((class, chars[start..i].iter().collect()));
    }
    out
}

#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Choice {
    pub insert: String,
    pub label: String,
    pub detail: String,
}

/// The word being typed before `caret` (a char index): its start and text.
fn word_at(sql: &str, caret: usize) -> Option<(usize, String)> {
    let before: Vec<char> = sql.chars().take(caret).collect();
    // Inside a quoted text or a comment, nothing is suggested.
    let mut quote = None;
    let mut comment = false;
    for (i, c) in before.iter().enumerate() {
        if comment {
            comment = *c != '\n';
        } else if let Some(q) = quote {
            if *c == q {
                quote = None;
            }
        } else if matches!(c, '\'' | '"' | '`') {
            quote = Some(*c);
        } else if *c == '-' && before.get(i + 1) == Some(&'-') {
            comment = true;
        }
    }
    if quote.is_some() || comment {
        return None;
    }
    let length = before
        .iter()
        .rev()
        .take_while(|c| c.is_alphanumeric() || **c == '_' || **c == '-')
        .count();
    let start = before.len() - length;
    let word: String = before[start..].iter().collect();
    (length > 0 && !word.starts_with('-') && !before[start].is_ascii_digit())
        .then_some((start, word))
}

/// The keyword before `start`, upper-cased.
fn previous_word(sql: &str, start: usize) -> String {
    let before: String = sql.chars().take(start).collect();
    before
        .split(|c: char| c.is_whitespace() || c == ',' || c == '(')
        .rev()
        .find(|w| !w.is_empty())
        .unwrap_or_default()
        .to_ascii_uppercase()
}

/// The collection a statement is about: the name after FROM or UPDATE.
pub(crate) fn statement_collection<'a>(
    sql: &str,
    collections: &'a [CollectionView],
) -> Option<&'a CollectionView> {
    let words: Vec<&str> = sql
        .split(|c: char| c.is_whitespace() || c == ';' || c == '(' || c == ')' || c == ',')
        .filter(|w| !w.is_empty())
        .collect();
    words.windows(2).find_map(|pair| {
        let keyword = pair[0].to_ascii_uppercase();
        let name = pair[1].trim_matches(['"', '`']).to_ascii_lowercase();
        if keyword == "FROM" || keyword == "UPDATE" {
            collections.iter().find(|c| c.name == name)
        } else {
            None
        }
    })
}

pub(crate) fn choices(
    sql: &str,
    caret: usize,
    collections: &[CollectionView],
) -> Option<(usize, Vec<Choice>)> {
    let (start, word) = word_at(sql, caret)?;
    let query = word.to_lowercase();
    let previous = previous_word(sql, start);
    let mut out = Vec::new();
    if previous == "FROM" || previous == "UPDATE" {
        for c in collections {
            if c.name.starts_with(&query) {
                out.push(Choice {
                    insert: c.name.clone(),
                    label: c.name.clone(),
                    detail: format!("collection · {} fields", c.blueprint.fields.len()),
                });
            }
        }
        return (!out.is_empty()).then_some((start, out));
    }
    if let Some(collection) = statement_collection(sql, collections) {
        for f in &collection.blueprint.fields {
            if f.name.starts_with(&query) || f.display_label().to_lowercase().starts_with(&query) {
                out.push(Choice {
                    insert: f.name.clone(),
                    label: f.name.clone(),
                    detail: format!(
                        "{} · {}",
                        f.display_label(),
                        crate::formula_builder::type_label(f)
                    ),
                });
            }
        }
        for name in SQL_RECORD_COLUMNS {
            if name.starts_with(&query) {
                out.push(Choice {
                    insert: name.into(),
                    label: name.into(),
                    detail: "record property".into(),
                });
            }
        }
    }
    let upper = word.to_ascii_uppercase();
    for keyword in SQL_KEYWORDS {
        if keyword.starts_with(&upper) && *keyword != upper {
            out.push(Choice {
                insert: keyword.to_string(),
                label: keyword.to_string(),
                detail: "keyword".into(),
            });
        }
    }
    for name in SQL_AGGREGATES {
        if name.starts_with(&upper) {
            out.push(Choice {
                insert: format!("{name}("),
                label: format!("{name}(…)"),
                detail: "adds up the records in each group".into(),
            });
        }
    }
    for (name, usage) in vulse_model::FUNCTIONS {
        if name.starts_with(&upper)
            && !SQL_AGGREGATES.contains(name)
            && !name.ends_with("IFS")
            && *name != "XLOOKUP"
        {
            out.push(Choice {
                insert: format!("{name}("),
                label: usage.to_string(),
                detail: crate::formula_builder::describe(name).0.into(),
            });
        }
    }
    out.truncate(12);
    (!out.is_empty()).then_some((start, out))
}

fn accept(sql: &str, start: usize, caret: usize, choice: &Choice) -> (String, usize) {
    let chars: Vec<char> = sql.chars().collect();
    let mut end = caret;
    while chars
        .get(end)
        .is_some_and(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
    {
        end += 1;
    }
    if choice.insert.ends_with('(') && chars.get(end) == Some(&'(') {
        end += 1;
    }
    let next = crate::formula_builder::replace_chars(sql, start, end, &choice.insert);
    let caret = next
        .chars()
        .take(start + choice.insert.chars().count())
        .map(char::len_utf16)
        .sum();
    (next, caret)
}

#[derive(Properties, PartialEq)]
pub(crate) struct SqlEditorProps {
    pub value: String,
    pub on_change: Callback<String>,
    pub on_run: Callback<()>,
    pub collections: Vec<CollectionView>,
}

#[function_component(SqlEditor)]
pub(crate) fn sql_editor(props: &SqlEditorProps) -> Html {
    let area = use_node_ref();
    let backdrop = use_node_ref();
    let caret = use_state(|| props.value.encode_utf16().count());
    let focused = use_state(|| false);
    let active = use_state(|| 0usize);
    let dismissed = use_state(|| None::<(String, usize)>);
    let at = vulse_model::utf16_to_char_index(&props.value, *caret);
    let suggestions = choices(&props.value, at, &props.collections);
    let open = *focused
        && suggestions.is_some()
        && dismissed.as_ref() != Some(&(props.value.clone(), *caret));
    let options = suggestions
        .as_ref()
        .map(|(_, o)| o.clone())
        .unwrap_or_default();
    let current = (*active).min(options.len().saturating_sub(1));

    let read = {
        let area = area.clone();
        let caret = caret.clone();
        Callback::from(move |_: ()| {
            if let Some(a) = area.cast::<HtmlTextAreaElement>() {
                let start = a.selection_start().ok().flatten().unwrap_or(0) as usize;
                if *caret != start {
                    caret.set(start);
                }
            }
        })
    };
    let apply = {
        let area = area.clone();
        let caret = caret.clone();
        let active = active.clone();
        let change = props.on_change.clone();
        Callback::from(move |(next, position): (String, usize)| {
            if let Some(a) = area.cast::<HtmlTextAreaElement>() {
                a.set_value(&next);
                a.focus().ok();
                a.set_selection_range(position as u32, position as u32).ok();
            }
            caret.set(position);
            active.set(0);
            change.emit(next);
        })
    };
    let onkeydown = {
        let value = props.value.clone();
        let suggestions = suggestions.clone();
        let apply = apply.clone();
        let active = active.clone();
        let dismissed = dismissed.clone();
        let run = props.on_run.clone();
        let position = *caret;
        Callback::from(move |e: KeyboardEvent| {
            if e.key() == "Enter" && (e.ctrl_key() || e.meta_key()) {
                e.prevent_default();
                run.emit(());
                return;
            }
            let Some((start, options)) = suggestions.as_ref().filter(|_| open) else {
                return;
            };
            match e.key().as_str() {
                "ArrowDown" => {
                    e.prevent_default();
                    active.set((current + 1) % options.len());
                }
                "ArrowUp" => {
                    e.prevent_default();
                    active.set((current + options.len() - 1) % options.len());
                }
                "Enter" | "Tab" => {
                    e.prevent_default();
                    apply.emit(accept(&value, *start, at, &options[current]));
                }
                "Escape" => {
                    e.prevent_default();
                    dismissed.set(Some((value.clone(), position)));
                }
                _ => {}
            }
        })
    };
    // The box grows with the statement, so the highlighting below never
    // wraps differently from the text.
    {
        let area = area.clone();
        use_effect_with(props.value.clone(), move |_| {
            if let Some(a) = area.cast::<HtmlTextAreaElement>() {
                let _ = a.set_attribute("style", "height:auto");
                let height = a.scroll_height().max(200) + 2;
                let _ = a.set_attribute("style", &format!("height:{height}px"));
            }
        });
    }
    let onscroll = {
        let area = area.clone();
        let backdrop = backdrop.clone();
        Callback::from(move |_: Event| {
            if let (Some(a), Some(b)) = (
                area.cast::<HtmlTextAreaElement>(),
                backdrop.cast::<HtmlElement>(),
            ) {
                b.set_scroll_top(a.scroll_top());
                b.set_scroll_left(a.scroll_left());
            }
        })
    };
    let choose = |choice: Choice| {
        let value = props.value.clone();
        let start = suggestions.as_ref().map(|(s, _)| *s).unwrap_or(0);
        let apply = apply.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            apply.emit(accept(&value, start, at, &choice));
        })
    };
    html! {<div class="sql-editor">
        <div class="sql-field">
            <pre class="sql-highlight" ref={backdrop} aria-hidden="true"><code>
                {for highlight(&props.value).into_iter().map(|(class, text)| if class.is_empty() {html!{{text}}} else {html!{<span class={class}>{text}</span>}})}
                {"\n "}
            </code></pre>
            <textarea ref={area} class="sql-input" aria-label="SQL statement" spellcheck="false" autocomplete="off" rows="8"
                role="combobox" aria-autocomplete="list" aria-expanded={open.to_string()} aria-controls="sql-suggestions"
                aria-activedescendant={open.then(|| format!("sql-option-{current}"))}
                placeholder="SELECT * FROM collection WHERE field = 'value'"
                value={props.value.clone()} {onkeydown} {onscroll}
                oninput={Callback::from({let change=props.on_change.clone();let read=read.clone();let active=active.clone();move |e:InputEvent|{let a:HtmlTextAreaElement=e.target_unchecked_into();active.set(0);change.emit(a.value());read.emit(());}})}
                onkeyup={Callback::from({let read=read.clone();move |_:KeyboardEvent|read.emit(())})}
                onclick={Callback::from({let read=read.clone();move |_|read.emit(())})}
                onfocus={Callback::from({let focused=focused.clone();let read=read.clone();move |_|{focused.set(true);read.emit(());}})}
                onblur={Callback::from({let focused=focused.clone();move |_|focused.set(false)})}/>
            if open {<div class="sql-suggestions" id="sql-suggestions" role="listbox" aria-label="SQL suggestions">
                {for options.iter().enumerate().map(|(index, choice)| html!{
                    <div role="option" id={format!("sql-option-{index}")} aria-selected={(index == current).to_string()} class={if index == current {"formula-option active"} else {"formula-option"}} onmousedown={choose(choice.clone())}>
                        <strong>{choice.label.clone()}</strong><span>{choice.detail.clone()}</span>
                    </div>
                })}
            </div>}
        </div>
    </div>}
}

#[cfg(test)]
mod tests {
    use super::*;
    use vulse_model::Blueprint;

    fn collections() -> Vec<CollectionView> {
        let blueprint: Blueprint = serde_json::from_value(serde_json::json!({"fields":[
            {"name":"aar","type":"number","label":"År"},{"name":"navn","type":"string"}
        ]}))
        .unwrap();
        ["aarsmelding", "pensjon-2024"]
            .iter()
            .map(|name| CollectionView {
                id: name.to_string(),
                name: name.to_string(),
                visibility: vulse_model::Visibility::Private,
                blueprint: blueprint.clone(),
                created_at: String::new(),
                updated_at: String::new(),
            })
            .collect()
    }

    #[test]
    fn suggests_by_context() {
        let all = collections();
        let names = |sql: &str| -> Vec<String> {
            choices(sql, sql.chars().count(), &all)
                .map(|(_, o)| o.into_iter().map(|c| c.insert).collect())
                .unwrap_or_default()
        };
        assert_eq!(names("SELECT * FROM pen"), ["pensjon-2024"]);
        assert_eq!(names("DELETE FROM pensjon-2"), ["pensjon-2024"]);
        assert_eq!(names("SELECT na"), Vec::<String>::new());
        assert_eq!(names("SELECT * FROM aarsmelding WHERE na"), ["navn"]);
        assert!(names("SELECT * FROM aarsmelding WHERE a").starts_with(&["aar".to_string()]));
        assert_eq!(names("sel"), ["SELECT"]);
        assert!(names("SELECT rou").contains(&"ROUND(".to_string()));
        assert!(names("SELECT co").contains(&"COUNT(".to_string()));
        assert_eq!(
            names("SELECT * FROM aarsmelding WHERE navn = 'na"),
            Vec::<String>::new()
        );
        assert_eq!(names("-- se"), Vec::<String>::new());
    }

    #[test]
    fn accepting_replaces_the_word() {
        let all = collections();
        let sql = "UPDATE aarsmelding SET na = 1";
        let (start, options) = choices(sql, 25, &all).unwrap();
        assert_eq!(
            accept(sql, start, 25, &options[0]),
            ("UPDATE aarsmelding SET navn = 1".into(), 27)
        );
        let (start, options) = choices("SELECT cou", 10, &all).unwrap();
        assert_eq!(
            accept("SELECT cou", start, 10, &options[0]).0,
            "SELECT COUNT("
        );
    }

    #[test]
    fn highlighting_covers_all_text() {
        let sql = "SELECT navn, COUNT(*) FROM b WHERE x = 'it''s' -- note\nAND y > 2.5";
        let parts = highlight(sql);
        assert_eq!(
            parts.iter().map(|(_, t)| t.as_str()).collect::<String>(),
            sql
        );
        assert!(parts.contains(&("sql-keyword", "SELECT".into())));
        assert!(parts.contains(&("sql-function", "COUNT".into())));
        assert!(parts.contains(&("sql-string", "'it''s'".into())));
        assert!(parts.contains(&("sql-comment", "-- note".into())));
        assert!(parts.contains(&("sql-number", "2.5".into())));
    }
}
