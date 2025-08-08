use serde_json::Value;

pub fn parse_meta(body: &Value) -> Option<Value> {
    let meta = body
        .as_object()?
        .get("CONTENTS")?
        .as_object()?
        .get("meta")?
        .as_object()?
        .get("VALUE")?
        .as_str()?;
    serde_json::from_str(meta).ok()
}
