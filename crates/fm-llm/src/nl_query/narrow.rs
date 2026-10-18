//! The query schema narrowed to one ledger, before it becomes a grammar.
//!
//! The schema T4.1 publishes accepts any category key, account id or currency. The
//! grammar the model decodes against accepts only the ones this ledger has, so a
//! model can't write a key that would fail when the query runs. It also:
//! - caps a query at [`MAX_ITEMS`] items, so a model can't pad an answer;
//! - requires a scope's period, so the model has to decide it rather than default
//!   to all time;
//! - drops `null` from optional fields, so the model leaves them out instead;
//! - makes a merchant one of the ledger's merchant names, and an original currency one
//!   recorded abroad (never the account's own).

use serde_json::{Value as Json, json};

use super::{GbnfError, Vocabulary};

/// Most items a query may have. TD.3's widest question needs 4.
pub const MAX_ITEMS: u64 = 6;

fn at<'a>(v: &'a mut Json, path: &[&str]) -> Result<&'a mut Json, GbnfError> {
    let mut cur = v;
    for p in path {
        cur = cur
            .get_mut(*p)
            .ok_or_else(|| GbnfError::Malformed(path.join("/")))?;
    }
    Ok(cur)
}

/// `{"type": [t, "null"], ...}` → `{"type": t, ...}`; `{"oneOf": [x, {"type": "null"}]}` → `x`.
fn not_null(field: &mut Json) {
    if let Some(Json::Array(types)) = field.get_mut("type") {
        types.retain(|t| t != "null");
        if types.len() == 1 {
            let only = types[0].clone();
            field["type"] = only;
        }
    }
    if let Some(Json::Array(options)) = field.get("oneOf").cloned() {
        let kept: Vec<Json> = options
            .into_iter()
            .filter(|o| o != &json!({"type": "null"}))
            .collect();
        if kept.len() == 1 {
            *field = kept.into_iter().next().unwrap_or_default();
        }
    }
}

fn strings(values: impl IntoIterator<Item = String>) -> Json {
    Json::Array(values.into_iter().map(Json::String).collect())
}

/// `schema` (T4.1's `query::SCHEMA`) narrowed to `vocabulary`.
pub fn narrow(schema: &Json, vocabulary: &Vocabulary) -> Result<Json, GbnfError> {
    let mut s = schema.clone();
    let defs = ["$defs"];
    let def = |name: &'static str| [defs[0], name];

    at(&mut s, &def("Query"))?["properties"]["items"]["maxItems"] = json!(MAX_ITEMS);

    let categories = strings(vocabulary.categories.iter().map(|c| c.key.clone()));
    let groups = strings(vocabulary.groups.iter().map(|g| g.key.clone()));

    let scope = at(&mut s, &def("Scope"))?;
    scope["required"] = json!(["period"]);
    let props = scope
        .get_mut("properties")
        .and_then(Json::as_object_mut)
        .ok_or_else(|| GbnfError::Malformed("Scope".into()))?;
    for field in props.values_mut() {
        not_null(field);
    }
    props["categories"]["items"] = json!({ "enum": categories });
    props["groups"]["items"] = json!({ "enum": groups });
    if vocabulary.currencies.is_empty() {
        props.remove("original_currency");
        props.remove("amounts");
    } else {
        props["original_currency"] =
            json!({ "enum": strings(vocabulary.currencies.iter().cloned()) });
    }
    if !vocabulary.merchants.is_empty() {
        props["merchant"] = json!({ "enum": strings(vocabulary.merchants.iter().cloned()) });
    }

    let ids: Vec<Json> = vocabulary.accounts.iter().map(|a| json!(a.id)).collect();
    let select = at(&mut s, &def("AccountSelect"))?;
    let props = select
        .get_mut("properties")
        .and_then(Json::as_object_mut)
        .ok_or_else(|| GbnfError::Malformed("AccountSelect".into()))?;
    if ids.is_empty() {
        props.remove("ids");
    } else {
        props["ids"]["items"] = json!({ "enum": ids });
    }

    let series = at(&mut s, &def("SeriesSelect"))?;
    let props = series
        .get_mut("properties")
        .and_then(Json::as_object_mut)
        .ok_or_else(|| GbnfError::Malformed("SeriesSelect".into()))?;
    for field in props.values_mut() {
        not_null(field);
    }
    props["categories"]["items"] = json!({ "enum": categories });
    Ok(s)
}
