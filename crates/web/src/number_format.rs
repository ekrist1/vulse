//! Numbers shown in a field's display format, laid out by the browser's
//! `Intl.NumberFormat`. Stored values and inputs keep plain numbers.

use serde_json::Value;
use vulse_model::{FieldDef, NumberFormat};
use wasm_bindgen::{JsCast, JsValue};

/// A field's value as the admin shows it: numbers in the field's number
/// format, anything else as `display_value`.
pub fn field_display(field: &FieldDef, value: &Value) -> String {
    match (&field.number_format, value.as_f64()) {
        (Some(format), Some(number)) => format_number(number, format),
        _ => vulse_model::display_value(value),
    }
}

/// `number` grouped and rounded as `format` says, such as `11 945 217 kr`
/// for NOK in `nb-NO`. Falls back to the plain number if the browser refuses
/// the locale or currency.
pub fn format_number(number: f64, format: &NumberFormat) -> String {
    formatter(format)
        .and_then(|format| format.call1(&JsValue::NULL, &number.into()).ok())
        .and_then(|text| text.as_string())
        .unwrap_or_else(|| number.to_string())
}

fn formatter(format: &NumberFormat) -> Option<js_sys::Function> {
    let options = js_sys::Object::new();
    let set = |key: &str, value: JsValue| js_sys::Reflect::set(&options, &key.into(), &value);
    if let Some(code) = &format.currency {
        set("style", "currency".into()).ok()?;
        set("currency", code.as_str().into()).ok()?;
    }
    match format.decimals {
        Some(digits) => {
            set("minimumFractionDigits", digits.into()).ok()?;
            set("maximumFractionDigits", digits.into()).ok()?;
        }
        // Plain numbers keep their stored digits instead of rounding to three.
        None if format.currency.is_none() => {
            set("maximumFractionDigits", 20.into()).ok()?;
        }
        None => {}
    }
    let locales = js_sys::Array::new();
    if let Some(tag) = &format.locale {
        locales.push(&tag.as_str().into());
    }
    let intl = js_sys::Reflect::get(&js_sys::global(), &"Intl".into()).ok()?;
    let class: js_sys::Function = js_sys::Reflect::get(&intl, &"NumberFormat".into())
        .ok()?
        .dyn_into()
        .ok()?;
    // Constructed through Reflect so an unknown locale or currency is an
    // error here instead of a panic.
    let instance =
        js_sys::Reflect::construct(&class, &js_sys::Array::of2(&locales, &options)).ok()?;
    js_sys::Reflect::get(&instance, &"format".into())
        .ok()?
        .dyn_into()
        .ok()
}
