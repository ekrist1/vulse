//! Rewrites field references inside formula text.

/// Rewrites `[old]` field references to `[new]`, all at once, leaving string
/// literals and other text untouched. Unparseable formulas are rewritten the
/// same way, so a later fix still sees the new names.
pub fn rename_formula_fields(
    formula: &str,
    renames: &std::collections::BTreeMap<String, String>,
) -> String {
    rename_formula_references(formula, |name| renames.get(name).cloned())
}

/// Rewrites each `[name]` that `rename` maps to a new name, leaving string
/// literals untouched. Used for renamed fields and for `[collection.field]`
/// references when another collection or its fields are renamed.
pub fn rename_formula_references(formula: &str, rename: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(formula.len());
    let mut chars = formula.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    out.push(chars.next().unwrap());
                } else {
                    in_string = false;
                }
            }
        } else if c == '"' {
            in_string = true;
            out.push(c);
        } else if c == '[' {
            let mut name = String::new();
            let mut closed = false;
            for next in chars.by_ref() {
                if next == ']' {
                    closed = true;
                    break;
                }
                name.push(next);
            }
            out.push('[');
            out.push_str(&rename(&name).unwrap_or(name));
            if closed {
                out.push(']');
            }
        } else {
            out.push(c);
        }
    }
    out
}
