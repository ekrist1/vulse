//! A formula text box with autocomplete for fields and functions, help for
//! the function being typed, a syntax check and an optional function
//! library. Used by calculated fields and summary formulas.

use crate::formula_builder::{
    FunctionLibrary, argument_hint, describe, insert_function, replace_chars,
};
use web_sys::HtmlTextAreaElement;
use yew::prelude::*;

/// A name that can go inside `[...]`.
#[derive(Clone, PartialEq, Debug)]
pub struct Suggestion {
    pub name: String,
    pub label: String,
    pub detail: String,
}

#[derive(Clone, PartialEq, Debug)]
enum Context {
    /// Inside `[`, from the bracket's char index.
    Field { start: usize, query: String },
    /// A word that may become a function name, from its char index.
    Function { start: usize, query: String },
}

#[derive(Clone, PartialEq, Debug)]
struct Choice {
    insert: String,
    title: String,
    detail: String,
}

/// What the text before `caret` (a char index) is in the middle of typing.
fn context(formula: &str, caret: usize) -> Option<Context> {
    let before: Vec<char> = formula.chars().take(caret).collect();
    let mut quoted = false;
    let mut open = None;
    let mut i = 0;
    while i < before.len() {
        let c = before[i];
        if c == '"' {
            if quoted && before.get(i + 1) == Some(&'"') {
                i += 1;
            } else {
                quoted = !quoted;
            }
        } else if !quoted && c == '[' {
            open = Some(i);
        } else if !quoted && c == ']' {
            open = None;
        }
        i += 1;
    }
    if quoted {
        return None;
    }
    if let Some(start) = open {
        return Some(Context::Field {
            start,
            query: before[start + 1..].iter().collect(),
        });
    }
    let word = before
        .iter()
        .rev()
        .take_while(|c| c.is_ascii_alphabetic())
        .count();
    let start = before.len() - word;
    let after_name = start > 0 && {
        let c = before[start - 1];
        c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ']')
    };
    (word > 0 && !after_name).then(|| Context::Function {
        start,
        query: before[start..].iter().collect(),
    })
}

