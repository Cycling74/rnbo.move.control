use serde::{Deserialize, Serialize};

#[derive(Deserialize, Clone, Debug)]
pub struct StartupProcess {
    pub cmd: String,
    pub args: Option<Vec<String>>,
}

#[derive(Deserialize, Debug)]
pub struct StartupConfig {
    pub jack: Option<StartupProcess>,
    pub apps: Option<Vec<StartupProcess>>,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct Config {
    pub volume: u8, //0..255
}

impl Default for Config {
    fn default() -> Self {
        Self { volume: 127 }
    }
}
