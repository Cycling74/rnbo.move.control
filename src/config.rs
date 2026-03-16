use {
    serde::{Deserialize, Serialize},
    std::{fs::File, io::BufReader, path::PathBuf},
};

const DEFAULT_OSC_PORT: u16 = 2345;

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
    pub oscport: Option<u16>,
    pub param_step_detents: Option<u8>,
}

impl Config {
    pub fn read_or_default(config_path: &PathBuf) -> Self {
        if std::path::Path::exists(&config_path) {
            if let Ok(file) = File::open(&config_path) {
                let reader = BufReader::new(file);
                serde_json::from_reader(reader).unwrap_or_default()
            } else {
                Self::default()
            }
        } else {
            Self::default()
        }
    }

    pub fn oscport(&self) -> u16 {
        self.oscport.unwrap_or(DEFAULT_OSC_PORT)
    }

    pub fn param_step_detents(&self) -> u8 {
        self.param_step_detents.unwrap_or(1 as _).max(1)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            volume: 178, // ~ 70%
            oscport: Some(DEFAULT_OSC_PORT),
            param_step_detents: Some(1),
        }
    }
}
