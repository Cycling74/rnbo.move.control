use {
    rosc::{OscMessage, OscType},
    serde::{Deserialize, Serialize},
    serde_json::Value,
    std::collections::HashMap,
    uuid::Uuid,
};

#[derive(Serialize)]
pub struct RunnerCmd {
    id: Uuid,
    method: String,
    params: HashMap<String, Value>,
    jsonrpc: &'static str,
}

#[derive(Deserialize)]
pub struct RunnerCmdResult {
    pub progress: f32,

    #[serde(flatten)]
    pub entries: HashMap<String, Value>,
}

#[derive(Deserialize)]
pub struct RunnerCmdResponse {
    id: Uuid,
    pub error: Option<HashMap<String, Value>>,
    pub result: Option<RunnerCmdResult>,
}

impl RunnerCmd {
    pub fn new(method: &str, params: HashMap<String, Value>) -> Self {
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

impl RunnerCmdResponse {
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    pub fn error(&self) -> bool {
        self.error.is_some()
    }

    pub fn take_result(&mut self) -> Option<RunnerCmdResult> {
        self.result.take()
    }
}

impl RunnerCmdResult {
    pub fn done(&self) -> bool {
        self.progress >= 100.0
    }
}
