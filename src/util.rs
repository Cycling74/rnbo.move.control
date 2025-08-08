use serde_json::Value;

pub fn parse_contents_meta(contents: &Value) -> Option<Value> {
    let meta = contents
        .as_object()?
        .get("meta")?
        .as_object()?
        .get("VALUE")?
        .as_str()?;
    serde_json::from_str(meta).ok()
}

pub fn parse_body_meta(body: &Value) -> Option<Value> {
    parse_contents_meta(body.as_object()?.get("CONTENTS")?)
}