/// The innermost function call around `caret` and the argument index there.
fn call_at(formula: &str, caret: usize) -> Option<(String, usize)> {
    let chars: Vec<char> = formula.chars().take(caret).collect();
    let mut stack: Vec<(Option<String>, usize)> = Vec::new();
    let mut quoted = false;
    let mut bracket = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if quoted {
            if c == '"' {
                if chars.get(i + 1) == Some(&'"') {
                    i += 1;
                } else {
                    quoted = false;
                }
            }
        } else if bracket {
            bracket = c != ']';
        } else {
            match c {
                '"' => quoted = true,
                '[' => bracket = true,
                '(' => {
                    let name: String = chars[..i]
                        .iter()
                        .rev()
                        .take_while(|c| c.is_ascii_alphabetic())
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                    let name = name.to_ascii_uppercase();
                    let known = vulse_model::FUNCTIONS.iter().any(|(n, _)| *n == name);
                    stack.push((known.then_some(name), 0));
                }
                ')' => {
                    stack.pop();
                }
                ',' | ';' => {
                    if let Some(top) = stack.last_mut() {
                        top.1 += 1;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    stack
        .into_iter()
        .rev()
        .find_map(|(name, index)| name.map(|n| (n, index)))
}

fn choices(context: &Context, suggestions: &[Suggestion]) -> Vec<Choice> {
    match context {
        Context::Field { query, .. } => {
            let query = query.to_lowercase();
            let mut found: Vec<(bool, &Suggestion)> = suggestions
                .iter()
                .filter_map(|s| {
                    let prefix = s.name.starts_with(&query);
                    (prefix || s.name.contains(&query) || s.label.to_lowercase().contains(&query))
                        .then_some((prefix, s))
                })
                .collect();
            // Names starting with the typed text first; other collections
            // only once their name is being typed.
            found.sort_by_key(|(prefix, s)| (!prefix, s.name.contains('.')));
            found
                .into_iter()
                .filter(|(_, s)| !s.name.contains('.') || !query.is_empty())
                .take(30)
                .map(|(_, s)| Choice {
                    insert: s.name.clone(),
                    title: format!("{} [{}]", s.label, s.name),
                    detail: s.detail.clone(),
                })
                .collect()
        }
        Context::Function { query, .. } => {
            let query = query.to_ascii_uppercase();
            let mut found: Vec<_> = vulse_model::FUNCTIONS
                .iter()
                .filter(|(name, _)| name.starts_with(&query))
                .collect();
            // An exact name first, so typing SUM offers SUM before SUMIF.
            found.sort_by_key(|(name, _)| *name != query.as_str());
            found
                .into_iter()
                .map(|(name, usage)| Choice {
                    insert: name.to_string(),
                    title: usage.to_string(),
                    detail: describe(name).0.to_string(),
                })
                .collect()
        }
    }
}

/// Applies a completion: the new formula and the caret (UTF-16) after it.
fn accept(formula: &str, context: &Context, caret: usize, choice: &Choice) -> (String, usize) {
    let chars: Vec<char> = formula.chars().collect();
    let (start, text) = match context {
        Context::Field { start, .. } => {
            let mut end = caret;
            while chars
                .get(end)
                .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
            {
                end += 1;
            }
            if chars.get(end) == Some(&']') {
                end += 1;
            }
            (*start, (end, format!("[{}]", choice.insert)))
        }
        Context::Function { start, .. } => {
            let mut end = caret;
            while chars.get(end).is_some_and(|c| c.is_ascii_alphabetic()) {
                end += 1;
            }
            // An existing '(' is kept rather than doubled.
            if chars.get(end) == Some(&'(') {
                end += 1;
            }
            (*start, (end, format!("{}(", choice.insert)))
        }
    };
    let (end, insert) = text;
    let prefix = if start == 0 && !formula.starts_with('=') {
        "="
    } else {
        ""
    };
    let next = replace_chars(formula, start, end, &format!("{prefix}{insert}"));
    let caret = next
        .chars()
        .take(start + prefix.len() + insert.chars().count())
        .map(char::len_utf16)
        .sum();
    (next, caret)
}

/// A syntax or field problem, or `None` when the formula looks valid.
/// `lookups` says whether `[collection.field]` references are allowed.
fn problem(formula: &str, suggestions: &[Suggestion], lookups: bool) -> Option<String> {
    let trimmed = formula.trim();
    if trimmed.is_empty() || trimmed == "=" {
        return None;
    }
    if let Err(message) = vulse_model::inspect_formula(formula) {
        return Some(message);
    }
    let dependencies = vulse_model::formula_dependencies(formula).unwrap_or_default();
    if !lookups
        && let Some((collection, field)) = dependencies.iter().find_map(|n| n.split_once('.'))
    {
        let message = vulse_model::lookup_message(collection, field);
        let mut chars = message.chars();
        return chars
            .next()
            .map(|first| first.to_uppercase().collect::<String>() + chars.as_str());
    }
    let unknown: Vec<&String> = dependencies
        .iter()
        .filter(|name| !suggestions.iter().any(|s| s.name == **name))
        .collect();
    if let Some((collection, field)) = unknown.iter().find_map(|n| n.split_once('.')) {
        return Some(
            if suggestions
                .iter()
                .any(|s| s.name.starts_with(&format!("{collection}.")))
            {
                format!("Collection '{collection}' has no field '{field}'")
            } else {
                format!("Unknown collection '{collection}' in [{collection}.{field}]")
            },
        );
    }
    (!unknown.is_empty()).then(|| {
        let names: Vec<String> = unknown.iter().map(|n| format!("[{n}]")).collect();
        format!("Unknown field {}", names.join(", "))
    })
}

#[derive(Properties, PartialEq)]
pub struct FormulaInputProps {
    pub value: String,
    pub on_change: Callback<String>,
    pub suggestions: Vec<Suggestion>,
    #[prop_or(AttrValue::from("Formula"))]
    pub label: AttrValue,
    #[prop_or_default]
    pub placeholder: AttrValue,
    #[prop_or(3)]
    pub rows: u32,
    /// Shows a Functions button that opens the function library.
    #[prop_or_default]
    pub library: bool,
    /// Whether formulas may read other collections as `[collection.field]`.
    #[prop_or_default]
    pub lookups: bool,
    #[prop_or_default]
    pub textarea: NodeRef,
    /// The selection (UTF-16 offsets) whenever it changes.
    #[prop_or_default]
    pub on_selection: Callback<(usize, usize)>,
}

#[function_component(FormulaInput)]
pub fn formula_input(props: &FormulaInputProps) -> Html {
    let selection = use_state(|| {
        let end = props.value.encode_utf16().count();
        (end, end)
    });
    let focused = use_state(|| false);
    let active = use_state(|| 0usize);
    let dismissed = use_state(|| None::<(String, usize)>);
    let library = use_state(|| false);
    let listbox = use_memo((), |_| format!("formula-{}", uuid::Uuid::new_v4().simple()));

    let caret = vulse_model::utf16_to_char_index(&props.value, selection.0);
    let context = (selection.0 == selection.1)
        .then(|| context(&props.value, caret))
        .flatten();
    let options = context
        .as_ref()
        .map(|c| choices(c, &props.suggestions))
        .unwrap_or_default();
    let open = *focused
        && !options.is_empty()
        && dismissed.as_ref() != Some(&(props.value.clone(), selection.0));
    let current = (*active).min(options.len().saturating_sub(1));

    let read_selection = {
        let node = props.textarea.clone();
        let selection = selection.clone();
        let on_selection = props.on_selection.clone();
        Callback::from(move |_: ()| {
            if let Some(area) = node.cast::<HtmlTextAreaElement>() {
                let start = area.selection_start().ok().flatten().unwrap_or(0) as usize;
                let end = area.selection_end().ok().flatten().unwrap_or(0) as usize;
                if *selection != (start, end) {
                    selection.set((start, end));
                }
                on_selection.emit((start, end));
            }
        })
    };
    // Writes a new formula and caret into the text box.
    let apply = {
        let node = props.textarea.clone();
        let change = props.on_change.clone();
        let selection = selection.clone();
        let active = active.clone();
        let on_selection = props.on_selection.clone();
        Callback::from(move |(next, caret): (String, usize)| {
            if let Some(area) = node.cast::<HtmlTextAreaElement>() {
                area.set_value(&next);
                area.focus().ok();
                area.set_selection_range(caret as u32, caret as u32).ok();
            }
            selection.set((caret, caret));
            active.set(0);
            on_selection.emit((caret, caret));
            change.emit(next);
        })
    };
    let choose = {
        let value = props.value.clone();
        let context = context.clone();
        let apply = apply.clone();
        move |choice: Choice| {
            let value = value.clone();
            let context = context.clone();
            let apply = apply.clone();
            Callback::from(move |e: MouseEvent| {
                // Keep focus in the text box.
                e.prevent_default();
                if let Some(context) = &context {
                    apply.emit(accept(&value, context, caret, &choice));
                }
            })
        }
    };
    let onkeydown = {
        let options = options.clone();
        let context = context.clone();
        let value = props.value.clone();
        let apply = apply.clone();
        let active = active.clone();
        let dismissed = dismissed.clone();
        let position = selection.0;
        Callback::from(move |e: KeyboardEvent| {
            if !open {
                return;
            }
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
                    if let Some(context) = &context {
                        apply.emit(accept(&value, context, caret, &options[current]));
                    }
                }
                "Escape" => {
                    e.prevent_default();
                    dismissed.set(Some((value.clone(), position)));
                }
                _ => {}
            }
        })
    };
    let oninput = {
        let change = props.on_change.clone();
        let read_selection = read_selection.clone();
        let active = active.clone();
        Callback::from(move |e: InputEvent| {
            let area: HtmlTextAreaElement = e.target_unchecked_into();
            active.set(0);
            change.emit(area.value());
            read_selection.emit(());
        })
    };
    let signature = call_at(&props.value, caret).map(|(name, index)| {
        let (description, example) = describe(&name);
        let usage = vulse_model::FUNCTIONS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, usage)| *usage)
            .unwrap_or_default();
        html! {<div class="formula-signature" aria-label="Function help">
            <code>{usage}</code>
            if let Some(hint) = argument_hint(&name, index) {<p><strong>{format!("Argument {}: ", index + 1)}</strong>{hint}</p>}
            <p class="muted">{description}</p>
            <p class="muted">{"Example: "}<code>{format!("={example}")}</code></p>
        </div>}
    });
    let problem = problem(&props.value, &props.suggestions, props.lookups);
    let insert_function = {
        let value = props.value.clone();
        let selection = *selection;
        let apply = apply.clone();
        let library = library.clone();
        Callback::from(move |name: String| {
            apply.emit(insert_function(&value, selection, &name));
            library.set(false);
        })
    };
    let input_id = format!("{}-input", *listbox);
    html! {<div class="formula-input">
        <label class="formula-label" for={input_id.clone()}>{props.label.clone()}</label>
        <div class="formula-field">
            <textarea id={input_id} ref={props.textarea.clone()} aria-label={props.label.clone()} rows={props.rows.to_string()} class="code" spellcheck="false" autocomplete="off"
                role="combobox" aria-autocomplete="list" aria-expanded={open.to_string()} aria-controls={(*listbox).clone()}
                aria-activedescendant={open.then(|| format!("{}-{current}", *listbox))}
                value={props.value.clone()} placeholder={props.placeholder.clone()}
                {oninput} {onkeydown}
                onkeyup={Callback::from({let read=read_selection.clone();move |_:KeyboardEvent|read.emit(())})}
                onclick={Callback::from({let read=read_selection.clone();move |_|read.emit(())})}
                onselect={Callback::from({let read=read_selection.clone();move |_|read.emit(())})}
                onfocus={Callback::from({let focused=focused.clone();let read=read_selection.clone();move |_|{focused.set(true);read.emit(());}})}
                onblur={Callback::from({let focused=focused.clone();move |_|focused.set(false)})}/>
        if open {<div class="formula-completions" role="listbox" id={(*listbox).clone()} aria-label={if matches!(context,Some(Context::Field{..})){"Field suggestions"}else{"Function suggestions"}}>
            {for options.iter().enumerate().map(|(index, choice)| html!{
                <div role="option" id={format!("{}-{index}", *listbox)} aria-selected={(index == current).to_string()} class={if index == current {"formula-option active"} else {"formula-option"}} onmousedown={choose(choice.clone())}>
                    <strong>{choice.title.clone()}</strong><span>{choice.detail.clone()}</span>
                </div>
            })}
        </div>}
        </div>
        {signature.unwrap_or_default()}
        if let Some(message) = problem {<p class="formula-status invalid" role="status">{message}</p>}
        else if !props.value.trim().trim_start_matches('=').is_empty() {<p class="formula-status ok" role="status">{"✓ Formula is valid"}</p>}
        if props.library {
            <div class="row"><button type="button" class="secondary" aria-expanded={library.to_string()} onclick={Callback::from({let library=library.clone();move |_|library.set(!*library)})}>{if *library {"Hide functions"} else {"Browse functions"}}</button><span class="muted">{"Type a function name or [ for fields. ↑ ↓ to choose, Enter to insert."}</span></div>
            if *library {<FunctionLibrary advanced={true} replace={false} on_use={Callback::noop()} on_insert={insert_function}/>}
        }
    </div>}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> Vec<Suggestion> {
        ["price", "quantity", "orders.amount", "orders.customer"]
            .iter()
            .map(|n| Suggestion {
                name: n.to_string(),
                label: n.to_string(),
                detail: String::new(),
            })
            .collect()
    }

    #[test]
    fn detects_fields_functions_and_ignores_text() {
        assert_eq!(
            context("=[pr", 4),
            Some(Context::Field {
                start: 1,
                query: "pr".into()
            })
        );
        assert_eq!(
            context("=ro", 3),
            Some(Context::Function {
                start: 1,
                query: "ro".into()
            })
        );
        assert_eq!(context("=\"ro", 4), None);
        assert_eq!(
            context("=\"[literal\" & [pr", 18),
            Some(Context::Field {
                start: 14,
                query: "pr".into()
            })
        );
        assert_eq!(context("=[price]", 8), None);
        assert_eq!(context("=[a]b", 5), None);
        assert_eq!(context("=SUM(", 5), None);
    }

    #[test]
    fn accepting_completes_names_and_keeps_the_rest() {
        let field = context("=[pr * 2", 4).unwrap();
        let choice = &choices(&field, &fields())[0];
        assert_eq!(choice.insert, "price");
        assert_eq!(
            accept("=[pr * 2", &field, 4, choice),
            ("=[price] * 2".into(), 8)
        );
        let function = context("=rou", 4).unwrap();
        let choice = &choices(&function, &fields())[0];
        assert_eq!(choice.insert, "ROUND");
        assert_eq!(accept("=rou", &function, 4, choice), ("=ROUND(".into(), 7));
        let first = context("su", 2).unwrap();
        let sum = choices(&first, &fields())
            .into_iter()
            .find(|c| c.insert == "SUMIFS")
            .unwrap();
        assert_eq!(accept("su", &first, 2, &sum).0, "=SUMIFS(");
        // Other collections are suggested once a name is typed.
        let empty = context("=[", 2).unwrap();
        assert!(
            choices(&empty, &fields())
                .iter()
                .all(|c| !c.insert.contains('.'))
        );
        let orders = context("=[ord", 5).unwrap();
        assert_eq!(choices(&orders, &fields()).len(), 2);
    }

    #[test]
    fn finds_the_argument_being_typed() {
        assert_eq!(call_at("=ROUND([price], ", 16), Some(("ROUND".into(), 1)));
        assert_eq!(
            call_at("=SUMIFS([a]; [b]; \"x,y\"; ", 25),
            Some(("SUMIFS".into(), 3))
        );
        assert_eq!(call_at("=ROUND((1+2", 11), Some(("ROUND".into(), 0)));
        assert_eq!(call_at("=ROUND(1, 2) + ", 15), None);
    }

    #[test]
    fn reports_syntax_and_unknown_fields() {
        assert_eq!(problem("=[price] * 2", &fields(), true), None);
        assert!(
            problem("=ROUND([price]", &fields(), true)
                .unwrap()
                .contains("')'")
        );
        assert_eq!(
            problem("=[price] * [tax]", &fields(), true).unwrap(),
            "Unknown field [tax]"
        );
        assert_eq!(problem("=", &fields(), true), None);
        assert_eq!(
            problem("=[orders.total]", &fields(), true).unwrap(),
            "Collection 'orders' has no field 'total'"
        );
        assert!(
            problem("=[nowhere.total]", &fields(), true)
                .unwrap()
                .starts_with("Unknown collection 'nowhere'")
        );
        assert_eq!(problem("=SUM([orders.amount])", &fields(), true), None);
        assert!(
            problem("=SUM([orders.amount])", &fields(), false)
                .unwrap()
                .starts_with("Summary formulas can only use")
        );
    }
}
