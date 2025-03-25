use {
    rosc::{OscMessage, OscType},
    serde::Serialize,
    serde_json::{map::Map, Value},
    uuid::Uuid,
};

#[derive(Serialize)]
pub struct RunnerCmd {
    id: Uuid,
    method: String,
    params: Map<String, Value>,
    jsonrpc: &'static str,
}

impl RunnerCmd {
    pub fn new(method: &str, params: Map<String, Value>) -> Self {
        let id = uuid::Uuid::new_v4();
        Self {
            id,
            method: method.to_string(),
            params,
            jsonrpc: &"2.0",
        }
    }

    pub fn id(&self) -> &Uuid {
        &self.id
    }

    pub fn into_osc(self) -> OscMessage {
        let args = serde_json::to_string(&self).expect("to convert cmd to string");
        let args = vec![OscType::String(args)];
        OscMessage {
            addr: "/rnbo/cmd".to_owned(),
            args,
        }
    }
}
